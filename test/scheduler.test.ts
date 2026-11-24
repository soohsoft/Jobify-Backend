import assert from 'node:assert/strict';
import { test } from 'node:test';

import { createLogger } from '../src/log.js';
import { DailyDigestScheduler, isDigestDue, localClock } from '../src/scheduler.js';

const TZ = 'Africa/Nairobi';
const silentLogger = createLogger('error', () => undefined);

test('the wall clock is read in the configured zone, not the host zone', () => {
  // 05:00 UTC is 08:00 in Nairobi.
  assert.deepEqual(localClock(new Date('2026-11-24T05:00:00Z'), TZ), { date: '2026-11-24', hour: 8, minute: 0 });
  // 21:30 UTC is already the next day, 00:30 local.
  assert.deepEqual(localClock(new Date('2026-11-24T21:30:00Z'), TZ), { date: '2026-11-25', hour: 0, minute: 30 });
});

test('due only inside the morning window, and only once per local day', () => {
  const at = (iso: string) => localClock(new Date(iso), TZ);

  assert.equal(isDigestDue(at('2026-11-24T05:00:00Z'), 8, null), true);
  // A restart at 08:07 still catches that morning's digest once.
  assert.equal(isDigestDue(at('2026-11-24T05:07:00Z'), 8, null), true);
  // Already ran today.
  assert.equal(isDigestDue(at('2026-11-24T05:07:00Z'), 8, '2026-11-24'), false);
  // Too late in the hour: refuse rather than send a digest at 08:50.
  assert.equal(isDigestDue(at('2026-11-24T05:50:00Z'), 8, null), false);
  // Not the configured hour.
  assert.equal(isDigestDue(at('2026-11-24T03:00:00Z'), 8, null), false);
});

test('the scheduler runs once per day even when ticked repeatedly', async () => {
  let runs = 0;
  const scheduler = new DailyDigestScheduler({
    timeZone: TZ,
    hour: 8,
    logger: silentLogger,
    now: () => new Date('2026-11-24T05:00:30Z'),
    run: async () => {
      runs += 1;
    },
  });

  await scheduler.tick();
  await scheduler.tick();
  await scheduler.tick();

  assert.equal(runs, 1);
  assert.equal(scheduler.lastRun, '2026-11-24');
});

test('a thrown digest is logged and does not wedge the scheduler', async () => {
  let attempts = 0;
  const scheduler = new DailyDigestScheduler({
    timeZone: TZ,
    hour: 8,
    logger: silentLogger,
    now: () => new Date('2026-11-24T05:01:00Z'),
    run: async () => {
      attempts += 1;
      throw new Error('backend down');
    },
  });

  await scheduler.tick();
  assert.equal(attempts, 1);
  // Same day: no second attempt, which is what stops a failing run from hammering the API.
  await scheduler.tick();
  assert.equal(attempts, 1);
});

test('an off-hour tick does nothing at all', async () => {
  let runs = 0;
  const scheduler = new DailyDigestScheduler({
    timeZone: TZ,
    hour: 8,
    logger: silentLogger,
    now: () => new Date('2026-11-24T12:00:00Z'),
    run: async () => {
      runs += 1;
    },
  });
  await scheduler.tick();
  assert.equal(runs, 0);
});
