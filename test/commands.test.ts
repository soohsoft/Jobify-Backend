/**
 * The command surface itself: exactly four commands, and the old names handled politely.
 *
 * The owner's rule is a product rule, not a Discord detail — a member who sees nine commands
 * sees nine things to learn. These tests pin the list AND pin that `/balance` is the `/credits`
 * panel under its new name, so a future rename cannot quietly drop the panel or its button.
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';

import { buildCommands, handleCommand } from '../src/commands.js';
import { RequestGuard, type BotContext } from '../src/context.js';
import type { Backend } from '../src/http.js';
import type { Logger } from '../src/log.js';
import { OnboardingSessions } from '../src/onboarding.js';
import type { DiscordUser, UserEnvelope } from '../src/types.js';

const MEMBER = '900000000000000931';
const APP = 'https://app.jobify.test';

const silentLogger: Logger = {
  debug: () => undefined,
  info: () => undefined,
  warn: () => undefined,
  error: () => undefined,
};

interface Reply {
  content: string;
  flags?: number;
  components?: Array<{ toJSON(): { components: Array<{ custom_id?: string; label?: string }> } }>;
}

function interactionFor(commandName: string): {
  interaction: Parameters<typeof handleCommand>[0];
  replies: Reply[];
  deferred: { flags?: number }[];
} {
  const replies: Reply[] = [];
  const deferred: { flags?: number }[] = [];
  const interaction = {
    commandName,
    user: { id: MEMBER, username: 'sahra', globalName: null },
    deferred: false,
    replied: false,
    options: { getString: () => null },
    deferReply: async (payload: { flags?: number } = {}) => {
      deferred.push(payload);
      (interaction as { deferred: boolean }).deferred = true;
      return undefined;
    },
    editReply: async (payload: Reply) => {
      replies.push(payload);
      return undefined;
    },
    reply: async (payload: Reply) => {
      replies.push(payload);
      (interaction as { replied: boolean }).replied = true;
      return undefined;
    },
  };
  return { interaction: interaction as unknown as Parameters<typeof handleCommand>[0], replies, deferred };
}

function ctxFor(): { ctx: BotContext; calls: { assistant: number } } {
  const calls = { assistant: 0 };
  const user: DiscordUser = {
    id: 'u1',
    name: 'Sahra Ali',
    email: '',
    role: 'user',
    provider: 'discord',
    email_verified: true,
    location: null,
    match_profile: null,
    alerts: null,
    education_level: null,
    education: [{ level: 'bachelor', field: 'Health' }],
    last_position: 'Nurse',
    experience_years: 3,
    sectors: [],
    skills: [],
    wants: null,
  };
  const envelope: UserEnvelope = {
    user,
    subscription_status: 'free',
    onboarding_complete: true,
    alerts_enabled: false,
    alerts: {
      enabled: false,
      mode: '',
      last_sent_at: '',
      user_disabled: false,
      excluded_employers: [],
      excluded_categories: [],
    },
    credits: {
      balance: 2,
      monthly_allowance: 30,
      used_this_period: 7,
      period_end: '2026-12-24T00:00:00Z',
      remaining_this_period: 23,
    },
  };
  const backend = {
    upsertUser: async () => ({ user, onboarding_complete: true, alerts_enabled: false }),
    getUser: async () => envelope,
    assistantQuery: async () => {
      calls.assistant += 1;
      throw new Error('a slash command must not call the assistant without a query');
    },
  };
  const ctx = {
    backend: backend as unknown as Backend,
    appBaseUrl: APP,
    digestTz: 'Africa/Nairobi',
    digestHour: 8,
    logger: silentLogger,
    sessions: new OnboardingSessions(),
    categories: { get: async () => [] },
    sendDm: async () => undefined,
    guard: new RequestGuard(),
  } as unknown as BotContext;
  return { ctx, calls };
}

// ---------------------------------------------------------------------------
// The list
// ---------------------------------------------------------------------------

test('exactly four commands are registered, in product order', () => {
  const commands = buildCommands();
  assert.deepEqual(
    commands.map((command) => command.name),
    ['jobs', 'tenders', 'balance', 'account'],
  );
});

test('the removed commands are gone from the registration payload', () => {
  const names = buildCommands().map((command) => command.name);
  for (const gone of ['start', 'alerts', 'company', 'help', 'stop', 'credits']) {
    assert.ok(!names.includes(gone), `${gone} is still registered`);
  }
});

test('the kept descriptions are the ones the owner approved', () => {
  const byName = new Map(buildCommands().map((command) => [command.name, command]));
  assert.equal(byName.get('jobs')?.description, 'Search jobs, or list the newest in your fields');
  assert.equal(byName.get('tenders')?.description, 'Search tenders, or list the newest in your fields');
  assert.equal(byName.get('account')?.description, 'Get a private one-time link to your Jobify account');
  // `/balance` IS `/credits`: the name changed, the sentence did not.
  assert.equal(byName.get('balance')?.description, 'How many searches you have left, and how to buy more');
});

test('jobs and tenders keep their optional query; balance and account take no options', () => {
  const byName = new Map(buildCommands().map((command) => [command.name, command]));
  for (const name of ['jobs', 'tenders']) {
    const options = byName.get(name)?.options ?? [];
    assert.equal(options.length, 1, `${name} has ${options.length} options`);
    assert.equal(options[0]?.name, 'query');
    assert.equal(options[0]?.required, false);
  }
  assert.deepEqual(byName.get('balance')?.options ?? [], []);
  assert.deepEqual(byName.get('account')?.options ?? [], []);
});

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

test('/balance answers with the credits panel and its Buy credits button', async () => {
  const { ctx, calls } = ctxFor();
  const { interaction, replies, deferred } = interactionFor('balance');
  await handleCommand(interaction, ctx);

  assert.equal(calls.assistant, 0);
  assert.equal(deferred.length, 1, 'deferred exactly once');
  const reply = replies.at(-1);
  assert.ok(reply, 'a reply was sent');
  assert.match(reply.content, /Free searches left this period: \*\*23 of 30\*\*/);
  assert.match(reply.content, /Purchased searches: \*\*2\*\*/);
  const ids = (reply.components ?? []).flatMap((row) => row.toJSON().components.map((button) => button.custom_id));
  assert.deepEqual(ids, ['credits:buy'], 'the buy button keeps its custom id');
});

test('a cached /credits answers with the surviving commands instead of failing', async () => {
  const { ctx } = ctxFor();
  const { interaction, replies } = interactionFor('credits');
  await handleCommand(interaction, ctx);

  const reply = replies.at(-1);
  assert.ok(reply);
  for (const name of ['/jobs', '/tenders', '/balance', '/account']) {
    assert.ok(reply.content.includes(name), `the answer names ${name}`);
  }
});
