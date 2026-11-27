/**
 * The Telegram front end, driven the way a member drives it: an update in, the Bot API calls out.
 *
 * The fake captures the `sendMessage` payloads, so the assertions are about what a member would
 * actually read and tap — not about internal method names.
 */
import assert from 'node:assert/strict';
import test from 'node:test';

import { TgBot } from '../src/tg/bot.js';
import { TelegramClient, type InlineKeyboard, type TgUpdate } from '../src/tg/client.js';
import { tgDigestText } from '../src/tg/digest.js';
import { TgSessions } from '../src/tg/prompts.js';

const logger = { debug() {}, info() {}, warn() {}, error() {} } as never;

interface Sent {
  method: string;
  chat_id?: number;
  text?: string;
  reply_markup?: { inline_keyboard: InlineKeyboard };
}

function harness() {
  const calls: Sent[] = [];
  const saved: Array<Record<string, unknown>> = [];
  const alerts: Array<{ enabled: boolean }> = [];
  const fetchImpl = (async (url: string, init?: RequestInit) => {
    const method = String(url).split('/').pop()!;
    const body = JSON.parse(String(init?.body ?? '{}')) as Sent;
    calls.push({ ...body, method });
    const result =
      method === 'getMe' ? { id: 1, username: 'jobify_bot' }
      : method === 'sendMessage' || method === 'editMessageText' ? { message_id: 10, chat: { id: body.chat_id, type: 'private' } }
      : true;
    return new Response(JSON.stringify({ ok: true, result }), { status: 200 });
  }) as unknown as typeof fetch;

  const backend = {
    async upsertUser() {
      return { user: {}, created: false };
    },
    async saveProfile(patch: Record<string, unknown>) {
      saved.push(patch);
      return { user: {} };
    },
    async jobsForUser() {
      return {
        rows: [
          {
            job: {
              _id: 'job1',
              title: 'WASH Officer',
              organization: 'UNICEF',
              location: 'Mogadishu',
              deadline: '2026-12-02T00:00:00Z',
              url: 'https://example.org/job1',
            },
            matchScore: 85,
          },
        ],
        meta: {},
      };
    },
    async getUser() {
      return {
        user: { company: null },
        credits: { remaining_this_period: 3, monthly_allowance: 5, balance: 2 },
        alerts: {},
        subscription_status: 'trial',
        onboarding_complete: true,
        alerts_enabled: true,
      };
    },
    async mintLink() {
      return { url: 'https://jobify.soohsoft.com/enter/abc', expires_at: '' };
    },
    async setAlerts(_id: string, enabled: boolean) {
      alerts.push({ enabled });
      return { alerts: {}, alerts_enabled: enabled };
    },
    async setCompany() {
      return { user: {}, company_set: true };
    },
    async deliveries() {
      return [];
    },
    async markSent(ids: string[]) {
      return ids.length;
    },
    async markFailed() {
      return { marked: 0, alertsPaused: 0 };
    },
    async refreshMatches() {
      return { subscribers: 0, queuedTotal: 0 };
    },
  } as never;

  const bot = new TelegramClient({ token: 'test-token', fetchImpl, perSecond: 100, perUserMs: 0 });
  const sessions = new TgSessions();
  const handler = new TgBot({
    bot,
    backend,
    sessions,
    logger,
    appBaseUrl: 'https://jobify.soohsoft.com',
    digestTz: 'Africa/Nairobi',
    digestHour: 8,
  });

  const message = (text: string): TgUpdate => ({
    update_id: 1,
    message: { message_id: 1, chat: { id: 555, type: 'private' }, from: { id: 555, is_bot: false, first_name: 'Dayib' }, text, date: 0 },
  });
  const tap = (data: string): TgUpdate => ({
    update_id: 2,
    callback_query: { id: 'q1', from: { id: 555, is_bot: false, first_name: 'Dayib' }, data, message: { message_id: 9, chat: { id: 555, type: 'private' }, date: 0 } },
  });

  return { calls, saved, alerts, handler, message, tap };
}

function lastText(calls: Sent[]): string {
  const sends = calls.filter((call) => call.method === 'sendMessage' || call.method === 'editMessageText');
  return sends[sends.length - 1]?.text ?? '';
}

function lastKeyboard(calls: Sent[]): InlineKeyboard {
  const sends = calls.filter((call) => call.method === 'sendMessage' || call.method === 'editMessageText');
  return sends[sends.length - 1]?.reply_markup?.inline_keyboard ?? [];
}

const flat = (keyboard: InlineKeyboard): string[] => keyboard.flat().map((button) => button.callback_data ?? button.text);

test('/start opens the interview and the name step asks for a typed answer', async () => {
  const { calls, handler, message } = harness();
  await handler.handle(message('/start'));
  assert.match(lastText(calls), /What is your name/);
  assert.equal(lastKeyboard(calls).length, 0, 'no keyboard on the name step');
});

