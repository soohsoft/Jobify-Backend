/**
 * Free text: a DM to the bot, or a message that @mentions it, goes to the same purpose-bound
 * assistant as `/jobs`, with the sender's snowflake as the identity.
 *
 * The reply is the assistant's own sentence plus up to three real rows as links. Nothing else
 * is generated here — no summarising, no "here are some tips" — because the assistant is the
 * only thing allowed to speak about Jobify's listings.
 */
import type { Message } from 'discord.js';

import type { BotContext } from './context.js';
import { formatAssistantReply } from './format.js';
import { assistantErrorReply, clampContent } from './ui.js';

const MAX_INPUT = 300;

export async function handleMessage(message: Message, ctx: BotContext, botId: string): Promise<void> {
  if (message.author.bot) return;
  if (!message.content) return;

  const mentioned = message.mentions.users.has(botId);
  // In a guild the MESSAGE CONTENT intent decides whether `content` is populated at all;
  // without it a mention arrives with an empty body. DMs need no intent, which is why the
  // documented path is "message the bot", not "poke it in a server".
  if (message.guild && !mentioned) return;

  const text = mentioned
    ? message.content.replace(new RegExp(`<@!?${botId}>`, 'g'), ' ').trim()
    : message.content.trim();
  if (text.length === 0) {
    await message.reply({
      content: 'Tell me what you are looking for — a role, a place, or both. For example: `data analyst in Mogadishu`.',
      allowedMentions: { parse: [] },
    });
    return;
  }

  if (text.length > MAX_INPUT) {
    await message.reply({
      content: `That is a bit long for me to search. Try a shorter question — under ${MAX_INPUT} characters.`,
      allowedMentions: { parse: [] },
    });
    return;
  }

  if ('sendTyping' in message.channel) {
    await message.channel.sendTyping().catch(() => undefined);
  }

  try {
    await ctx.backend.upsertUser({
      discord_id: message.author.id,
      username: message.author.username,
      global_name: message.author.globalName ?? null,
    });

    const answer = await ctx.backend.assistantQuery(message.author.id, text);
    const content = formatAssistantReply(answer, { appBaseUrl: ctx.appBaseUrl, maxRows: 3 });
    await message.reply({ content: clampContent(content), allowedMentions: { parse: [] } });
    ctx.logger.info('free-text answered', {
      discordId: message.author.id,
      intent: answer.intent,
      rows: answer.jobs.length,
    });
  } catch (err) {
    const failure = assistantErrorReply(err);
    ctx.logger.info('free-text refused or failed', {
      discordId: message.author.id,
      message: err instanceof Error ? err.message : String(err),
    });
    await message
      .reply({
        content: clampContent(failure.content),
        ...(failure.components ? { components: failure.components } : {}),
        allowedMentions: { parse: [] },
      })
      .catch(() => undefined);
  }
}
