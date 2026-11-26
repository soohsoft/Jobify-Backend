import assert from 'node:assert/strict';
import { test } from 'node:test';

import type { Message } from 'discord.js';

import { RequestGuard, type BotContext } from '../src/context.js';
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
  education?: DiscordUser['education'];
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
    education: options.education ?? [],
    last_position: null,
    experience_years: null,
    sectors: [],
    skills: [],
    wants: null,
  };
  const envelope: UserEnvelope = {
    user,
    subscription_status: 'free',
    onboarding_complete: options.storedOnboarded ?? false,
    alerts_enabled: true,
    alerts: { enabled: true, mode: 'daily', last_sent_at: '', user_disabled: false, excluded_employers: [], excluded_categories: [] },
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
    guard: new RequestGuard(),
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

test('a brand-new member DMing starts onboarding at the name step and makes zero assistant calls', async () => {
  const { ctx, calls, sessions } = setup({ onboarded: false });
  const { message, replies } = messageFor({ content: 'Find me a job' });

  await handleMessage(message, ctx, BOT_ID);

  assert.equal(calls.upsert, 1);
  assert.equal(calls.assistant, 0, 'onboarding must not spend a query');
  assert.equal(calls.getUser, 0, 'the upsert flag is authoritative when present');
  assert.equal(replies.length, 1);

  const content = replies[0]?.content ?? '';
  assert.ok(content.includes('Welcome to Jobify. Four quick questions and I will start sending you matches.'));
  assert.ok(content.includes('Step 1 of 4'));
  assert.ok(componentIds(replies[0]).includes(CID.nameOpen), 'the name button must be attached');

  const session = sessions.get(MEMBER);
  assert.ok(session);
  assert.equal(session.step, 'name');
  assert.equal(sessions.size, 1);
});

test('an onboarded member still gets exactly one assistant answer', async () => {
  const { ctx, calls, sessions } = setup({ onboarded: true });
  const { message, replies } = messageFor({ content: 'data analyst in Mogadishu' });

  await handleMessage(message, ctx, BOT_ID);

  assert.equal(calls.assistant, 1);
  assert.equal(replies.length, 1);
  // The answer is the owner's job block, not a sentence about the search.
  assert.ok((replies[0]?.content ?? '').startsWith('1. **Data Analyst**'), replies[0]?.content);
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
  // The answer is the owner's job block, not a sentence about the search.
  assert.ok((replies[0]?.content ?? '').startsWith('1. **Data Analyst**'), replies[0]?.content);
});

// ---------------------------------------------------------------------------
// Existing answers and resumability
// ---------------------------------------------------------------------------

test('answers already on file are carried into the session for display and prefill', async () => {
  const { ctx, sessions } = setup({
    onboarded: false,
    education: [{ level: 'bachelor', field: 'Health' }],
  });
  const { message, replies } = messageFor({ content: 'Find me a job' });

  await handleMessage(message, ctx, BOT_ID);

  const session = sessions.get(MEMBER);
  assert.ok(session);
  assert.deepEqual(session.education, [], 'a re-run collects its own list');
  assert.deepEqual(session.previousEducation, [{ level: 'bachelor', field: 'Health' }]);
  assert.ok((replies[0]?.content ?? '').includes('Step 1 of 4'), 'the run starts at the name step');
});

test('a member mid-interview is re-prompted at their current step, not sent to the assistant', async () => {
  const { ctx, calls, sessions } = setup({ onboarded: false });
  const first = messageFor({ content: 'Find me a job' });

  await handleMessage(first.message, ctx, BOT_ID);
  const session = sessions.get(MEMBER);
  assert.ok(session);

  // Stand in for answers already given: a bachelor whose field is the open question.
  session.step = 'education_field';
  session.pendingLevel = 'bachelor';

  const second = messageFor({ content: 'any jobs today?' });
  await handleMessage(second.message, ctx, BOT_ID);

  assert.equal(calls.assistant, 0, 'a mid-interview message must not be answered by the assistant');
  assert.equal(sessions.size, 1, 'no second session may be created');
  assert.equal(sessions.get(MEMBER), session, 'the same session object is reused');
  assert.equal(session.pendingLevel, 'bachelor', 'in-memory answers must survive the re-prompt');
  const content = second.replies[0]?.content ?? '';
  assert.ok(content.includes('Step 2 of 4'));
  assert.ok(content.includes('Bachelor'), 'the prompt names the level whose field is being asked');
  assert.ok(componentIds(second.replies[0]).includes(CID.field('health_medicine')));
  assert.ok(!componentIds(second.replies[0]).some((id) => id.startsWith('onb:sectors')));
});

test('a member at the job-history step is re-prompted for their last job', async () => {
  const { ctx, sessions } = setup({ onboarded: false });
  await handleMessage(messageFor({ content: 'hello' }).message, ctx, BOT_ID);

  const session = sessions.get(MEMBER);
  assert.ok(session);
  session.step = 'position';

  const second = messageFor({ content: 'still there?' });
  await handleMessage(second.message, ctx, BOT_ID);

  const content = second.replies[0]?.content ?? '';
  assert.ok(content.includes('Step 3 of 4'));
  assert.ok(componentIds(second.replies[0]).includes(CID.positionOpen));
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
  // The answer is the owner's job block, not a sentence about the search.
  assert.ok((replies[0]?.content ?? '').startsWith('1. **Data Analyst**'), replies[0]?.content);
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

test('an onboarded member who says hello gets the two tabs, not an assistant search', async () => {
  const { ctx, calls } = setup({ onboarded: true, storedOnboarded: true });

  const sent = messageFor({ content: 'hi' });
  await handleMessage(sent.message, ctx, BOT_ID);

  assert.equal(calls.assistant, 0, 'a greeting must not spend an assistant call');
  const reply = sent.replies[0];
  assert.ok(reply?.content.includes('What would you like to see'), reply?.content);
  // The tabs, so the visit ends in a list rather than in copy telling them to type a command.
  assert.deepEqual(
    componentIds(reply).slice(0, 2),
    ['onb:browse:job', 'onb:browse:tender'],
  );
});

test('a greeting from a member who is not set up still starts the interview', async () => {
  const { ctx, sessions } = setup({ onboarded: false });

  await handleMessage(messageFor({ content: 'hello' }).message, ctx, BOT_ID);

  assert.equal(sessions.size, 1, 'the interview, not a menu');
});
