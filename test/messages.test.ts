import assert from 'node:assert/strict';
import { test } from 'node:test';

import type { Message } from 'discord.js';

import type { BotContext } from '../src/context.js';
import type { Backend } from '../src/http.js';
import type { Logger } from '../src/log.js';
import { handleMessage } from '../src/messages.js';
import { CID, OnboardingSessions } from '../src/onboarding.js';
import type { AssistantAnswer, Category, DiscordUser, UserEnvelope } from '../src/types.js';

const BOT_ID = '1574835560315363418';
const APP = 'https://app.jobify.test';
const MEMBER = '900000000000000611';
const FREE_CHANNEL = '1574842294778069086';

const categories: Category[] = [
  { slug: 'ict', label: 'ICT', group: 'all_fields' },
  { slug: 'health', label: 'Health', group: 'all_fields' },
];

const silentLogger: Logger = {
  debug: () => undefined,
  info: () => undefined,
  warn: () => undefined,
  error: () => undefined,
};

const assistantAnswer: AssistantAnswer = {
  intent: 'search',
  reply: 'I found roles that match what you asked.',
  jobs: [
    { job_id: 'job-1', title: 'Data Analyst', organization: 'Horn Relief', location: 'Mogadishu', kind: 'job', deadline: null },
    { job_id: 'job-2', title: 'Network Engineer', organization: 'Hormuud', location: 'Mogadishu', kind: 'job', deadline: null },
    { job_id: 'job-3', title: 'IT Support', organization: 'UNDP', location: 'Mogadishu', kind: 'job', deadline: null },
    { job_id: 'job-4', title: 'Fourth Role', organization: 'NGO', location: null, kind: 'job', deadline: null },
  ],
  credits: { balance: 0, monthly_allowance: 30, used_this_period: 1, period_end: '2026-12-24T00:00:00Z', remaining_this_period: 29 },
};

interface Reply {
  content: string;
  components?: Array<{ toJSON(): { components: Array<{ custom_id?: string; options?: Array<{ value: string; default?: boolean }> }> } }>;
}

function messageFor(options: {
  content: string;
  guild?: string | null;
  channelId?: string;
  mentioned?: boolean;
  id?: string;
}): { message: Message; replies: Reply[] } {
  const replies: Reply[] = [];
  const mentions = new Map<string, unknown>();
  if (options.mentioned) mentions.set(BOT_ID, { id: BOT_ID });
  const channelId = options.channelId ?? 'dm-channel';

  const message = {
    author: {
      id: options.id ?? MEMBER,
      username: 'sahra',
      globalName: null,
      bot: false,
    },
    content: options.content,
    // `null` is a DM; a string is a guild id.
    guild: options.guild ? { id: options.guild } : null,
    channelId,
    mentions: { users: mentions },
    channel: { id: channelId, sendTyping: async () => undefined },
    reply: async (payload: Reply) => {
      replies.push(payload);
      return undefined;
    },
  };

  return { message: message as unknown as Message, replies };
}

interface BackendOptions {
  /** The flag `POST /internal/discord/users` reports; undefined simulates an older backend. */
  onboarded?: boolean;
  /** What `GET /internal/discord/user/{id}` would report. */
  storedOnboarded?: boolean;
  sectors?: string[];
  answer?: AssistantAnswer;
  freeChannelId?: string;
}

