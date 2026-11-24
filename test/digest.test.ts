import assert from 'node:assert/strict';
import { test } from 'node:test';

import { runDigest, groupByUser } from '../src/digest.js';
import { BackendError } from '../src/http.js';
import { createLogger } from '../src/log.js';
import type { Delivery } from '../src/types.js';

const APP = 'https://app.jobify.test';
const silentLogger = createLogger('error', () => undefined);

function delivery(id: string, userId: string, title = `Role ${id}`): Delivery {
  return {
    notificationId: id,
    userId,
    title,
    body: 'body',
    jobId: `job-${id}`,
    job: {
      _id: `job-${id}`,
      title,
      organization: 'Horn Relief',
      location: 'Mogadishu',
      deadline: '2026-12-03',
    },
  };
}

interface FakeBackend {
  refreshMatches(): Promise<{ subscribers: number; queuedTotal: number }>;
  deliveries(limit?: number): Promise<Delivery[]>;
  linkForUser(userId: string): Promise<{ user_id: string; discord_id: string; username: string }>;
  markSent(ids: string[]): Promise<number>;
  markFailed(failures: Array<{ id: string; reason: string }>): Promise<{ marked: number; alertsPaused: number }>;
}

function fakeBackend(options: {
  pending: Delivery[];
  link?: (userId: string) => Promise<{ user_id: string; discord_id: string; username: string }>;
  refreshFails?: boolean;
}): { backend: FakeBackend; sent: string[][]; failed: Array<Array<{ id: string; reason: string }>> } {
  const sent: string[][] = [];
  const failed: Array<Array<{ id: string; reason: string }>> = [];
  const backend: FakeBackend = {
    async refreshMatches() {
      if (options.refreshFails) throw new Error('refresh exploded');
      return { subscribers: 1, queuedTotal: options.pending.length };
    },
    async deliveries() {
      return options.pending;
    },
    async linkForUser(userId) {
      if (options.link) return options.link(userId);
      return { user_id: userId, discord_id: `discord-${userId}`, username: 'sahra' };
    },
    async markSent(ids) {
      sent.push(ids);
      return ids.length;
    },
    async markFailed(failures) {
      failed.push(failures);
      return { marked: failures.length, alertsPaused: 0 };
    },
  };
  return { backend, sent, failed };
}

test('a user with nine pending matches gets one DM of five and four left pending', async () => {
  const pending = Array.from({ length: 9 }, (_, index) => delivery(`n${index + 1}`, 'u1'));
  const { backend, sent, failed } = fakeBackend({ pending });
  const dms: Array<{ discordId: string; content: string }> = [];

  const report = await runDigest({
    backend,
    appBaseUrl: APP,
    logger: silentLogger,
    sendDm: async (discordId, content) => {
      dms.push({ discordId, content });
    },
  });

  assert.equal(dms.length, 1, 'one DM per member, not one per match');
  assert.equal(dms[0]?.discordId, 'discord-u1');
  assert.equal(dms[0]?.content.match(/\/jobs\/job-/g)?.length, 5);
  assert.ok(dms[0]?.content.includes('4 more waiting'));

  // Only the five that were actually sent are marked; the rest stay for tomorrow.
  assert.deepEqual(sent.flat().sort(), ['n1', 'n2', 'n3', 'n4', 'n5']);
  assert.equal(failed.length, 0);
  assert.equal(report.sent, 5);
  assert.equal(report.deferred, 4);
  assert.equal(report.pending, 9);
  assert.equal(report.dms, 1);
});

test('a blocked bot is recorded as blocked, per notification', async () => {
  const { backend, sent, failed } = fakeBackend({ pending: [delivery('n1', 'u1'), delivery('n2', 'u1')] });

  const report = await runDigest({
    backend,
    appBaseUrl: APP,
    logger: silentLogger,
    sendDm: async () => {
      const err = Object.assign(new Error('Cannot send messages to this user'), { code: 50007 });
      throw err;
    },
  });

  assert.equal(sent.length, 0);
  assert.deepEqual(failed[0], [
    { id: 'n1', reason: 'blocked' },
    { id: 'n2', reason: 'blocked' },
  ]);
  assert.equal(report.failed, 2);
  assert.equal(report.sent, 0);
});

test('an account with no Discord identity is recorded as unlinked, not retried', async () => {
  const { backend, failed } = fakeBackend({
    pending: [delivery('n1', 'web-user')],
    link: async () => {
      throw new BackendError(404, 'not_found', 'no link');
    },
  });

  const report = await runDigest({
    backend,
    appBaseUrl: APP,
    logger: silentLogger,
    sendDm: async () => {
      assert.fail('an unlinked account must not be DMd');
    },
  });

  assert.deepEqual(failed[0], [{ id: 'n1', reason: 'no_discord_link' }]);
  assert.equal(report.unlinked, 1);
  assert.equal(report.failed, 1);
});

test('a lookup failure elsewhere leaves the rows queued', async () => {
  const { backend, sent, failed } = fakeBackend({
    pending: [delivery('n1', 'u1')],
    link: async () => {
      throw new BackendError(500, 'internal_error', 'mongo down');
    },
  });

  const report = await runDigest({
    backend,
    appBaseUrl: APP,
    logger: silentLogger,
    sendDm: async () => {
      assert.fail('nothing to send when the identity lookup failed');
    },
  });

  assert.equal(sent.length, 0);
  assert.equal(failed.length, 0);
  assert.equal(report.deferred, 1);
});

test('one DM per member, each capped, across several recipients', async () => {
  const pending = [
    delivery('n1', 'u1'),
    delivery('n2', 'u2'),
    delivery('n3', 'u1'),
    delivery('n4', 'u2'),
  ];
  const { backend, sent } = fakeBackend({ pending });
  const dms: string[] = [];

  const report = await runDigest({
    backend,
    appBaseUrl: APP,
    logger: silentLogger,
    sendDm: async (discordId) => {
      dms.push(discordId);
    },
  });

  assert.deepEqual(dms.sort(), ['discord-u1', 'discord-u2']);
  assert.equal(report.sent, 4);
  assert.equal(sent.flat().length, 4);
});

test('a failed refresh still drains what is already queued', async () => {
  const { backend, sent } = fakeBackend({ pending: [delivery('n1', 'u1')], refreshFails: true });

  const report = await runDigest({
    backend,
    appBaseUrl: APP,
    logger: silentLogger,
    sendDm: async () => undefined,
  });

  assert.equal(report.refreshedQueued, 0);
  assert.equal(report.sent, 1);
  assert.equal(sent.length, 1);
});

test('an empty queue sends nothing and says so quietly', async () => {
  const { backend, sent } = fakeBackend({ pending: [] });
  const report = await runDigest({
    backend,
    appBaseUrl: APP,
    logger: silentLogger,
    sendDm: async () => {
      assert.fail('nothing should be sent');
    },
  });
  assert.equal(report.pending, 0);
  assert.equal(sent.length, 0);
});

test('grouping preserves the queue order per recipient', () => {
  const groups = groupByUser([delivery('n1', 'u1'), delivery('n2', 'u2'), delivery('n3', 'u1')]);
  assert.deepEqual(
    groups.get('u1')?.map((item) => item.notificationId),
    ['n1', 'n3'],
  );
  assert.deepEqual(
    groups.get('u2')?.map((item) => item.notificationId),
    ['n2'],
  );
});
