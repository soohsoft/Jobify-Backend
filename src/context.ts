/**
 * What a handler needs, assembled once at boot. Keeping it a plain object (rather than
 * reaching for the Discord client inside handlers) is what lets the command logic be
 * exercised without a gateway in tests.
 */
import type {
  ActionRowBuilder,
  ButtonBuilder,
  Client,
  StringSelectMenuBuilder,
} from 'discord.js';

import type { Backend } from './http.js';
import type { Logger } from './log.js';
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

export interface BotContext {
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

export function createDmSender(client: Client): SendDm {
  return async (discordId, payload) => {
    const user = await client.users.fetch(discordId);
    await user.send(payload);
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