function setup(options: BackendOptions = {}): {
  ctx: BotContext;
  calls: { upsert: number; assistant: number; getUser: number };
  sessions: OnboardingSessions;
} {
  const calls = { upsert: 0, assistant: 0, getUser: 0 };
  const user: DiscordUser = {
    id: 'u1',
    name: 'Sahra',
    email: '',
    role: 'user',
    provider: 'discord',
    email_verified: true,
    location: null,
    match_profile: null,
    alerts: null,
    education_level: null,
    experience_years: null,
    sectors: options.sectors ?? [],
    skills: [],
    wants: null,
  };
  const envelope: UserEnvelope = {
    user,
    subscription_status: 'free',
    onboarding_complete: options.storedOnboarded ?? false,
    alerts_enabled: true,
    alerts: { enabled: true, mode: 'daily', last_sent_at: '', excluded_employers: [], excluded_categories: [] },
    credits: { balance: 0, monthly_allowance: 30, used_this_period: 0, period_end: '', remaining_this_period: 30 },
  };

  const backend = {
    upsertUser: async () => {
      calls.upsert += 1;
      return {
        user,
        created: true,
        // Property present with `undefined` is exactly the "flag absent" shape.
        onboarding_complete: options.onboarded,
      };
    },
    getUser: async () => {
      calls.getUser += 1;
      return envelope;
    },
    assistantQuery: async () => {
      calls.assistant += 1;
      return options.answer ?? assistantAnswer;
    },
  };

  const ctx: BotContext = {
    backend: backend as unknown as Backend,
    appBaseUrl: APP,
    digestTz: 'Africa/Nairobi',
    digestHour: 8,
    logger: silentLogger,
    sessions: new OnboardingSessions(),
    categories: async () => categories,
    sendDm: async () => undefined,
    ...(options.freeChannelId ? { freeChannelId: options.freeChannelId } : {}),
  };

  return { ctx, calls, sessions: ctx.sessions };
}

function componentIds(reply: Reply | undefined): string[] {
  return (reply?.components ?? []).flatMap((row) =>
    row.toJSON().components.map((component) => component.custom_id ?? ''),
  );
}

// ---------------------------------------------------------------------------
// First contact
// ---------------------------------------------------------------------------

test('a brand-new member DMing starts onboarding and makes zero assistant calls', async () => {
  const { ctx, calls, sessions } = setup({ onboarded: false });
  const { message, replies } = messageFor({ content: 'Find me a job' });

  await handleMessage(message, ctx, BOT_ID);

  assert.equal(calls.upsert, 1);
  assert.equal(calls.assistant, 0, 'onboarding must not spend a query');
  assert.equal(calls.getUser, 0, 'the upsert flag is authoritative when present');
  assert.equal(replies.length, 1);

  const content = replies[0]?.content ?? '';
  assert.ok(content.includes('Welcome to Jobify. Three quick questions and I will start sending you matches.'));
  assert.ok(content.includes('Step 1 of 6'));
  assert.ok(componentIds(replies[0]).includes(CID.education('bachelor')), 'the education buttons must be attached');

  const session = sessions.get(MEMBER);
  assert.ok(session);
  assert.equal(session.step, 'education');
  assert.equal(sessions.size, 1);
});

test('an onboarded member still gets exactly one assistant answer', async () => {
  const { ctx, calls, sessions } = setup({ onboarded: true });
  const { message, replies } = messageFor({ content: 'data analyst in Mogadishu' });

  await handleMessage(message, ctx, BOT_ID);

  assert.equal(calls.assistant, 1);
  assert.equal(replies.length, 1);
  assert.ok((replies[0]?.content ?? '').startsWith(assistantAnswer.reply));
  // Exactly the three-row ceiling, even though the assistant returned four.
  assert.equal((replies[0]?.content ?? '').match(/\/jobs\/job-/g)?.length, 3);
  assert.equal(sessions.get(MEMBER), undefined);
});

test('when the upsert omits the flag, onboarding state comes from GET /user', async () => {
  const { ctx, calls } = setup({ onboarded: undefined, storedOnboarded: true });
  const { message, replies } = messageFor({ content: 'Find me a tender' });

  await handleMessage(message, ctx, BOT_ID);

  assert.equal(calls.getUser, 1);
  assert.equal(calls.assistant, 1);
  assert.ok((replies[0]?.content ?? '').startsWith(assistantAnswer.reply));
});

// ---------------------------------------------------------------------------
// Existing answers and idempotence
// ---------------------------------------------------------------------------

test('sectors already on file are carried into the new interview', async () => {
  const { ctx, sessions } = setup({ onboarded: false, sectors: ['ict'] });
  const { message } = messageFor({ content: 'Find me a job' });

  await handleMessage(message, ctx, BOT_ID);

  const session = sessions.get(MEMBER);
  assert.ok(session);
  assert.deepEqual(session.sectors, ['ict']);
});

