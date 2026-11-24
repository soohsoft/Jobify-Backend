/**
 * What a handler needs, assembled once at boot. Keeping it a plain object (rather than
 * reaching for the Discord client inside handlers) is what lets the command logic be
 * exercised without a gateway in tests.
 */
import type { Client } from 'discord.js';

import type { Backend } from './http.js';
import type { Logger } from './log.js';
import type { OnboardingSessions } from './onboarding.js';
import type { Category } from './types.js';

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
  sendDm: (discordId: string, payload: string) => Promise<void>;
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

export function createDmSender(client: Client): (discordId: string, payload: string) => Promise<void> {
  return async (discordId, payload) => {
    const user = await client.users.fetch(discordId);
    await user.send(payload);
  };
}
