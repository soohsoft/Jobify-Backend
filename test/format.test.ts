import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  MAX_DIGEST_ROWS,
  assistantFailureCopy,
  capRows,
  deliveryRow,
  digestMessage,
  dmFailureReason,
  formatAssistantReply,
  formatRows,
  helpText,
  jobLink,
  rowLine,
} from '../src/format.js';
import type { AssistantAnswer, Delivery, JobRow } from '../src/types.js';

const APP = 'https://app.jobify.test';

function row(overrides: Partial<JobRow> = {}): JobRow {
  return {
    job_id: 'job-1',
    title: 'Data Analyst',
    organization: 'Horn Relief',
    location: 'Mogadishu',
    kind: 'job',
    deadline: '2026-12-03',
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// Assistant reply mapping
// ---------------------------------------------------------------------------

test('an assistant answer keeps its own sentence and appends the real rows as links', () => {
  const answer: AssistantAnswer = {
    intent: 'search',
    reply: 'I found three ICT roles in Mogadishu.',
    jobs: [
      row(),
      row({ job_id: 'job-2', title: 'Network Engineer', organization: 'Hormuud' }),
      row({ job_id: 'job-3', title: 'IT Support Officer', organization: 'UNDP' }),
    ],
    credits: { balance: 0, monthly_allowance: 30, used_this_period: 1, period_end: '2026-12-24T00:00:00Z', remaining_this_period: 29 },
  };

  const content = formatAssistantReply(answer, { appBaseUrl: APP, maxRows: 3 });

  assert.ok(content.startsWith('I found three ICT roles in Mogadishu.'));
  assert.ok(content.includes('**Data Analyst** — Horn Relief — Mogadishu — closes 2026-12-03'));
  assert.ok(content.includes(`${APP}/jobs/job-1`));
  assert.ok(content.includes(`${APP}/jobs/job-3`));
  // Exactly the three rows the backend returned: no invented fourth.
  assert.equal(content.match(/\/jobs\/job-/g)?.length, 3);
});

test('rows beyond the reply cap are counted, not silently dropped', () => {
  const answer: AssistantAnswer = {
    intent: 'search',
    reply: 'Six matches.',
    jobs: Array.from({ length: 6 }, (_, index) => row({ job_id: `job-${index + 1}`, title: `Role ${index + 1}` })),
    credits: { balance: 0, monthly_allowance: 30, used_this_period: 1, period_end: '', remaining_this_period: 29 },
  };

  const content = formatAssistantReply(answer, { appBaseUrl: APP, maxRows: 3 });
  assert.equal(content.match(/\/jobs\/job-/g)?.length, 3);
  assert.ok(content.includes('_…and 3 more in the portal._'));
});

test('an empty result is answered with the backend sentence alone', () => {
  const answer: AssistantAnswer = {
    intent: 'search',
    reply: 'Nothing live matches that right now.',
    jobs: [],
    credits: { balance: 0, monthly_allowance: 30, used_this_period: 2, period_end: '', remaining_this_period: 28 },
  };
  assert.equal(formatAssistantReply(answer, { appBaseUrl: APP }), 'Nothing live matches that right now.');
});

test('a row without a location or deadline says so instead of guessing', () => {
  const line = rowLine({ title: 'Driver', organization: 'NGO', location: null, deadline: null });
  assert.equal(line, '**Driver** — NGO — location not listed — no deadline listed');
  assert.ok(!line.includes('closes'));
});

test('a row with no id renders without a link', () => {
  const rendered = formatRows([{ title: 'Driver', organization: 'NGO', location: null, deadline: null, jobId: null }], {
    appBaseUrl: APP,
  });
  assert.ok(!rendered.includes(APP));
  assert.equal(jobLink(APP, null), null);
});

// ---------------------------------------------------------------------------
// Out of credits and friends
// ---------------------------------------------------------------------------

test('402 out_of_credits offers the purchase action and never echoes the backend body', () => {
  const copy = assistantFailureCopy(402, 'out_of_credits');
  assert.equal(copy.offerPurchase, true);
  assert.match(copy.content, /free searches/i);
  assert.match(copy.content, /Buy credits/);
});

test('429 is "slow down" and 502 is honest about nothing being charged', () => {
  const slow = assistantFailureCopy(429, 'rate_limited');
  assert.equal(slow.offerPurchase, false);
  assert.match(slow.content, /Slow down/);

  const down = assistantFailureCopy(502, 'llm_unavailable');
  assert.equal(down.offerPurchase, false);
  assert.match(down.content, /unavailable/);
  assert.match(down.content, /Nothing was charged/);
});

test('an unknown failure gets a generic line rather than internals', () => {
  const copy = assistantFailureCopy(500, 'internal_error');
  assert.equal(copy.offerPurchase, false);
  assert.ok(!copy.content.includes('internal_error'));
  assert.ok(!copy.content.includes('sql'));
});

// ---------------------------------------------------------------------------
// Delivery outcome mapping
// ---------------------------------------------------------------------------

test('a blocked bot is terminal and a missing user is deactivated', () => {
  assert.equal(dmFailureReason({ code: 50007 }), 'blocked');
  assert.equal(dmFailureReason({ status: 403 }), 'blocked');
  assert.equal(dmFailureReason({ code: 10013 }), 'deactivated');
  assert.equal(dmFailureReason({ status: 404 }), 'deactivated');
});

test('anything else is transient, including a plain Error', () => {
  assert.equal(dmFailureReason(new Error('socket hang up')), 'transient');
  assert.equal(dmFailureReason({ code: 0 }), 'transient');
  assert.equal(dmFailureReason(null), 'transient');
  assert.equal(dmFailureReason('nope'), 'transient');
});

// ---------------------------------------------------------------------------
// The 5-per-DM cap
// ---------------------------------------------------------------------------

test('the digest cap is five rows and the remainder is reported', () => {
  assert.equal(MAX_DIGEST_ROWS, 5);

  const rows = Array.from({ length: 9 }, (_, index) => ({
    title: `Role ${index + 1}`,
    organization: 'Org',
    location: 'Mogadishu',
    deadline: '2026-12-01',
    jobId: `job-${index + 1}`,
  }));

  const { delivered, remaining } = capRows(rows);
  assert.equal(delivered.length, 5);
  assert.equal(remaining, 4);

  const message = digestMessage(rows, { appBaseUrl: APP });
  assert.equal(message.match(/\/jobs\/job-/g)?.length, 5);
  assert.ok(message.includes('**5 new matches for you on Jobify**'));
  assert.ok(message.includes("4 more waiting"));
  assert.ok(message.length < 2000, 'a digest DM must fit in one Discord message');
});

test('a single match reads naturally', () => {
  const message = digestMessage([{ title: 'Only Role', organization: 'Org', location: null, deadline: null, jobId: 'job-9' }], {
    appBaseUrl: APP,
  });
  assert.ok(message.includes('**1 new match for you on Jobify**'));
});

test('a delivery prefers the embedded job document and falls back to the notification title', () => {
  const withJob: Delivery = {
    notificationId: 'n1',
    userId: 'u1',
    title: 'Jobify notification',
    body: 'body',
    jobId: 'job-7',
    job: { _id: 'job-7', title: 'WASH Officer', organization: 'Oxfam', location: 'Baidoa', deadline: '2026-12-09' },
  };
  const rowWithJob = deliveryRow(withJob);
  assert.equal(rowWithJob.title, 'WASH Officer');
  assert.equal(rowWithJob.jobId, 'job-7');

  const withoutJob: Delivery = { notificationId: 'n2', userId: 'u1', title: 'Driver needed', body: 'body', jobId: null };
  const fallback = deliveryRow(withoutJob);
  assert.equal(fallback.title, 'Driver needed');
  assert.equal(fallback.location, null);
  assert.equal(fallback.jobId, null);
});

test('help is at most ten lines', () => {
  const lines = helpText().split('\n');
  assert.ok(lines.length <= 10, `help has ${lines.length} lines`);
  assert.ok(helpText().includes('/start'));
  assert.ok(helpText().includes('/stop'));
});
