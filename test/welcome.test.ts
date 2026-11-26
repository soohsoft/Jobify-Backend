import assert from 'node:assert/strict';
import { test } from 'node:test';

import { RequestGuard, type BotContext, type ChannelPayload, type MessagePayload } from '../src/context.js';
import type { Backend } from '../src/http.js';
import type { Logger } from '../src/log.js';
import { CID, OnboardingSessions } from '../src/onboarding.js';
import type { DiscordUser, UserEnvelope } from '../src/types.js';
import { handleMemberJoin, type JoinMember } from '../src/welcome.js';

const MEMBER_ID = '900000000000000711';
const FREE_CHANNEL = '1574842294778069086';

interface LogLine {
  level: string;
  msg: string;
  fields: Record<string, unknown>;
}

function captureLogger(): { logger: Logger; lines: LogLine[] } {
  const lines: LogLine[] = [];
  const record = (level: string) => (msg: string, fields?: Record<string, unknown>) => {
    lines.push({ level, msg, fields: fields ?? {} });
  };
  return {
    logger: { debug: record('debug'), info: record('info'), warn: record('warn'), error: record('error') },
    lines,
  };
}

const user: DiscordUser = {
  id: 'u7',
  name: 'Sahra',
  email: '',
  role: 'user',
  provider: 'discord',
  email_verified: true,
  location: null,
  match_profile: null,
  alerts: null,
  education_level: null,
  education: [{ level: 'bachelor', field: 'Health' }],
  last_position: 'Programme Officer',
  experience_years: 3,
  sectors: ['health'],
  skills: [],
  wants: null,
};

function envelope(onboarded: boolean): UserEnvelope {
  return {
    user,
    subscription_status: 'free',
    onboarding_complete: onboarded,
    alerts_enabled: true,
    alerts: { enabled: true, mode: 'daily', last_sent_at: '', user_disabled: false, excluded_employers: [], excluded_categories: [] },
    credits: { balance: 0, monthly_allowance: 30, used_this_period: 0, period_end: '', remaining_this_period: 30 },
  };
}

interface SetupOptions {
  onboarded?: boolean;
  /** undefined simulates an older backend that omits the flag on the upsert. */
  upsertFlag?: boolean;
  dmFails?: boolean;
  freeChannelId?: string;
  sendChannelFails?: boolean;
}

function setup(options: SetupOptions = {}): {
  ctx: BotContext;
  sessions: OnboardingSessions;
  dms: Array<{ discordId: string; payload: string | MessagePayload }>;
  channelPosts: Array<{ channelId: string; payload: ChannelPayload }>;
  calls: { upsert: number; getUser: number };
  lines: LogLine[];
} {
  const dms: Array<{ discordId: string; payload: string | MessagePayload }> = [];
  const channelPosts: Array<{ channelId: string; payload: ChannelPayload }> = [];
  const calls = { upsert: 0, getUser: 0 };
  const { logger, lines } = captureLogger();
  const onboarded = options.onboarded ?? false;

  const backend = {
    upsertUser: async () => {
      calls.upsert += 1;
      return {
        user,
        created: true,
        onboarding_complete: 'upsertFlag' in options ? options.upsertFlag : onboarded,
      };
    },
    getUser: async () => {
      calls.getUser += 1;
      return envelope(onboarded);
    },
  };

  const ctx: BotContext = {
    backend: backend as unknown as Backend,
    appBaseUrl: 'https://app.jobify.test',
    digestTz: 'Africa/Nairobi',
    digestHour: 8,
    logger,
    sessions: new OnboardingSessions(),
    categories: async () => [],
    sendDm: async (discordId, payload) => {
      dms.push({ discordId, payload });
      if (options.dmFails) {
        const err = new Error('Cannot send messages to this user') as Error & { code: number };
        err.code = 50007;
        throw err;
      }
    },
    sendChannel: async (channelId, payload) => {
      channelPosts.push({ channelId, payload });
      if (options.sendChannelFails) throw new Error('Missing Access');
    },
    guard: new RequestGuard(),
    ...(options.freeChannelId ? { freeChannelId: options.freeChannelId } : {}),
  };

  return { ctx, sessions: ctx.sessions, dms, channelPosts, calls, lines };
}

function member(overrides: { id?: string; bot?: boolean; sendFails?: boolean } = {}): {
  member: JoinMember;
  dms: MessagePayload[];
} {
  const dms: MessagePayload[] = [];
  const fake = {
    id: overrides.id ?? MEMBER_ID,
    user: {
      id: overrides.id ?? MEMBER_ID,
      username: 'sahra',
      globalName: null,
      bot: overrides.bot ?? false,
    },
    displayName: 'sahra',
    send: async (payload: MessagePayload) => {
      dms.push(payload);
      if (overrides.sendFails) {
        const err = new Error('Cannot send messages to this user') as Error & { code: number };
        err.code = 50007;
        throw err;
      }
      return undefined;
    },
  };
  return { member: fake as unknown as JoinMember, dms };
}

function componentIds(payload: MessagePayload | undefined): string[] {
  return (payload?.components ?? []).flatMap((row) =>
    row.toJSON().components.map((component) => (component as { custom_id?: string }).custom_id ?? ''),
  );
}

function mentionCount(content: string, id: string): number {
  return content.split(`<@${id}>`).length - 1;
}

// ---------------------------------------------------------------------------
// A new member: account created, session started, interview in the DM
// ---------------------------------------------------------------------------

