import assert from 'node:assert/strict';
import { test } from 'node:test';

import { Collection } from 'discord.js';

import {
  INTRO_MARKER,
  ensureIntro,
  hasMarker,
  introContent,
  zoneLabel,
  type IntroChannel,
  type IntroMessage,
} from '../src/intro.js';
import type { Logger } from '../src/log.js';

const CHANNEL_ID = '1574842294778069086';

const silentLogger: Logger = {
  debug: () => undefined,
  info: () => undefined,
  warn: () => undefined,
  error: () => undefined,
};

interface FakeMessage extends IntroMessage {
  pinned: boolean;
  edits: string[];
  pinCalls: number;
}

function fakeMessage(id: string, content: string, pinned = false): FakeMessage {
  const message: FakeMessage = {
    id,
    content,
    pinned,
    edits: [],
    pinCalls: 0,
    async pin() {
      message.pinCalls += 1;
      message.pinned = true;
      return message;
    },
    async edit(payload) {
      message.edits.push(payload.content);
      message.content = payload.content;
      return message;
    },
  };
  return message;
}

interface FakeChannel extends IntroChannel {
  stored: FakeMessage[];
  sent: Array<{ content: string; allowedMentions?: { parse: [] } }>;
  pinFails?: boolean;
}

function fakeChannel(messages: FakeMessage[] = [], options: { pinFails?: boolean } = {}): FakeChannel {
  const stored = [...messages];
  const sent: Array<{ content: string; allowedMentions?: { parse: [] } }> = [];
  let sequence = stored.length;
  const channel: FakeChannel = {
    id: CHANNEL_ID,
    stored,
    sent,
    messages: {
      fetchPins: async () => ({
        items: new Collection(stored.filter((message) => message.pinned).map((message) => [message.id, message])),
      }),
      fetch: async ({ limit }) =>
        new Collection(
          stored
            .slice(-limit)
            .reverse()
            .map((message) => [message.id, message]),
        ),
    },
    send: async (payload) => {
      sent.push(payload);
      sequence += 1;
      const message = fakeMessage(`new-${sequence}`, payload.content);
      if (options.pinFails) {
        message.pin = async () => {
          throw Object.assign(new Error('Missing Permissions'), { code: 50013 });
        };
      }
      stored.push(message);
      return message;
    },
  };
  return channel;
}

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

test('the intro explains search, commands, the digest and carries the marker', () => {
  const content = introContent(8, 'Africa/Nairobi');
  assert.ok(content.includes('**How Jobify works**'));
  assert.ok(content.includes('`nurse jobs in Mogadishu`'));
  assert.ok(content.includes('/start'));
  assert.ok(content.includes('/alerts'));
  assert.ok(content.includes('/account'));
  assert.ok(content.includes('08:00 EAT'));
  assert.ok(content.trimEnd().endsWith(INTRO_MARKER), 'the marker must be the last line');
  assert.ok(content.length <= 1900);
  assert.equal(zoneLabel('Africa/Nairobi'), 'EAT');
  assert.equal(zoneLabel('Europe/London'), 'Europe/London');
});

test('the marker is found in real message text', () => {
  assert.ok(hasMarker({ content: `hello\n${INTRO_MARKER}` }));
  assert.ok(!hasMarker({ content: 'hello' }));
  // A different version marker is a different message.
  assert.ok(!hasMarker({ content: '-# jobify-intro v2' }));
});

// ---------------------------------------------------------------------------
// Idempotence
// ---------------------------------------------------------------------------

test('an empty channel gets the intro posted and pinned exactly once', async () => {
  const channel = fakeChannel();

  const outcome = await ensureIntro(channel, silentLogger, introContent());

  assert.equal(outcome, 'posted');
  assert.equal(channel.sent.length, 1);
  assert.ok(channel.sent[0]?.content.includes(INTRO_MARKER));
  assert.deepEqual(channel.sent[0]?.allowedMentions, { parse: [] }, 'the intro must not ping anyone');
  assert.equal(channel.stored.length, 1);
  assert.equal(channel.stored[0]?.pinCalls, 1);
  assert.equal(channel.stored[0]?.pinned, true);
});

test('a second run finds the pinned intro and posts nothing new', async () => {
  const channel = fakeChannel();
  await ensureIntro(channel, silentLogger, introContent());

  const outcome = await ensureIntro(channel, silentLogger, introContent());

  assert.equal(outcome, 'exists');
  assert.equal(channel.sent.length, 1, 'the restart must not duplicate the intro');
  assert.equal(channel.stored.length, 1);
  assert.equal(channel.stored[0]?.edits.length, 0);
});

test('an intro present only in recent history is re-pinned, never reposted', async () => {
  const existing = fakeMessage('m1', introContent(), false);
  const channel = fakeChannel([existing]);

  const outcome = await ensureIntro(channel, silentLogger, introContent());

  assert.equal(outcome, 'repinned');
  assert.equal(channel.sent.length, 0, 'an existing copy must never be duplicated');
  assert.equal(existing.pinCalls, 1);
  assert.equal(existing.edits.length, 0, 'content is left alone');
});

test('when re-pinning is refused the intro is left as it is', async () => {
  const existing = fakeMessage('m1', introContent(), false);
  existing.pin = async () => {
    throw Object.assign(new Error('Missing Permissions'), { code: 50013 });
  };
  const channel = fakeChannel([existing]);

  const outcome = await ensureIntro(channel, silentLogger, introContent());

  assert.equal(outcome, 'exists');
  assert.equal(channel.sent.length, 0);
  assert.equal(existing.edits.length, 0);
});

test('a pinned intro with changed copy is edited in place instead of reposted', async () => {
  const existing = fakeMessage('m1', `stale copy\n${INTRO_MARKER}`, true);
  const channel = fakeChannel([existing]);

  const outcome = await ensureIntro(channel, silentLogger, introContent());

  assert.equal(outcome, 'updated');
  assert.equal(channel.sent.length, 0);
  assert.equal(existing.edits.length, 1);
  assert.ok(existing.edits[0]?.includes('**How Jobify works**'));
});

test('a pin failure still leaves exactly one intro and does not throw', async () => {
  const channel = fakeChannel([], { pinFails: true });

  const outcome = await ensureIntro(channel, silentLogger, introContent());

  assert.equal(outcome, 'posted-unpinned');
  assert.equal(channel.sent.length, 1);
  assert.equal(channel.stored.length, 1);

  // A later restart finds the marker and stays quiet.
  const again = await ensureIntro(channel, silentLogger, introContent());
  assert.equal(again, 'exists');
  assert.equal(channel.sent.length, 1);
});

test('an older discord.js without fetchPins still finds the pinned intro', async () => {
  const existing = fakeMessage('m1', introContent(), true);
  const channel = fakeChannel([existing]);
  const legacy: IntroChannel = {
    id: channel.id,
    messages: {
      fetchPinned: async () => new Collection([[existing.id, existing]]),
      fetch: async () => new Collection([[existing.id, existing]]),
    },
    send: channel.send.bind(channel),
  };

  const outcome = await ensureIntro(legacy, silentLogger, introContent());

  assert.equal(outcome, 'exists');
  assert.equal(existing.edits.length, 0);
});

test('concurrent calls serialise into one post', async () => {
  const channel = fakeChannel();

  const [first, second] = await Promise.all([
    ensureIntro(channel, silentLogger, introContent()),
    ensureIntro(channel, silentLogger, introContent()),
  ]);

  assert.deepEqual([first, second].sort(), ['exists', 'posted']);
  assert.equal(channel.sent.length, 1, 'two ready events must not race into two intros');
  assert.equal(channel.stored.length, 1);
});
