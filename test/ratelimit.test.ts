/**
 * The pacing rules, in virtual time: no test here waits a real second.
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  DEFAULT_DM_LIMITS,
  DiscordRateLimiter,
  isRateLimitError,
  isUnknownChannel,
  retryAfterMs,
  withDiscordRetry,
} from '../src/ratelimit.js';

function virtualClock(): { now: () => number; sleep: (ms: number) => Promise<void>; slept: number[] } {
  let clock = 0;
  const slept: number[] = [];
  return {
    now: () => clock,
    sleep: async (ms: number) => {
      slept.push(ms);
      clock += ms;
    },
    slept,
  };
}

test('a burst goes out at once, then the sustained rate applies', async () => {
  const clock = virtualClock();
  const limiter = new DiscordRateLimiter({ ...DEFAULT_DM_LIMITS, now: clock.now, sleep: clock.sleep });

  for (let i = 0; i < DEFAULT_DM_LIMITS.burst; i += 1) await limiter.acquire(`user-${i}`);
  assert.deepEqual(clock.slept, [], 'the burst is free');

  await limiter.acquire('user-next');
  assert.deepEqual(clock.slept, [Math.round(1000 / DEFAULT_DM_LIMITS.perSecond)]);
});

test('two sends to the same member are spaced, two members are not', async () => {
  const clock = virtualClock();
  const limiter = new DiscordRateLimiter({
    burst: 100,
    perSecond: 100,
    perUserMs: 1_200,
    now: clock.now,
    sleep: clock.sleep,
  });

  await limiter.acquire('member-a');
  await limiter.acquire('member-b');
  assert.deepEqual(clock.slept, [], 'different members do not wait for each other');

  await limiter.acquire('member-a');
  assert.deepEqual(clock.slept, [1_200], 'the same member waits the per-user gap');
});

test('a concurrent second send to one member queues behind the first', async () => {
  const clock = virtualClock();
  const limiter = new DiscordRateLimiter({
    burst: 100,
    perSecond: 100,
    perUserMs: 1_000,
    now: clock.now,
    sleep: clock.sleep,
  });

  await Promise.all([limiter.acquire('member-a'), limiter.acquire('member-a')]);
  assert.equal(clock.slept.length, 1, 'one of the two had to wait');
  assert.ok(clock.slept[0] >= 1_000 - 1);
});

test('a 429 is waited out for exactly as long as Discord asked', async () => {
  const clock = virtualClock();
  let attempts = 0;
  const result = await withDiscordRetry(
    async () => {
      attempts += 1;
      if (attempts === 1) {
        const err = Object.assign(new Error('rate limited'), { status: 429, retryAfter: 2 });
        throw err;
      }
      return 'sent';
    },
    { sleep: clock.sleep, jitterMs: 0 },
  );

  assert.equal(result, 'sent');
  assert.equal(attempts, 2);
  assert.deepEqual(clock.slept, [2_000]);
});

test('a blocked DM is final: no retry, no waiting', async () => {
  const clock = virtualClock();
  let attempts = 0;
  await assert.rejects(
    withDiscordRetry(
      async () => {
        attempts += 1;
        throw Object.assign(new Error('Cannot send messages to this user'), { status: 403 });
      },
      { sleep: clock.sleep, jitterMs: 0 },
    ),
  );
  assert.equal(attempts, 1);
  assert.deepEqual(clock.slept, []);
});

test('a stale cached channel is re-opened before the retry', async () => {
  const clock = virtualClock();
  let reopened = 0;
  let attempts = 0;
  await withDiscordRetry(
    async () => {
      attempts += 1;
      if (attempts === 1) throw Object.assign(new Error('Unknown Channel'), { code: 10003 });
    },
    { sleep: clock.sleep, jitterMs: 0, onRetryable: () => { reopened += 1; } },
  );
  assert.equal(reopened, 1);
  assert.equal(attempts, 2);
});

test('the readers recognise the shapes discord.js actually throws', () => {
  assert.equal(retryAfterMs({ retryAfter: 1.5 }), 1_500);
  assert.equal(retryAfterMs({ rawError: { retry_after: 0.25 } }), 250);
  assert.equal(retryAfterMs(new Error('nope')), null);
  assert.equal(isRateLimitError({ status: 429 }), true);
  assert.equal(isRateLimitError({ name: 'RateLimitError' }), true);
  assert.equal(isRateLimitError({ status: 403 }), false);
  assert.equal(isUnknownChannel({ code: 10003 }), true);
  assert.equal(isUnknownChannel({ code: 50035 }), false);
});