test('a member mid-interview is re-prompted at their current step, not sent to the assistant', async () => {
  const { ctx, calls, sessions } = setup({ onboarded: false });
  const first = messageFor({ content: 'Find me a job' });

  await handleMessage(first.message, ctx, BOT_ID);
  const session = sessions.get(MEMBER);
  assert.ok(session);

  // Stand in for answers already given: education is saved server-side and in the session.
  session.education = 'bachelor';
  session.step = 'location';

  const second = messageFor({ content: 'any jobs today?' });
  await handleMessage(second.message, ctx, BOT_ID);

  assert.equal(calls.assistant, 0, 'a mid-interview message must not be answered by the assistant');
  assert.equal(sessions.size, 1, 'no second session may be created');
  assert.equal(sessions.get(MEMBER), session, 'the same session object is reused');
  assert.equal(session.education, 'bachelor', 'in-memory answers must survive the re-prompt');
  assert.ok((second.replies[0]?.content ?? '').includes('Step 4 of 6'));
  assert.ok(componentIds(second.replies[0]).includes(CID.locationOpen));
});

test('stored sectors are pre-selected when a mid-interview member is re-prompted at the sector step', async () => {
  const { ctx, calls, sessions } = setup({ onboarded: false, sectors: ['ict'] });
  const first = messageFor({ content: 'Find me a job' });
  await handleMessage(first.message, ctx, BOT_ID);

  const session = sessions.get(MEMBER);
  assert.ok(session);
  session.step = 'sectors';

  const second = messageFor({ content: 'hello again' });
  await handleMessage(second.message, ctx, BOT_ID);

  assert.equal(calls.assistant, 0);
  const menu = second.replies[0]?.components?.[0]?.toJSON().components[0];
  assert.ok(menu?.options);
  assert.deepEqual(
    menu.options.filter((option) => option.default).map((option) => option.value),
    ['ict'],
  );
});

// ---------------------------------------------------------------------------
// The optional free-text channel
// ---------------------------------------------------------------------------

test('a message in the configured free-text channel is handled without a mention', async () => {
  const { ctx, calls } = setup({ onboarded: false, freeChannelId: FREE_CHANNEL });
  const { message, replies } = messageFor({
    content: 'Find me a job',
    guild: 'guild-1',
    channelId: FREE_CHANNEL,
    mentioned: false,
  });

  await handleMessage(message, ctx, BOT_ID);

  assert.equal(calls.upsert, 1);
  assert.equal(calls.assistant, 0);
  assert.ok((replies[0]?.content ?? '').includes('Welcome to Jobify'));
});

test('an onboarded member in the free-text channel reaches the assistant', async () => {
  const { ctx, calls } = setup({ onboarded: true, freeChannelId: FREE_CHANNEL });
  const { message, replies } = messageFor({
    content: 'monitoring and evaluation jobs',
    guild: 'guild-1',
    channelId: FREE_CHANNEL,
    mentioned: false,
  });

  await handleMessage(message, ctx, BOT_ID);

  assert.equal(calls.assistant, 1);
  assert.ok((replies[0]?.content ?? '').startsWith(assistantAnswer.reply));
});

test('guild messages in any other channel are ignored without a mention', async () => {
  const { ctx, calls } = setup({ onboarded: false, freeChannelId: FREE_CHANNEL });
  const { message, replies } = messageFor({
    content: 'Find me a job',
    guild: 'guild-1',
    channelId: '1574842294778069999',
    mentioned: false,
  });

  await handleMessage(message, ctx, BOT_ID);

  assert.equal(calls.upsert, 0);
  assert.equal(calls.assistant, 0);
  assert.equal(replies.length, 0);
});

test('with no free-text channel set, every guild message still needs a mention', async () => {
  const { ctx, calls } = setup({ onboarded: false });
  const { message, replies } = messageFor({
    content: 'Find me a job',
    guild: 'guild-1',
    channelId: FREE_CHANNEL,
    mentioned: false,
  });

  await handleMessage(message, ctx, BOT_ID);

  assert.equal(calls.upsert, 0);
  assert.equal(replies.length, 0);
});
