/**
 * Small pieces of reply plumbing shared by commands, components and free-text messages, so
 * the out-of-credits answer reads the same wherever it appears.
 */
import {
  ActionRowBuilder,
  ButtonBuilder,
  ButtonStyle,
  type InteractionEditReplyOptions,
  type InteractionReplyOptions,
  type MessageCreateOptions,
} from 'discord.js';

import { assistantFailureCopy } from './format.js';
import { BackendError } from './http.js';

/** Anything that can answer once: a slash command, a button, a select or a modal submit. */
export interface Replyable {
  replied: boolean;
  deferred: boolean;
  reply(options: InteractionReplyOptions): Promise<unknown>;
  editReply(options: InteractionEditReplyOptions): Promise<unknown>;
}

export async function respond(target: Replyable, payload: InteractionReplyOptions): Promise<void> {
  if (target.deferred || target.replied) {
    await target.editReply(payload as InteractionEditReplyOptions);
    return;
  }
  await target.reply(payload);
}

/**
 * "Buy credits" is an interaction, not a link: the portal has no public pricing page, and a
 * bare link would land an unauthenticated visitor on the sign-in screen. The button mints a
 * one-time portal link and answers privately.
 */
export function buyCreditsRow(): ActionRowBuilder<ButtonBuilder> {
  return new ActionRowBuilder<ButtonBuilder>().addComponents(
    new ButtonBuilder().setCustomId('credits:buy').setLabel('Buy credits').setStyle(ButtonStyle.Primary),
  );
}

export interface AssistantErrorReply {
  content: string;
  components?: ActionRowBuilder<ButtonBuilder>[];
}

/**
 * Maps a failed assistant call to something a person can act on. The backend's message is
 * never echoed: the codes are the contract, and a 500 body can contain internals.
 */
export function assistantErrorReply(err: unknown): AssistantErrorReply {
  if (err instanceof BackendError) {
    const copy = assistantFailureCopy(err.status, err.code);
    return copy.offerPurchase
      ? { content: copy.content, components: [buyCreditsRow()] }
      : { content: copy.content };
  }
  return { content: 'Something went wrong on my side. Please try again in a moment.' };
}

/** Keeps a message inside Discord's 2000-character ceiling without cutting a link in half. */
export function clampContent(content: string, max = 1900): string {
  return content.length <= max ? content : `${content.slice(0, max - 20)}\n…(truncated)`;
}

export type Sendable = MessageCreateOptions;
