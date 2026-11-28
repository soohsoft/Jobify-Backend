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

function harness(options: { onboardingComplete?: boolean } = {}) {
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
    async jobsForUser(_id: string, options: { threshold?: number } = {}) {
      const matched = {
        job: {
          _id: 'job1',
          title: 'WASH Officer',
          organization: 'UNICEF',
          location: 'Mogadishu',
          deadline: '2026-12-02T00:00:00Z',
          url: 'https://example.org/job1',
        },
        matchScore: 85,
      };
      const related = {
        job: {
          _id: 'job2',
          title: 'Finance Officer',
          organization: 'Islamic Relief',
          location: 'Mogadishu',
          deadline: '2026-12-09T00:00:00Z',
          url: 'https://example.org/job2',
        },
        matchScore: 45,
      };
      // The real backend returns only rows over the cut-off unless `threshold: 0` asks for
      // everything the member's areas admit — that second read is where related rows come from.
      return { rows: options.threshold === 0 ? [matched, related] : [matched], meta: {} };
    },
    async getUser() {
      return {
        user: {
          company: null,
          match_profile: {
            categories: ['business_and_finance'],
            keywords: [],
            locations: [],
            category_source: 'llm',
            updated_at: '',
            positions: ['health'],
            employer_types: ['ngo'],
          },
        },
        credits: { remaining_this_period: 3, monthly_allowance: 5, balance: 2 },
        alerts: {},
        subscription_status: 'trial',
        onboarding_complete: options.onboardingComplete ?? true,
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

  assert.match(lastText(calls), /List the last three positions you worked/);
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

  assert.match(lastText(calls), /Which other sectors are you interested to get notified about/);
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

  assert.match(lastText(calls), /Which other sectors/);
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
  assert.match(text, /Matched Jobs/);
  assert.match(text, /WASH Officer/);
  assert.match(text, /UNICEF/);
  assert.match(text, /href="https:\/\/example.org\/job1"/);
});

test('My Jobs shows matched rows and, separately, related positions', async () => {
  const { calls, handler, message } = harness();
  await handler.handle(message('/jobs'));
  const text = lastText(calls);
  assert.match(text, /Matched Jobs/, 'the first frame is the matched list');
  assert.match(text, /Other Sectors Selected/, 'rows admitted by the picked sectors get their own frame');
  const matchedAt = text.indexOf('Matched Jobs');
  const relatedAt = text.indexOf('Other Sectors Selected');
  assert.ok(matchedAt < relatedAt, 'matched comes first');
  assert.ok(text.indexOf('WASH Officer') < relatedAt, 'the matched row sits in the first frame');
  assert.ok(text.indexOf('Finance Officer') > relatedAt, 'the related row sits in the second');
});

test('/sectors reopens the picker with the stored picks and saves only that answer', async () => {
  const { calls, saved, handler, message, tap } = harness();
  await handler.handle(message('/sectors'));
  assert.match(lastText(calls), /Which other sectors/);
  // The picker starts from what is stored, not from an empty list.
  assert.ok(
    lastKeyboard(calls)
      .flat()
      .some((button) => (button.text ?? '').startsWith('✓ ')),
    'the stored sector comes back ticked',
  );
  // Adding a sector on top of the stored one: the picker is a toggle, so tapping the stored
  // sector would switch it OFF, and `Done` refuses an empty list on purpose.
  await handler.handle(tap('onb:pos:wash'));
  await handler.handle(tap('onb:pos:done'));
  const written = saved[saved.length - 1] as { positions?: string[]; name?: unknown };
  assert.deepEqual(written.positions, ['health', 'wash']);
  assert.equal(written.name, undefined, 'an edit must not rewrite the whole profile');
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

test('My Jobs from an unfinished profile opens the interview, not an empty list', async () => {
  const { calls, handler, tap } = harness({ onboardingComplete: false });
  await handler.handle(tap('browse:job'));
  assert.match(lastText(calls), /Step 1 of 5 — What is your name/);
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
          source_name: 'UNICEF',
          deadline: '2026-12-02T00:00:00Z',
        },
      },
    ],
    1,
  );
  assert.match(text, /1 new job for you/);
  assert.match(text, /WASH Officer/);
  assert.match(text, /Closing Date: 2 Dec 2026/);
  assert.doesNotMatch(text, /Posted By/);
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