test('the full interview saves name, degrees, position, employer types and sectors', async () => {
  const { calls, saved, handler, message, tap } = harness();

  await handler.handle(message('/start'));
  await handler.handle(message('Dayib Ahmed'));

  await handler.handle(tap('onb:lvl:bachelor'));
  assert.match(lastText(calls), /Which field is your Bachelor/);
  await handler.handle(tap('onb:fld:health_medicine'));
  assert.match(lastText(calls), /Do you have a Master's degree/);
  await handler.handle(tap('onb:master:no'));

  assert.match(lastText(calls), /What job are you looking for/);
  await handler.handle(message('WASH Officer'));

  await handler.handle(tap('onb:yrs:3'));
  assert.match(lastText(calls), /Where would you like to work/);

  await handler.handle(tap('onb:emp:ngo'));
  assert.ok(
    lastKeyboard(calls).flat().some((button) => button.text === '✓ Non-Profit Organisations'),
    'the toggle shows the pick',
  );
  await handler.handle(tap('onb:emp:government'));
  await handler.handle(tap('onb:emp:done'));

  assert.match(lastText(calls), /Which sectors are you interested to get notified about/);
  await handler.handle(tap('onb:pos:wash'));
  await handler.handle(tap('onb:pos:ict'));
  await handler.handle(tap('onb:pos:done'));

  const final = saved[saved.length - 1];
  assert.equal(final.name, 'Dayib Ahmed');
  assert.deepEqual(final.employer_types, ['ngo', 'government']);
  assert.deepEqual(final.positions, ['wash', 'ict']);
  assert.equal(final.experience_years, 3);
  assert.equal(final.last_position, 'WASH Officer');
  assert.deepEqual(final.education, [{ level: 'bachelor', field: 'Health and Medicine' }]);

  assert.match(lastText(calls), /Setup complete/);
  assert.match(lastText(calls), /Never miss a job/);
  assert.deepEqual(flat(lastKeyboard(calls)), ['browse:job', 'browse:tender'], 'no account button on the tabs');
});

test('Bachelor asks for a Master’s, and Yes collects the Master’s field', async () => {
  const { saved, handler, message, tap } = harness();
  await handler.handle(message('/start'));
  await handler.handle(message('Amina'));
  await handler.handle(tap('onb:lvl:bachelor'));
  await handler.handle(tap('onb:fld:law'));
  await handler.handle(tap('onb:master:yes'));
  await handler.handle(tap('onb:fld:economics'));
  await handler.handle(tap('onb:another:no'));
  await handler.handle(message('Legal Officer'));
  await handler.handle(tap('onb:yrs:1'));
  await handler.handle(tap('onb:emp:private'));
  await handler.handle(tap('onb:emp:done'));
  await handler.handle(tap('onb:pos:legal_and_regulatory_services'));
  await handler.handle(tap('onb:pos:done'));

  const final = saved[saved.length - 1];
  assert.deepEqual(final.education, [
    { level: 'bachelor', field: 'Law' },
    { level: 'master', field: 'Economics' },
  ]);
});

test('the sector picker pages through 28 sectors and keeps picks across pages', async () => {
  const { calls, handler, message, tap } = harness();
  await handler.handle(message('/start'));
  await handler.handle(message('Yusuf'));
  await handler.handle(tap('onb:lvl:diploma'));
  await handler.handle(tap('onb:another:no'));
  await handler.handle(message('Driver'));
  await handler.handle(tap('onb:yrs:6'));
  await handler.handle(tap('onb:emp:done'));

  assert.match(lastText(calls), /Which sectors/);
  assert.ok(flat(lastKeyboard(calls)).includes('onb:posp:1'), 'More pages exist');
  await handler.handle(tap('onb:pos:health'));
  await handler.handle(tap('onb:posp:1'));
  assert.ok(flat(lastKeyboard(calls)).includes('onb:pos:wash'));
  await handler.handle(tap('onb:pos:wash'));
  assert.match(lastText(calls), /Picked: Health, WASH/);
});

test('/jobs lists matches with a link back to the job', async () => {
  const { calls, handler, message } = harness();
  await handler.handle(message('/jobs'));
  const text = lastText(calls);
  assert.match(text, /My Jobs/);
  assert.match(text, /WASH Officer/);
  assert.match(text, /UNICEF/);
  assert.match(text, /href="https:\/\/example.org\/job1"/);
});

test('/balance shows searches left and offers My Account', async () => {
  const { calls, handler, message } = harness();
  await handler.handle(message('/balance'));
  assert.match(lastText(calls), /3 of 5/);
  assert.deepEqual(flat(lastKeyboard(calls)), ['account:link']);
});

test('/account sends a one-time link to the portal', async () => {
  const { calls, handler, message } = harness();
  await handler.handle(message('/account'));
  assert.match(lastText(calls), /jobify\.soohsoft\.com\/enter\/abc/);
});

test('/stop turns alerts off', async () => {
  const { alerts, handler, message } = harness();
  await handler.handle(message('/stop'));
  assert.deepEqual(alerts, [{ enabled: false }]);
});

test('/tenders goes straight to the company name question, with no Start button', async () => {
  const { calls, handler, message } = harness();
  await handler.handle(message('/tenders'));
  assert.match(lastText(calls), /What is the company name/);
  assert.equal(lastKeyboard(calls).length, 0, 'a typed answer, not a button');
});

test('the digest footer sells the $1 plan but never replaces the jobs', async () => {
  const text = tgDigestText(
    [
      {
        notificationId: 'n1',
        userId: '555',
        title: 'WASH Officer',
        body: '',
        url: 'https://example.org/job1',
        matchScore: 85,
        job: {
          _id: 'j1',
          title: 'WASH Officer',
          organization: 'UNICEF',
          location: 'Mogadishu',
          deadline: '2026-12-02T00:00:00Z',
        },
      },
    ],
    1,
  );
  assert.match(text, /1 new job for you/);
  assert.match(text, /WASH Officer/);
  assert.match(text, /closes 2026-12-02/);
  assert.ok(text.indexOf('Never miss a job') > text.indexOf('WASH Officer'), 'jobs first, offer last');
  assert.match(text, /\/balance/);
  assert.doesNotMatch(text, /My Account/, 'the link is only offered by /balance');
});

test('an expired button tells the member to restart instead of silently doing nothing', async () => {
  const { calls, handler, tap } = harness();
  await handler.handle(tap('onb:lvl:bachelor'));
  assert.match(lastText(calls), /expired/i);
  assert.match(lastText(calls), /\/start/);
});
