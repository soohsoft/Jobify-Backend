/**
 * A very small Telegram Bot API client.
 *
 * Deliberately dependency-free: the calls this bot needs are `getUpdates`, `sendMessage`,
 * `editMessageText`, `answerCallbackQuery` and `setMyCommands`, and a library would add a
 * framework to learn for five endpoints. Node 22 has `fetch`, so this is a thin typed wrapper.
 *
 * Two behaviours are built in because Telegram punishes them hard:
 * - a token bucket at ~25 sends/s (Telegram's documented ceiling is ~30/s bot-wide), shared by
 *   everything this bot sends;
 * - `429` handling that waits out `retry_after` instead of retrying into the wall.
 */
import { DiscordRateLimiter, withDiscordRetry } from '../ratelimit.js';

export interface TgUser {
  id: number;
  is_bot: boolean;
  first_name: string;
  last_name?: string;
  username?: string;
}

export interface TgChat {
  id: number;
  type: 'private' | 'group' | 'supergroup' | 'channel';
}

export interface TgMessage {
  message_id: number;
  from?: TgUser;
  chat: TgChat;
  text?: string;
  date: number;
}

export interface TgCallbackQuery {
  id: string;
  from: TgUser;
  data?: string;
  message?: TgMessage;
}

export interface TgUpdate {
  update_id: number;
  message?: TgMessage;
  callback_query?: TgCallbackQuery;
}

export interface InlineButton {
  text: string;
  callback_data?: string;
  url?: string;
}

export type InlineKeyboard = InlineButton[][];

export interface SendResult {
  message_id: number;
  chat: TgChat;
}

/** Thrown when the API answers `ok: false`; `code` is Telegram's error_code. */
export class TelegramError extends Error {
  constructor(
    message: string,
    readonly code: number,
    readonly description: string,
  ) {
    super(message);
    this.name = 'TelegramError';
  }
}

export interface TelegramClientOptions {
  token: string;
  /** Defaults to the public API; a local Bot API server can be set for 2 GB files. */
  baseUrl?: string;
  /** Sends per second bot-wide. Krill-safe default well under Telegram's ~30. */
  perSecond?: number;
  /** Minimum gap between two messages in the same chat; tests set 0. */
  perUserMs?: number;
  /** Injected in tests. */
  fetchImpl?: typeof fetch;
}

export class TelegramClient {
  private readonly token: string;
  private readonly baseUrl: string;
  private readonly fetchImpl: typeof fetch;
  private readonly limiter: DiscordRateLimiter;
  private offset = 0;

  constructor(options: TelegramClientOptions) {
    if (!options.token) throw new Error('a Telegram bot token is required');
    this.token = options.token;
    this.baseUrl = (options.baseUrl ?? 'https://api.telegram.org').replace(/\/+$/, '');
    this.fetchImpl = options.fetchImpl ?? fetch;
    this.limiter = new DiscordRateLimiter({
      burst: 5,
      perSecond: options.perSecond ?? 25,
      // One chat tolerates about one message per second; 1.1 s keeps us under it.
      perUserMs: options.perUserMs ?? 1_100,
    });
  }

  /** POSTs one Bot API method, paced and retried. */
  async call<T>(method: string, payload: Record<string, unknown> = {}): Promise<T> {
    const url = `${this.baseUrl}/bot${this.token}/${method}`;
    await this.limiter.acquire(String(payload.chat_id ?? method));
    return withDiscordRetry(
      async () => {
        const response = await this.fetchImpl(url, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify(payload),
        });
        const body = (await response.json()) as {
          ok: boolean;
          result?: T;
          description?: string;
          error_code?: number;
          parameters?: { retry_after?: number };
        };
        if (!body.ok) {
          const description = body.description ?? 'Telegram API error';
          const retryAfter = body.parameters?.retry_after;
          const err = new TelegramError(description, body.error_code ?? 0, description) as TelegramError & {
            retryAfter?: number;
          };
          if (typeof retryAfter === 'number') err.retryAfter = retryAfter;
          throw err;
        }
        return body.result as T;
      },
      { attempts: 3, jitterMs: 200 },
    );
  }

  async sendMessage(
    chatId: number,
    text: string,
    keyboard?: InlineKeyboard,
    extra: Record<string, unknown> = {},
  ): Promise<SendResult> {
    return this.call<SendResult>('sendMessage', {
      chat_id: chatId,
      text,
      parse_mode: 'HTML',
      link_preview_options: { is_disabled: true },
      ...(keyboard ? { reply_markup: { inline_keyboard: keyboard } } : {}),
      ...extra,
    });
  }

  async editMessage(
    chatId: number,
    messageId: number,
    text: string,
    keyboard?: InlineKeyboard,
  ): Promise<void> {
    await this.call('editMessageText', {
      chat_id: chatId,
      message_id: messageId,
      text,
      parse_mode: 'HTML',
      link_preview_options: { is_disabled: true },
      ...(keyboard ? { reply_markup: { inline_keyboard: keyboard } } : {}),
    });
  }

  /**
   * Acknowledges a button tap. Telegram expires a callback query after a few minutes, so a tap
   * on an old message answers `query is too old` — that is not an error worth failing an update
   * over, and rethrowing it aborted the step the member actually asked for.
   */
  async answerCallback(id: string, text?: string): Promise<void> {
    try {
      await this.call('answerCallbackQuery', { callback_query_id: id, ...(text ? { text } : {}) });
    } catch (err) {
      const description = err instanceof TelegramError ? err.description : '';
      if (/query is too old|query ID is invalid/i.test(description)) return;
      throw err;
    }
  }

  async setCommands(commands: Array<{ command: string; description: string }>): Promise<void> {
    await this.call('setMyCommands', { commands });
  }

  /** Long-polls for updates, handing each batch to `handler`. Resolves when `signal` aborts. */
  async run(handler: (update: TgUpdate) => Promise<void>, signal?: AbortSignal): Promise<void> {
    for (;;) {
      if (signal?.aborted) return;
      let updates: TgUpdate[] = [];
      try {
        updates = await this.call<TgUpdate[]>('getUpdates', {
          offset: this.offset,
          timeout: 30,
          allowed_updates: ['message', 'callback_query'],
        });
      } catch (err) {
        // A network blip must not kill the loop: log-and-wait is the whole recovery.
        await new Promise((resolve) => setTimeout(resolve, 1_000));
        if (signal?.aborted) return;
        continue;
      }
      for (const update of updates) {
        this.offset = update.update_id + 1;
        try {
          await handler(update);
        } catch (err) {
          // One bad update cannot stop the bot.
          console.error(JSON.stringify({ level: 'error', msg: 'update failed', err: String(err) }));
        }
      }
    }
  }
}
