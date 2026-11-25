import assert from 'node:assert/strict';
import { test } from 'node:test';

import { Backend, BackendError, normalizeFeedPage } from '../src/http.js';

const KEY = 'test-internal-key';

interface Call {
  url: string;
  method: string;
  headers: Record<string, string>;
  body: unknown;
}

/** A fetch stub: records what the client built and answers from a queue. */
function stubFetch(responses: Array<() => Response | Promise<Response>>): { calls: Call[]; fetchImpl: typeof fetch } {
  const calls: Call[] = [];
  const fetchImpl = (async (input: string | URL | Request, init?: RequestInit) => {
    calls.push({
      url: String(input),
      method: init?.method ?? 'GET',
      headers: (init?.headers ?? {}) as Record<string, string>,
      body: init?.body ? JSON.parse(String(init.body)) : undefined,
    });
    const next = responses.shift();
    if (!next) throw new Error('the test did not queue a response');
    return next();
  }) as unknown as typeof fetch;
  return { calls, fetchImpl };
}

function json(body: unknown, status = 200): () => Response {
  return () =>
    new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
}

const noSleep = async (): Promise<void> => undefined;

test('upsertUser posts the snowflake to the internal route with the key and no token', async () => {
  const { calls, fetchImpl } = stubFetch([
    json({ status: 'success', data: { user: { id: 'u1', name: 'Sahra' }, created: true, onboarding_complete: false } }),
  ]);
  const backend = new Backend({ baseUrl: 'http://backend.test', apiKey: KEY, fetchImpl, sleep: noSleep });

  const result = await backend.upsertUser({ discord_id: '900000000000000955', username: 'sahra', global_name: null });

  assert.equal(calls.length, 1);
  assert.equal(calls[0]?.url, 'http://backend.test/internal/discord/users');
  assert.equal(calls[0]?.method, 'POST');
  assert.equal(calls[0]?.headers['x-api-key'], KEY);
  assert.deepEqual(calls[0]?.body, { discord_id: '900000000000000955', username: 'sahra', global_name: null });
  assert.equal(result.created, true);
  // The envelope is unwrapped, so callers read fields, not `data.data`.
  assert.equal(result.user.name, 'Sahra');
});

test('an assistant answer is unwrapped into intent, reply, rows and credits', async () => {
  const { fetchImpl, calls } = stubFetch([
    json({
      status: 'success',
      data: {
        intent: 'search',
        reply: 'Two matches.',
        jobs: [
          {
            job_id: 'job-1',
            title: 'Data Analyst',
            organization: 'REACH',
            location: 'Mogadishu',
            kind: 'job',
            deadline: '2026-12-03',
          },
        ],
        credits: {
          balance: 2,
          monthly_allowance: 30,
          used_this_period: 4,
          period_end: '2026-12-24T00:00:00Z',
          remaining_this_period: 26,
        },
      },
      meta: { charged_queries: 1 },
    }),
  ]);
  const backend = new Backend({ baseUrl: 'http://backend.test', apiKey: KEY, fetchImpl, sleep: noSleep });

  const answer = await backend.assistantQuery('900000000000000955', 'data analyst');

  assert.equal(calls[0]?.url, 'http://backend.test/internal/assistant/query');
  assert.deepEqual(calls[0]?.body, { discord_id: '900000000000000955', message: 'data analyst' });
  assert.equal(answer.intent, 'search');
  assert.equal(answer.reply, 'Two matches.');
  assert.equal(answer.jobs.length, 1);
  assert.equal(answer.jobs[0]?.job_id, 'job-1');
  assert.equal(answer.credits.remaining_this_period, 26);
});

test('a 402 becomes a BackendError carrying out_of_credits, and is not retried', async () => {
  const { fetchImpl, calls } = stubFetch([
    json({ status: 'error', code: 'out_of_credits', message: 'No queries left' }, 402),
  ]);
  const backend = new Backend({ baseUrl: 'http://backend.test', apiKey: KEY, fetchImpl, sleep: noSleep });

  await assert.rejects(
    () => backend.assistantQuery('900000000000000955', 'anything'),
    (err: unknown) => {
      assert.ok(err instanceof BackendError);
      assert.equal(err.status, 402);
      assert.equal(err.code, 'out_of_credits');
      return true;
    },
  );
  // A 4xx is an answer, not a hiccup: exactly one attempt.
  assert.equal(calls.length, 1);
});

test('a 5xx is retried with backoff, then succeeds', async () => {
  const sleeps: number[] = [];
  const { fetchImpl, calls } = stubFetch([
    json({ status: 'error', code: 'internal_error', message: 'boom' }, 500),
    json({ status: 'success', data: { user: { id: 'u1' }, subscription_status: 'none', onboarding_complete: true, alerts_enabled: false, alerts: { enabled: false, mode: '', last_sent_at: '', excluded_employers: [], excluded_categories: [] }, credits: { balance: 0, monthly_allowance: 30, used_this_period: 0, period_end: '', remaining_this_period: 30 } } }),
  ]);
  const backend = new Backend({
    baseUrl: 'http://backend.test',
    apiKey: KEY,
    fetchImpl,
    sleep: async (ms) => {
      sleeps.push(ms);
    },
  });

  const envelope = await backend.getUser('900000000000000955');
  assert.equal(calls.length, 2);
  assert.deepEqual(sleeps, [400]);
  assert.equal(envelope.onboarding_complete, true);
});

test('a network failure is retried and then reported as status 0', async () => {
  const fetchImpl = (async () => {
    throw new TypeError('fetch failed');
  }) as unknown as typeof fetch;
  const backend = new Backend({
    baseUrl: 'http://backend.test',
    apiKey: KEY,
    fetchImpl,
    sleep: noSleep,
    attempts: 2,
  });

  await assert.rejects(
    () => backend.getUser('1'),
    (err: unknown) => err instanceof BackendError && err.status === 0 && err.code === 'network',
  );
});

