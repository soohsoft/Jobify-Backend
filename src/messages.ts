/**
 * Free text: a DM to the bot, a message in the configured free-text channel, or a message
 * that @mentions it, goes to the same purpose-bound assistant as `/jobs`, with the sender's
 * snowflake as the identity.
 *
 * First contact is different: a member with no usable profile is walked into the interview
 * instead of being answered with the assistant's generic "I can help with..." line. That
 * reply would otherwise be the only thing a brand-new member ever sees, because nothing tells
 * them the interview exists. The onboarding prompt is free — no assistant call, no query charged.
 *
 * The reply to an onboarded member is the assistant's own sentence plus up to three real rows
 * as links. Nothing else is generated here — no summarising, no "here are some tips" — because
 * the assistant is the only thing allowed to speak about Jobify's listings.
 */
import type { Message } from 'discord.js';

import { REPLY_GAP_MS, type BotContext } from './context.js';
import { browseRows } from './commands.js';
import { formatAssistantReply } from './format.js';
import { browseTabsRow, namePrompt, promptForStep, type OnboardingSession } from './onboarding.js';
import { assistantErrorReply, clampContent } from './ui.js';

const MAX_INPUT = 300;

const WELCOME =
  'Welcome to Jobify. Four quick questions and I will start sending you matches. First: what is your name?';

/**
 * Starts the interview the same way the join welcome does, carrying forward the answers
 * already on file so the modals are pre-filled rather than empty.
 */
/**
 * Replies to a member's own message, spending one slot in the bot-wide pacer.
 *
 * The reply still arrives in well under a second for a normal conversation; what the pacer
 * prevents is five thousand members writing in the same minute turning into five thousand
 * simultaneous channel writes, which is what Discord punishes.
 */
async function reply(message: Message, ctx: BotContext, payload: Parameters<Message['reply']>[0]): Promise<void> {
  if (ctx.outbound) await ctx.outbound.acquire(message.channelId, REPLY_GAP_MS);
  await message.reply(payload);
}

async function startInterview(
  message: Message,
  ctx: BotContext,
  existing: { education: import('./types.js').EducationEntry[]; name: string; lastPosition: string | null; experienceYears: number | null },
): Promise<void> {
  ctx.sessions.start(message.author.id, {
    previousEducation: existing.education,
    name: existing.name || undefined,
    lastPosition: existing.lastPosition ?? undefined,
    experienceYears: existing.experienceYears ?? undefined,
  });
  const prompt = namePrompt();
  await reply(message, ctx, {
    content: clampContent(`${WELCOME}\n\n${prompt.content}`),
    components: prompt.components,
    allowedMentions: { parse: [] },
  });
}

/**
 * A member who writes again mid-interview is put back on their current step. The session is
 * reused, never replaced: its in-memory answers and the answers already saved to the backend
 * both stay exactly as they were.
 */
async function resumeInterview(message: Message, ctx: BotContext, session: OnboardingSession): Promise<void> {
  const prompt = promptForStep(session.step, session);
  await reply(message, ctx, {
    content: clampContent(`You are part-way through setup — here is where we left off.\n\n${prompt.content}`),
    components: prompt.components,
    allowedMentions: { parse: [] },
  });
}

