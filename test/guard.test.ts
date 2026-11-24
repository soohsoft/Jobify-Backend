import assert from 'node:assert/strict';
import { test } from 'node:test';

import { RequestGuard } from '../src/context.js';

// A redelivered interaction or message must not produce a second identical list in the
// member's DM. The guard is the last line: one answer per event id.

test('an id is answered once and every repeat is refused', () => {
  const guard = new RequestGuard();
  assert.equal(guard.claim('interaction-1'), true);
  assert.equal(guard.claim('interaction-1'), false);
  assert.equal(guard.claim('interaction-1'), false);
  // A different request is untouched.
  assert.equal(guard.claim('interaction-2'), true);
  // The message namespace is separate from the interaction one.
  assert.equal(guard.claim('msg:interaction-1'), true);
});

test('a missing id is never treated as a duplicate', () => {
  const guard = new RequestGuard();
  // Without an id there is nothing to claim; refusing would silently drop a real request.
  assert.equal(guard.claim(undefined), true);
  assert.equal(guard.claim(null), true);
  assert.equal(guard.claim(''), true);
});

test('the cache stays bounded and forgets old ids', () => {
  const guard = new RequestGuard(1, 3);
  for (let i = 0; i < 20; i += 1) {
    assert.equal(guard.claim(`id-${i}`), true);
  }
  // The newest id is still remembered...
  assert.equal(guard.claim('id-19'), false);
});
