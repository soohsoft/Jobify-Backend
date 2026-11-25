/**
 * Greeting a new member before they say anything.
 *
 * A join is the only moment the bot can speak to someone who has not yet written. The DM is
 * the product's real surface, so the interview starts there: the message carries the same
 * name prompt a free-text first contact builds, and the existing button handlers pick the
 * session up unchanged because it lives in the same in-memory store.
 *
 * DMs are frequently closed on Discord ("Cannot send messages to this user", code 50007).
 * Then the welcome goes once into the free-text channel with one explicit @mention and a hint
 * either to open DMs or to type there — where free text already starts the interview. The DM
 * is attempted exactly once; a retry loop would just hammer a permission Discord already
 * refused.
 */
import type { BotContext, MessagePayload } from './context.js';
import { errorFields } from './log.js';
import { namePrompt } from './onboarding.js';
import { clampContent } from './ui.js';

/** Discord's "you cannot DM this user" code. Logged, never matched for control flow alone. */
export const DM_CLOSED = 50007;

/** The minimum a guild member must expose here; discord.js `GuildMember` satisfies it. */
export interface JoinMember {
  id: string;
  user: { id: string; username: string; globalName?: string | null; bot?: boolean };
  displayName?: string | null;
  send(payload: MessagePayload): Promise<unknown>;
}

export interface WelcomeResult {
  path: 'dm' | 'channel' | 'failed';
  interview: boolean;
}

function nameFor(member: JoinMember): string | null {
  const global = member.user.globalName?.trim();
  if (global) return global;
  const username = member.user.username?.trim();
  return username && username.length > 0 ? username : null;
}

/** The three lines every new member gets: what this is, what arrives, what to do next. */
export function welcomeLines(name: string | null): string {
  const greeting = name ? `**Welcome to Jobify, ${name}.**` : '**Welcome to Jobify.**';
  return [
    greeting,
    'I find Somali job and tender openings and send the ones that match you straight to your DMs.',
    'Four quick questions set it up — then your matches arrive daily at 08:00 EAT.',
  ].join('\n');
}

/** Onboarded members already have a profile, so they get the greeting and nothing to fill in. */
export function plainWelcome(member: JoinMember): MessagePayload {
  return {
    content: clampContent(
      [
        welcomeLines(nameFor(member)),
        '',
        'Your profile is already set up, so I will keep sending your daily matches.',
        '`/account` opens your portal — payment, alerts and company details.',
      ].join('\n'),
    ),
  };
}

export function interviewWelcome(member: JoinMember): MessagePayload {
  const prompt = namePrompt();
  return {
    content: clampContent(`${welcomeLines(nameFor(member))}\n\n${prompt.content}`),
    components: prompt.components,
  };
}

export function channelFallbackMessage(member: JoinMember, interview: boolean): MessagePayload & { mention: string } {
  const mention = `<@${member.id}>`;
  const hint = interview
    ? 'I could not DM you — Discord returned 50007. Type your role and place right here (for example `nurse jobs in Mogadishu`) and we will start, or open your DMs (Server Settings → Privacy Settings) and message me there.'
    : 'I could not DM you — Discord returned 50007. Your profile is already set up; open your DMs (Server Settings → Privacy Settings) so the daily digest can reach you, or type a role and place here, e.g. `nurse jobs in Mogadishu`.';
  return {
    mention,
    content: clampContent(`${mention} ${welcomeLines(nameFor(member))}\n\n${hint}`),
  };
}

function isDmClosed(err: unknown): boolean {
  return (err as { code?: unknown } | null)?.code === DM_CLOSED;
}

/**
 * Upserts the account first so a profile exists before the interview writes to it, starts the
 * session only for a member who has not finished onboarding, and reports which delivery path
 * was used. Never throws: a join handler that throws is a silent loss of the only greeting the
 * member will get.
 */
export async function handleMemberJoin(member: JoinMember, ctx: BotContext): Promise<WelcomeResult> {
  if (member.user.bot) return { path: 'failed', interview: false };

  const discordId = member.user.id;
  let onboarded = true;
  let existing: { education: import('./types.js').EducationEntry[]; name: string; lastPosition: string | null; experienceYears: number | null } = {
    education: [],
    name: member.user.globalName ?? member.user.username,
    lastPosition: null,
    experienceYears: null,
  };
  try {
    const upsert = await ctx.backend.upsertUser({
      discord_id: discordId,
      username: member.user.username,
      global_name: member.user.globalName ?? null,
    });
    existing = {
      education: upsert.user.education ?? [],
      name: upsert.user.name ?? existing.name,
      lastPosition: upsert.user.last_position ?? null,
      experienceYears: upsert.user.experience_years ?? null,
    };
    // Older backends omit the flag on the upsert response; then the GET is authoritative.
    const flag = (upsert as { onboarding_complete?: boolean }).onboarding_complete;
    onboarded = flag ?? (await ctx.backend.getUser(discordId)).onboarding_complete;
  } catch (err) {
    // Without a known state, do not start an interview whose answers could not be saved.
    ctx.logger.warn('member join: account upsert failed, sending plain welcome', {
      discordId,
      ...errorFields(err),
    });
    onboarded = true;
  }

  const interview = !onboarded;
  if (interview) ctx.sessions.start(discordId, {
    previousEducation: existing.education,
    name: existing.name || undefined,
    lastPosition: existing.lastPosition ?? undefined,
    experienceYears: existing.experienceYears ?? undefined,
  });

  const payload = interview ? interviewWelcome(member) : plainWelcome(member);

  try {
    await member.send(payload);
    ctx.logger.info('member welcomed', { discordId, path: 'dm', interview });
    return { path: 'dm', interview };
  } catch (err) {
    ctx.logger.warn('welcome dm failed', {
      discordId,
      interview,
      dm_closed: isDmClosed(err),
      ...errorFields(err),
    });
  }

  if (!ctx.freeChannelId || !ctx.sendChannel) {
    ctx.logger.error('member welcome failed: no channel fallback configured', {
      discordId,
      path: 'failed',
    });
    return { path: 'failed', interview };
  }

  const fallback = channelFallbackMessage(member, interview);
  try {
    await ctx.sendChannel(ctx.freeChannelId, {
      content: fallback.content,
      allowedMentions: { users: [discordId] },
    });
    ctx.logger.info('member welcomed', {
      discordId,
      path: 'channel',
      interview,
      channelId: ctx.freeChannelId,
    });
    return { path: 'channel', interview };
  } catch (err) {
    ctx.logger.error('member welcome failed in both paths', {
      discordId,
      path: 'failed',
      interview,
      ...errorFields(err),
    });
    return { path: 'failed', interview };
  }
}