test('a new member is upserted, given a session and DM\'d the name step', async () => {
  const { ctx, sessions, calls, lines } = setup({ onboarded: false });
  const joined = member();

  const result = await handleMemberJoin(joined.member, ctx);

  assert.equal(result.path, 'dm');
  assert.equal(result.interview, true);
  assert.equal(calls.upsert, 1, 'the account must exist before the interview writes to it');
  assert.equal(joined.dms.length, 1);

  const payload = joined.dms[0] as MessagePayload;
  assert.ok(payload.content.includes('Welcome to Jobify'));
  assert.ok(payload.content.includes('daily'), 'the welcome must say matches arrive daily');
  assert.ok(payload.content.includes('Step 1 of 4'));
  assert.ok(componentIds(payload).includes(CID.nameOpen), 'the name button must be attached');

  const session = sessions.get(MEMBER_ID);
  assert.ok(session);
  assert.equal(session.step, 'name');
  assert.deepEqual(session.education, [], 'the run collects its own list, so nothing is duplicated');
  assert.deepEqual(session.previousEducation, [{ level: 'bachelor', field: 'Health' }], 'what is on file is carried for display');
  assert.equal(sessions.size, 1);

  const log = lines.find((line) => line.msg === 'member welcomed');
  assert.equal(log?.fields.path, 'dm');
});

test('an already onboarded member gets the plain welcome and no interview', async () => {
  const { ctx, sessions } = setup({ onboarded: true });
  const joined = member();

  const result = await handleMemberJoin(joined.member, ctx);

  assert.equal(result.path, 'dm');
  assert.equal(result.interview, false);
  assert.equal(sessions.size, 0, 'no session may be started for an onboarded member');

  const payload = joined.dms[0] as MessagePayload;
  assert.ok(payload.content.includes('Welcome to Jobify'));
  assert.ok(payload.content.includes('already set up'));
  assert.ok(!payload.content.includes('Step 1 of 4'), 'no interview copy');
  // Not the interview, but not a dead end either: the greeting carries the two tabs.
  const row = payload.components?.[0]?.toJSON().components as { label?: string }[];
  assert.deepEqual(
    row.map((button) => button.label),
    ['My Jobs', 'My Tenders'],
    'the plain welcome offers the two tabs, not the interview',
  );
});

test('an upsert that omits the flag falls back to GET /user for onboarding state', async () => {
  const { ctx, sessions, calls } = setup({ onboarded: true, upsertFlag: undefined });

  const result = await handleMemberJoin(member().member, ctx);

  assert.equal(calls.getUser, 1);
  assert.equal(result.interview, false);
  assert.equal(sessions.size, 0);
});

test('bots joining are ignored entirely', async () => {
  const { ctx, sessions, dms, calls } = setup({ onboarded: false });

  const result = await handleMemberJoin(member({ bot: true }).member, ctx);

  assert.equal(result.path, 'failed');
  assert.equal(calls.upsert, 0);
  assert.equal(dms.length, 0);
  assert.equal(sessions.size, 0);
});

// ---------------------------------------------------------------------------
// DMs closed: exactly one mention in the free channel
// ---------------------------------------------------------------------------

test('a closed DM falls back to one channel message with exactly one mention', async () => {
  const { ctx, sessions, channelPosts, lines } = setup({
    onboarded: false,
    freeChannelId: FREE_CHANNEL,
  });
  const joined = member({ sendFails: true });

  const result = await handleMemberJoin(joined.member, ctx);

  assert.equal(result.path, 'channel');
  assert.equal(joined.dms.length, 1, 'the DM is attempted exactly once');
  assert.equal(channelPosts.length, 1);
  assert.equal(channelPosts[0]?.channelId, FREE_CHANNEL);

  const payload = channelPosts[0]?.payload;
  assert.ok(payload);
  assert.equal(mentionCount(payload.content, MEMBER_ID), 1);
  assert.deepEqual(payload.allowedMentions, { users: [MEMBER_ID] }, 'only the join fallback pings');
  assert.ok(payload.content.includes('Welcome to Jobify'));
  assert.ok(payload.content.includes('open your DMs'), 'the fallback must say how to fix it');
  assert.equal(payload.components, undefined, 'buttons in a shared channel would be ambiguous');

  // The session survives the fallback, so the member's next channel message resumes step 1.
  assert.equal(sessions.get(MEMBER_ID)?.step, 'name');

  const log = lines.find((line) => line.msg === 'member welcomed');
  assert.equal(log?.fields.path, 'channel');
});

test('an onboarded member with a closed DM gets the plain channel welcome', async () => {
  const { ctx, channelPosts } = setup({
    onboarded: true,
    freeChannelId: FREE_CHANNEL,
  });
  const joined = member({ sendFails: true });

  const result = await handleMemberJoin(joined.member, ctx);

  assert.equal(result.path, 'channel');
  assert.equal(joined.dms.length, 1);
  assert.equal(channelPosts.length, 1);
  const content = channelPosts[0]?.payload.content ?? '';
  assert.equal(mentionCount(content, MEMBER_ID), 1);
  assert.ok(content.includes('already set up'));
  assert.ok(!content.includes('Step 1 of 4'));
});

test('with no free channel configured a closed DM is logged as failed, not thrown', async () => {
  const { ctx, lines } = setup({ onboarded: false });
  const joined = member({ sendFails: true });

  const result = await handleMemberJoin(joined.member, ctx);

  assert.equal(result.path, 'failed');
  assert.ok(lines.some((line) => line.msg === 'member welcome failed: no channel fallback configured'));
});

test('when both delivery paths fail the handler only logs', async () => {
  const { ctx, lines } = setup({
    onboarded: false,
    freeChannelId: FREE_CHANNEL,
    sendChannelFails: true,
  });
  const joined = member({ sendFails: true });

  const result = await handleMemberJoin(joined.member, ctx);

  assert.equal(result.path, 'failed');
  const log = lines.find((line) => line.msg === 'member welcome failed in both paths');
  assert.equal(log?.level, 'error');
  assert.equal(log?.fields.path, 'failed');
});
