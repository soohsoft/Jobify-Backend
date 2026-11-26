/**
 * What a handler needs, assembled once at boot. Keeping it a plain object (rather than
 * reaching for the Discord client inside handlers) is what lets the command logic be
 * exercised without a gateway in tests.
 */
import type {
  ActionRowBuilder,
  ButtonBuilder,
  Client,
  DMChannel,
  StringSelectMenuBuilder,
} from 'discord.js';

import type { Backend } from './http.js';
import type { Logger } from './log.js';
import {
  DEFAULT_DM_LIMITS,
  DiscordRateLimiter,
  isUnknownChannel,
  withDiscordRetry,
} from './ratelimit.js';
import type { OnboardingSessions } from './onboarding.js';
import type { Category } from './types.js';

/** Any row a message may carry: buttons, or a select menu (the owner type and the position picker). */
export type AnyActionRow =
  | ActionRowBuilder<ButtonBuilder>
  | ActionRowBuilder<StringSelectMenuBuilder>;

/** A message with buttons: the welcome DM carries the interview's first step. */
export interface MessagePayload {
  content: string;
  components?: AnyActionRow[];
  /** True when the message is an interview step: it must not offer the Jobs/Tenders tabs. */
  hideTabs?: boolean;
}

/** What `channel.send` needs for the fallback: the mention is explicit, never parsed from text. */
export interface ChannelPayload extends MessagePayload {
  allowedMentions?: { users?: string[] };
}

/** A DM to a Discord id; throws when the member cannot be reached. */
export type SendDm = (discordId: string, payload: string | MessagePayload) => Promise<void>;

/** A message posted into a known channel. */
export type SendChannel = (channelId: string, payload: ChannelPayload) => Promise<void>;

/**
 * Answers an event once.
 *
 * Discord can deliver the same interaction or message more than once (a resumed gateway
 * session, a client retry, a double registration), and every handler here sends a list.
 * Claiming the id once is what keeps one request from producing two identical messages.
 * In-memory and time-bounded on purpose: a duplicate arrives within seconds, not hours.
 */
export class RequestGuard {
  private seen = new Map<string, number>();

  constructor(private readonly ttlMs = 10 * 60_000, private readonly maxEntries = 2_000) {}

  /** True the first time an id is seen, false for every repeat. */
  claim(id: string | undefined | null): boolean {
    if (!id) return true;
    const now = Date.now();
    if (this.seen.has(id)) return false;
    this.seen.set(id, now);
    if (this.seen.size > this.maxEntries) {
      for (const [key, at] of this.seen) {
        if (now - at > this.ttlMs) this.seen.delete(key);
      }
    }
    return true;
  }
}

/**
 * The bot-wide outbound pacer.
 *
 * One instance is shared by every send the bot makes on its own initiative OR in reply to a
 * member's message, because Discord's limits are per bot, not per lane: a thousand members
 * writing at once must not add up to a thousand simultaneous channel writes.
 */
export interface OutboundPacer {
  /** Waits until a send to `key` is allowed. A smaller gap is used for replies. */
  acquire(key: string, gapMs?: number): Promise<void>;
}

/** A gap that keeps a member's own conversation responsive while still spacing a crowd. */
export const REPLY_GAP_MS = 300;

export interface BotContext {
  /** Optional so tests can build a context without a pacer; production always sets it. */
  outbound?: OutboundPacer;
  backend: Backend;
  appBaseUrl: string;
  digestTz: string;
  digestHour: number;
  logger: Logger;
  sessions: OnboardingSessions;
  /** A guild channel where plain messages are answered without a mention. Unset = mention only. */
  freeChannelId?: string;
  /** The taxonomy, cached: it changes only with a deploy and every interview needs it. */
  categories: () => Promise<Category[]>;
  /** A DM to a Discord id; throws when the member cannot be reached. */
  sendDm: SendDm;
  /** Posts into a known channel; used for the join fallback and the pinned intro. */
  sendChannel?: SendChannel;
  /** Answers each interaction/message once, so a list is never sent twice for one request. */
  guard: RequestGuard;
}

export function createCategoryCache(backend: Backend, ttlMs = 10 * 60_000): () => Promise<Category[]> {
  let cached: Category[] | undefined;
  let loadedAt = 0;
  return async () => {
    const now = Date.now();
    if (cached && now - loadedAt < ttlMs) return cached;
    cached = await backend.categories();
    loadedAt = now;
    return cached;
  };
}

/**
 * A DM sender that paces itself.
 *
 * Two things matter here and both are about not looking like spam:
 *
 * - the DM channel is CACHED per member. `users.fetch()` + `user.send()` opens a channel every
 *   time, and those channel opens are the request Discord throttles hardest; a digest over a
 *   thousand members would otherwise do two thousand round trips for one message each.
 * - the send waits on `DiscordRateLimiter` and retries a 429 for exactly as long as Discord
 *   says. Replies to a member's own action never come through here, so this adds no delay to a
 *   conversation — only to the proactive digests.
 */
export function createDmSender(
  client: Client,
  options: {
    limiter?: DiscordRateLimiter;
    logger?: Logger;
  } = {},
): SendDm {
  const limiter = options.limiter ?? new DiscordRateLimiter(DEFAULT_DM_LIMITS);
  const channels = new Map<string, DMChannel>();

  const openChannel = async (discordId: string): Promise<DMChannel> => {
    const cached = channels.get(discordId);
    if (cached) return cached;
    const user = await client.users.fetch(discordId);
    const channel = (await user.createDM()) as DMChannel;
    channels.set(discordId, channel);
    return channel;
  };

  return async (discordId, payload) => {
    let channel = await openChannel(discordId);
    await limiter.acquire(discordId);
    try {
      await withDiscordRetry(() => channel.send(payload), {
        attempts: 3,
        onRetry: (waitMs, err) => {
          options.logger?.warn('discord: waited out a rate limit', {
            discordId,
            waitMs,
            reason: err instanceof Error ? err.message : String(err),
          });
        },
        // A channel that was deleted (or a cache entry that went stale) is not rate limiting:
        // drop it and open a fresh one before the next attempt.
        onRetryable: async () => {
          channels.delete(discordId);
          channel = await openChannel(discordId);
        },
      });
    } catch (err) {
      // Keep the cache honest: a channel that failed for a non-retryable reason is discarded so
      // the next digest starts from a fresh fetch instead of reusing it forever.
      if (isUnknownChannel(err)) channels.delete(discordId);
      throw err;
    }
  };
}

export function createChannelSender(client: Client): SendChannel {
  return async (channelId, payload) => {
    const channel = await client.channels.fetch(channelId);
    if (!channel || !('send' in channel) || typeof channel.send !== 'function') {
      throw new Error(`Channel ${channelId} is not a text channel`);
    }
    // An explicit `allowedMentions` keeps the one intended @mention and nothing else.
    await channel.send(payload);
  };
}
