/**
 * What a handler needs, assembled once at boot. Keeping it a plain object (rather than
 * reaching for the Discord client inside handlers) is what lets the command logic be
 * exercised without a gateway in tests.
 */
import type { ActionRowBuilder, ButtonBuilder, Client } from 'discord.js';

import type { Backend } from './http.js';
import type { Logger } from './log.js';
import type { OnboardingSessions } from './onboarding.js';
import type { Category } from './types.js';

/** A message with buttons: the welcome DM carries the interview's first step. */
export interface MessagePayload {
  content: string;
  components?: ActionRowBuilder<ButtonBuilder>[];
}

/** What `channel.send` needs for the fallback: the mention is explicit, never parsed from text. */
export interface ChannelPayload extends MessagePayload {
  allowedMentions?: { users?: string[] };
}

/** A DM to a Discord id; throws when the member cannot be reached. */
export type SendDm = (discordId: string, payload: string | MessagePayload) => Promise<void>;

/** A message posted into a known channel. */
export type SendChannel = (channelId: string, payload: ChannelPayload) => Promise<void>;

export interface BotContext {
  backend: Backend;
  appBaseUrl: string;
  digestTz: string;
  digestHour: number;
  logger: Logger;
  sessions: OnboardingSessions;
  /** A guild channel where plain messages are answered without a mention. Unset = mention only. */
  freeChannelId?: string;
  /** The taxonomy, cached: it changes only with a deploy and every `/start` needs it. */
  categories: () => Promise<Category[]>;
  /** A DM to a Discord id; throws when the member cannot be reached. */
  sendDm: SendDm;
  /** Posts into a known channel; used for the join fallback and the pinned intro. */
  sendChannel?: SendChannel;
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