export async function handleMessage(message: Message, ctx: BotContext, botId: string): Promise<void> {
  if (message.author.bot) return;
  if (!message.content) return;

  const mentioned = message.mentions.users.has(botId);
  // A configured jobs/support channel is conversational without a mention; every other guild
  // channel keeps today's mention-only rule. DMs are unaffected.
  const inFreeChannel = Boolean(ctx.freeChannelId) && message.channelId === ctx.freeChannelId;
  // In a guild the MESSAGE CONTENT intent decides whether `content` is populated at all;
  // without it a mention arrives with an empty body. DMs need no intent, which is why the
  // documented path is "message the bot", not "poke it in a server".
  if (message.guild && !mentioned && !inFreeChannel) return;

  const text = mentioned
    ? message.content.replace(new RegExp(`<@!?${botId}>`, 'g'), ' ').trim()
    : message.content.trim();
  if (text.length === 0) {
    await reply(message, ctx, {
      content: 'Tell me what you are looking for — a role, a place, or both. For example: `data analyst in Mogadishu`.',
      allowedMentions: { parse: [] },
    });
    return;
  }

  if (text.length > MAX_INPUT) {
    await reply(message, ctx, {
      content: `That is a bit long for me to search. Try a shorter question — under ${MAX_INPUT} characters.`,
      allowedMentions: { parse: [] },
    });
    return;
  }

  if ('sendTyping' in message.channel) {
    await message.channel.sendTyping().catch(() => undefined);
  }

  try {
    const upsert = await ctx.backend.upsertUser({
      discord_id: message.author.id,
      username: message.author.username,
      global_name: message.author.globalName ?? null,
    });

    // An open session wins over the onboarding flag: answers are saved as soon as they are
    // picked, so the backend can call a half-finished interview "onboarded". The member is
    // still mid-interview and should be answered with their next question, not the assistant.
    const session = ctx.sessions.get(message.author.id);
    if (session) {
      await resumeInterview(message, ctx, session);
      ctx.logger.info('free-text resumed onboarding', {
        discordId: message.author.id,
        step: session.step,
      });
      return;
    }

    // Older backends may omit the flag; then the authoritative read is one GET.
    const flag = (upsert as { onboarding_complete?: boolean }).onboarding_complete;
    const onboarded = flag ?? (await ctx.backend.getUser(message.author.id)).onboarding_complete;
    if (!onboarded) {
      await startInterview(message, ctx, {
        education: upsert.user.education ?? [],
        name: upsert.user.name ?? '',
        lastPosition: upsert.user.last_position ?? null,
        experienceYears: upsert.user.experience_years ?? null,
      });
      ctx.logger.info('onboarding started from free text', { discordId: message.author.id });
      return;
    }

    // A bare "jobs" / "job" / "tenders" is a request for the member's own list, not a paid
    // search: it goes through the same matcher `/jobs` uses, so typing it can never answer
    // with an unrelated row the way a text search over the whole corpus would.
    const bare = text.toLowerCase().replace(/[.!?]+$/, '').trim();
    const browseKind =
      /^(jobs?|vacanc(?:y|ies))$/.test(bare) ? 'job' : /^tenders?$/.test(bare) ? 'tender' : null;
    if (browseKind) {
      const listing = await browseRows(ctx, message.author.id, browseKind);
      // No tabs while the reply is an interview step (see BrowseReply.hideTabs).
      const tabs = listing.hideTabs ? [] : [browseTabsRow()];
      await reply(message, ctx, {
        content: listing.content,
        components: [...tabs, ...(listing.components ?? [])],
        allowedMentions: { parse: [] },
      });
      ctx.logger.info('free-text browse', { discordId: message.author.id, kind: browseKind });
      return;
    }

    // A greeting is not a search. Opening the chat and typing "hi" used to spend an assistant
    // call and answer with nothing useful; now it answers with the two tabs, which is what a
    // member arriving without a question actually wants.
    const greeting =
      /^(hi+|hey+|hello+|yo|sup|start|menu|help|salaam|salam|asc|asalaamu\s*calaykum|good\s+(?:morning|afternoon|evening))[\s!.?]*$/i.test(
        bare,
      );
    if (greeting) {
      const me = await ctx.backend.getUser(message.author.id).catch(() => null);
      const named = me?.user.name ? `, ${me.user.name}` : '';
      await reply(message, ctx, {
        content:
          `**What would you like to see${named}?**\n` +
          'Tap a tab, or ask me anything — for example `nurse jobs in Mogadishu`.',
        components: [browseTabsRow()],
        allowedMentions: { parse: [] },
      });
      ctx.logger.info('free-text greeting menu', { discordId: message.author.id });
      return;
    }

    const answer = await ctx.backend.assistantQuery(message.author.id, text);
    const content = formatAssistantReply(answer, { appBaseUrl: ctx.appBaseUrl, maxRows: 3 });
    await reply(message, ctx, { content: clampContent(content), allowedMentions: { parse: [] } });
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
    await reply(message, ctx, {
      content: clampContent(failure.content),
      ...(failure.components ? { components: failure.components } : {}),
      allowedMentions: { parse: [] },
    })
      .catch(() => undefined);
  }
}
