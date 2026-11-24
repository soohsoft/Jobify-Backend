/**
 * The pinned "How Jobify works" message in the free channel.
 *
 * Every restart calls `ensureIntro`, so it must be idempotent: the markers below are how an
 * existing post is recognised without a database. The scan covers pinned messages *and* the
 * last 50, because a pin can be removed by a moderator while the message stays in history —
 * then we neither duplicate it nor silently re-pin something a human unpinned.
 *
 * A marker line (`-# …`) renders as small grey text: visible enough to be useful, quiet
 * enough not to clutter the post.
 */
import { Collection } from 'discord.js';

import type { Logger } from './log.js';
import { errorFields } from './log.js';
import { clampContent } from './ui.js';

export const INTRO_MARKER = '-# jobify-intro v1';

/** The minimum a message must expose; discord.js `Message` satisfies it. */
export interface IntroMessage {
  id: string;
  content: string;
  pinned?: boolean;
  pin(): Promise<unknown>;
  edit(payload: { content: string }): Promise<unknown>;
}

/** The minimum a channel must expose; discord.js `TextChannel` satisfies it. */
export interface IntroChannel {
  id: string;
  messages: {
    /** discord.js >= 14.26; the deprecated `fetchPinned` is kept as a fallback. */
    fetchPins?(): Promise<{ items: Collection<string, IntroMessage> }>;
    fetchPinned?(): Promise<Collection<string, IntroMessage>>;
    fetch(options: { limit: number }): Promise<Collection<string, IntroMessage>>;
  };
  send(payload: { content: string; allowedMentions?: { parse: [] } }): Promise<IntroMessage>;
}

/** Africa/Nairobi is EAT; any other zone is printed by name so the copy stays honest. */
export function zoneLabel(tz: string): string {
  if (tz === 'Africa/Nairobi') return 'EAT';
  return tz;
}

export function introContent(digestHour = 8, digestTz = 'Africa/Nairobi'): string {
  const hours = `${String(digestHour).padStart(2, '0')}:00`;
  return clampContent(
    [
      '**How Jobify works**',
      'I watch Somali job and tender listings and send the ones that match your profile straight to your DMs — there is no website to check.',
      '',
      '**Ask me anything**',
      "Type a role and a place — e.g. `nurse jobs in Mogadishu` — and I answer with live openings. You can type right here in this channel, no @mention needed.",
      '',
      '**Commands**',
      '`/start` — set up or redo your 3-question interview',
      '`/jobs` · `/tenders` — browse everything',
      '`/alerts` — turn the daily digest on or off',
      '`/account` — payment and company profile',
      '',
      '**The digest**',
      `Every morning at ${hours} ${zoneLabel(digestTz)} I DM you your matches. You can turn alerts off at any time with \`/alerts\`.`,
      '',
      INTRO_MARKER,
    ].join('\n'),
  );
}

export function hasMarker(message: Pick<IntroMessage, 'content'>): boolean {
  return message.content.includes(INTRO_MARKER);
}

export type IntroOutcome = 'exists' | 'updated' | 'repinned' | 'posted' | 'posted-unpinned';

/** Serialises calls within the process: two ready events must not race into two posts. */
let queue: Promise<unknown> = Promise.resolve();

export function ensureIntro(
  channel: IntroChannel,
  logger: Logger,
  content = introContent(),
): Promise<IntroOutcome> {
  const run = queue.then(() => ensureIntroOnce(channel, logger, content));
  // Keep the chain alive after a failure so the next call still runs.
  queue = run.catch(() => undefined);
  return run;
}

/**
 * `fetchPinned()` is deprecated in current discord.js but still the only name older 14.x has.
 * Preferring `fetchPins()` keeps the logs free of a deprecation warning on every boot.
 */
async function fetchPinned(channel: IntroChannel): Promise<Collection<string, IntroMessage>> {
  if (channel.messages.fetchPins) {
    return (await channel.messages.fetchPins()).items;
  }
  if (channel.messages.fetchPinned) {
    return channel.messages.fetchPinned();
  }
  return new Collection<string, IntroMessage>();
}

async function ensureIntroOnce(channel: IntroChannel, logger: Logger, content: string): Promise<IntroOutcome> {
  const pinned = await fetchPinned(channel);
  const recent = await channel.messages.fetch({ limit: 50 });

  // Pinned first, then history: a duplicate can only appear if a human posted one by hand.
  const seen = new Map<string, IntroMessage>();
  for (const message of [...pinned.values(), ...recent.values()]) seen.set(message.id, message);
  const existing = [...seen.values()].find(hasMarker);

  if (existing) {
    if (existing.pinned && existing.content !== content) {
      await existing.edit({ content });
      logger.info('intro message updated in place', { channelId: channel.id, messageId: existing.id });
      return 'updated';
    }
    if (!existing.pinned) {
      // The content is left untouched, but a bot-owned intro should be on the pin list: this
      // is the repair path for a run that lacked Manage Messages or a pin a moderator cleared.
      try {
        await existing.pin();
        logger.info('intro message re-pinned', { channelId: channel.id, messageId: existing.id });
        return 'repinned';
      } catch (err) {
        logger.warn('intro message exists but is not pinned', {
          channelId: channel.id,
          messageId: existing.id,
          hint: 'grant the bot Manage Messages and restart, or delete the message to repost it',
          ...errorFields(err),
        });
        return 'exists';
      }
    }
    logger.info('intro message already present', {
      channelId: channel.id,
      messageId: existing.id,
      pinned: true,
    });
    return 'exists';
  }

  const sent = await channel.send({ content, allowedMentions: { parse: [] } });
  try {
    await sent.pin();
    logger.info('intro message posted and pinned', { channelId: channel.id, messageId: sent.id });
    return 'posted';
  } catch (err) {
    // Missing Manage Messages must not stop the bot; the text is still useful, just not on top.
    logger.warn('intro message posted but pinning failed', {
      channelId: channel.id,
      messageId: sent.id,
      ...errorFields(err),
    });
    return 'posted-unpinned';
  }
}