test('an aborted request is reported as a timeout, not a mystery', async () => {
  const fetchImpl = (async (_input: unknown, init?: RequestInit) => {
    return new Promise<Response>((_resolve, reject) => {
      init?.signal?.addEventListener('abort', () => {
        const abort = new Error('aborted');
        abort.name = 'AbortError';
        reject(abort);
      });
    });
  }) as unknown as typeof fetch;
  const backend = new Backend({
    baseUrl: 'http://backend.test',
    apiKey: KEY,
    fetchImpl,
    sleep: noSleep,
    timeoutMs: 5,
    attempts: 1,
  });

  await assert.rejects(
    () => backend.deliveries(10),
    (err: unknown) => err instanceof BackendError && err.code === 'timeout',
  );
});

test('public reads carry no internal key', async () => {
  const { fetchImpl, calls } = stubFetch([
    json({ status: 'success', data: { categories: [{ slug: 'ict', label: 'ICT', group: 'all_fields' }], count: 1 } }),
    json({ status: 'success', data: [{ _id: 'job-1', title: 'T', organization: 'O' }] }),
  ]);
  const backend = new Backend({ baseUrl: 'http://backend.test', apiKey: KEY, fetchImpl, sleep: noSleep });

  const categories = await backend.categories();
  assert.deepEqual(categories.map((category) => category.slug), ['ict']);
  assert.equal(calls[0]?.headers['x-api-key'], undefined);

  const page = await backend.listJobs({ kind: 'tender', category: 'ict', limit: 5 });
  assert.equal(calls[1]?.url, 'http://backend.test/jobs?kind=tender&category=ict&limit=5');
  assert.equal(calls[1]?.headers['x-api-key'], undefined);
  assert.equal(page.items.length, 1);
});

test('the feed page normaliser accepts the array the backend actually returns', () => {
  assert.deepEqual(normalizeFeedPage([{ _id: 'a' }]).items.length, 1);
  assert.deepEqual(normalizeFeedPage({ items: [{ _id: 'a' }], total: 9 }), { items: [{ _id: 'a' }], total: 9 });
  assert.deepEqual(normalizeFeedPage(undefined), { items: [], total: 0 });
});

test('delivery failures are posted in one batched request', async () => {
  const { fetchImpl, calls } = stubFetch([json({ status: 'success', data: { marked: 2, alertsPaused: 0 } })]);
  const backend = new Backend({ baseUrl: 'http://backend.test', apiKey: KEY, fetchImpl, sleep: noSleep });

  const result = await backend.markFailed([
    { id: 'n1', reason: 'blocked' },
    { id: 'n2', reason: 'transient' },
  ]);

  assert.equal(calls[0]?.url, 'http://backend.test/internal/deliveries/failed');
  assert.deepEqual(calls[0]?.body, {
    failures: [
      { id: 'n1', reason: 'blocked' },
      { id: 'n2', reason: 'transient' },
    ],
  });
  assert.equal(result.marked, 2);
});

test('marking an empty page sent is a no-op, not a request', async () => {
  const { fetchImpl, calls } = stubFetch([]);
  const backend = new Backend({ baseUrl: 'http://backend.test', apiKey: KEY, fetchImpl, sleep: noSleep });
  assert.equal(await backend.markSent([]), 0);
  assert.equal(calls.length, 0);
});

test('jobsForUser asks the backend for the member by snowflake and unwraps rows and meta', async () => {
  const { fetchImpl, calls } = stubFetch([
    json({
      status: 'success',
      data: [{ job: { _id: 'job-1', title: 'Data Analyst', organization: 'ACTED' }, matchScore: 4.5 }],
      meta: {
        reason: 'ok',
        total: 5,
        categories: ['ict'],
        categorySource: 'llm',
      },
    }),
  ]);
  const backend = new Backend({ baseUrl: 'http://backend.test', apiKey: KEY, fetchImpl, sleep: noSleep });

  const { rows, meta } = await backend.jobsForUser('900000000000009401', { limit: 3, kind: 'tender' });

  // The route addresses the member by their own snowflake: no token, no email round-trip.
  assert.equal(
    calls[0]?.url,
    'http://backend.test/internal/jobs/for-user/900000000000009401?limit=3&kind=tender',
  );
  assert.equal(calls[0]?.headers['authorization'], undefined, 'internal routes carry the key, not a bearer');
  assert.equal(rows.length, 1);
  assert.equal(rows[0]?.matchScore, 4.5);
  // The decision is rendered from meta, never re-derived in the bot.
  assert.deepEqual(meta.categories, ['ict']);
  assert.equal(meta.categorySource, 'llm');
  assert.equal(meta.reason, 'ok');
});

test('jobsForUser tolerates a meta-less backend and an unknown member is an error', async () => {
  const { fetchImpl } = stubFetch([
    json({ status: 'success', data: [] }),
    json({ status: 'error', error: { code: 'not_found', message: 'no such user' } }, 404),
  ]);
  const backend = new Backend({ baseUrl: 'http://backend.test', apiKey: KEY, fetchImpl, sleep: noSleep });

  const { rows, meta } = await backend.jobsForUser('1');
  assert.deepEqual(rows, []);
  assert.deepEqual(meta.categories, []);
  assert.equal(meta.total, 0);

  await assert.rejects(() => backend.jobsForUser('unknown'), (err: unknown) => {
    assert.ok(err instanceof BackendError);
    assert.equal(err.status, 404);
    return true;
  });
});
