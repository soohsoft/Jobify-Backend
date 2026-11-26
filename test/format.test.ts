import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  MAX_DIGEST_ROWS,
  NO_MATCHES_SENTENCE,
  assistantFailureCopy,
  capRows,
  dateOnly,
  deliveryRow,
  digestMessage,
  dmFailureReason,
  formatAssistantReply,
  formatRows,
  jobLink,
  matchLabel,
  noMatchesCopy,
  rowLine,
  sourceLink,
  uniqueRows,
} from '../src/format.js';
import type { AssistantAnswer, Delivery, JobRow } from '../src/types.js';
import type { RowInput } from '../src/format.js';

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

test('an assistant answer is the rows and nothing else: no sentence, no footer', () => {
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

  assert.ok(!content.includes('I found three ICT roles'), 'no commentary around the list');
  assert.ok(content.startsWith('1. **Data Analyst**'));
  assert.ok(content.includes('**Data Analyst** — Horn Relief — Mogadishu — closes 2026-12-03'));
  assert.ok(content.includes(`${APP}/jobs/job-1`));
  assert.ok(content.includes(`${APP}/jobs/job-3`));
  // Exactly the three rows the backend returned: no invented fourth.
  assert.equal(content.match(/\/jobs\/job-/g)?.length, 3);
  assert.ok(!content.includes('% match'), 'the list view has no percentage');
});

test('the reply cap holds the list at three rows', () => {
  const answer: AssistantAnswer = {
    intent: 'search',
    reply: 'Six matches.',
    jobs: Array.from({ length: 6 }, (_, index) => row({ job_id: `job-${index + 1}`, title: `Role ${index + 1}` })),
    credits: { balance: 0, monthly_allowance: 30, used_this_period: 1, period_end: '', remaining_this_period: 29 },
  };

  const content = formatAssistantReply(answer, { appBaseUrl: APP, maxRows: 3 });
  assert.equal(content.match(/\/jobs\/job-/g)?.length, 3);
  assert.ok(!content.includes('…and'), 'no footer around the rows');
});

test('an empty result is answered with the product sentence, never the internal phrasing', () => {
  const answer: AssistantAnswer = {
    intent: 'search',
    reply: 'Nothing live matches that right now.',
    jobs: [],
    credits: { balance: 0, monthly_allowance: 30, used_this_period: 2, period_end: '', remaining_this_period: 28 },
  };
  assert.equal(formatAssistantReply(answer, { appBaseUrl: APP }), NO_MATCHES_SENTENCE);

  // Any other zero-row answer (a question about credits, say) is passed through untouched.
  const other: AssistantAnswer = { ...answer, intent: 'help', reply: 'Run /credits to see what is left this period.' };
  assert.equal(formatAssistantReply(other, { appBaseUrl: APP }), other.reply);
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
// Where a job line points
// ---------------------------------------------------------------------------

const SOURCE = 'https://somalijobs.com/jobs/mogadishu/14525213046612058/reach-data-analyst';
const LOCAL = 'http://localhost:5173';

/** A rendered-row input (not the wire shape): this is what the formatter actually consumes. */
function rowInput(overrides: Partial<RowInput> = {}): RowInput {
  return {
    title: 'Data Analyst',
    organization: 'Horn Relief',
    location: 'Mogadishu',
    deadline: '2026-12-03',
    kind: 'job',
    jobId: 'job-1',
    ...overrides,
  };
}

test('a job line links to the source page, never to the portal', () => {
  const rendered = formatRows([rowInput({ url: SOURCE })], { appBaseUrl: APP });

  assert.ok(rendered.includes(SOURCE), 'the source page is the destination');
  assert.ok(!rendered.includes(`${APP}/jobs/`), 'the portal path is not used when a source exists');
});

test('an assistant answer links each row to its source page', () => {
  const answer: AssistantAnswer = {
    intent: 'search',
    reply: 'One data role in Mogadishu.',
    jobs: [row({ url: SOURCE })],
    credits: { balance: 0, monthly_allowance: 30, used_this_period: 1, period_end: '', remaining_this_period: 29 },
  };

  const content = formatAssistantReply(answer, { appBaseUrl: LOCAL, maxRows: 3 });
  assert.ok(content.includes('https://somalijobs.com/jobs/mogadishu/14525213046612058/reach-data-analyst'));
  assert.ok(!content.includes('localhost'), 'a local portal base must never become a link');
});

test('a local portal base renders the row as text, not as a dead localhost link', () => {
  const rendered = formatRows([rowInput()], { appBaseUrl: LOCAL });
  const digest = digestMessage([rowInput()], { appBaseUrl: LOCAL });

  assert.ok(!rendered.includes('localhost'));
  assert.ok(!digest.includes('localhost'));
  // The line is complete; it simply has nowhere to send the reader.
  assert.equal(rendered, `${rowLine(rowInput(), 1)}\n`);
});

test('localhost on the row itself is dropped as a destination', () => {
  const rendered = formatRows([rowInput({ url: 'http://127.0.0.1:3000/jobs/job-1' })], { appBaseUrl: APP });
  assert.ok(!rendered.includes('127.0.0.1'));
  assert.ok(!rendered.includes('localhost'));
});

test('a row with no source url at all renders text only', () => {
  const rendered = formatRows(
    [{ title: 'Driver', organization: 'NGO', location: null, deadline: null, jobId: null }],
    { appBaseUrl: APP },
  );
  assert.equal(
    rendered,
    `${rowLine({ title: 'Driver', organization: 'NGO', location: null, deadline: null }, 1)}\n`,
  );
  assert.ok(!rendered.includes('http'));
});

test('a malformed or non-http source url is not turned into a link', () => {
  assert.equal(sourceLink(null), null);
  assert.equal(sourceLink('   '), null);
  assert.equal(sourceLink('somalijobs.com/jobs/1'), null);
  assert.equal(sourceLink('javascript:alert(1)'), null);
  assert.equal(sourceLink(SOURCE), SOURCE);
  assert.equal(jobLink(LOCAL, 'job-1'), null);
  assert.equal(jobLink(APP, null), null);
});

test('the portal is only a fallback when the row has no source page', () => {
  const rendered = formatRows([rowInput()], { appBaseUrl: APP });
  assert.ok(rendered.includes(`${APP}/jobs/job-1`));
});

test('a digest row links from the row or the embedded job, and one vacancy is sent once', () => {
  const nested: Delivery = {
    notificationId: 'n1',
    userId: 'u1',
    title: 'Jobify notification',
    body: 'body',
    jobId: 'job-7',
    job: { _id: 'job-7', title: 'WASH Officer', organization: 'Oxfam', location: 'Baidoa', deadline: '2026-12-09', url: SOURCE },
  };
  const atRow: Delivery = { ...nested, notificationId: 'n2', job: null, title: 'WASH Officer', url: SOURCE };

  assert.equal(deliveryRow(nested).url, SOURCE);
  assert.equal(deliveryRow(atRow).url, SOURCE);

  // Both deliveries are the SAME vacancy (job-7); the digest must send it once.
  const rendered = digestMessage([deliveryRow(nested), deliveryRow(atRow)], { appBaseUrl: LOCAL });
  assert.equal(rendered.match(/somalijobs\.com/g)?.length, 1);
  assert.ok(!rendered.includes('localhost'));
  // The opening line is the job itself: no "1 new match" prefix, no markdown — this is what
  // the phone shows on its lock screen.
  assert.ok(rendered.startsWith('WASH Officer'), rendered);
  assert.ok(!rendered.includes('new match for you'), 'the prefix is gone');
  assert.ok(!rendered.startsWith('**'), 'the opening line is plain text');
});

// ---------------------------------------------------------------------------
// Out of credits and friends
// ---------------------------------------------------------------------------

test('402 out_of_credits offers the purchase action and never echoes the backend body', () => {
  const copy = assistantFailureCopy(402, 'out_of_credits');
  assert.equal(copy.offerPurchase, true);
  assert.match(copy.content, /smart searches/i);
  // The owner's sentence: a price in TEXT, and the action is still a plain account link —
  // no in-app purchase, which is the part Discord's policy forbids.
  assert.match(copy.content, /Get unlimited job search/i);
  assert.match(copy.content, /\$1\/month/);
  assert.doesNotMatch(copy.content, /credit card|pay now|checkout/i, 'no payment action in chat');
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
  assert.ok(message.startsWith('Role 1 — Org'), message);
  assert.ok(message.includes("4 more waiting"));
  assert.ok(message.length < 2000, 'a digest DM must fit in one Discord message');
});

test('a single match reads naturally', () => {
  const message = digestMessage([{ title: 'Only Role', organization: 'Org', location: null, deadline: null, jobId: 'job-9' }], {
    appBaseUrl: APP,
  });
  assert.ok(message.startsWith('Only Role'), message);
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

// ---------------------------------------------------------------------------
// The per-row match percentage, and the empty state that keeps scoring internal
// ---------------------------------------------------------------------------

test('the list line is exactly title, employer, place and date — no percentage', () => {
  assert.equal(matchLabel(87), '87% match');
  assert.equal(matchLabel(70), '70% match');
  assert.equal(matchLabel(87.4), '87% match');
  assert.equal(matchLabel(140), '100% match');
  assert.equal(matchLabel(-5), '0% match');
  // An absent or broken score renders nothing rather than an invented number.
  assert.equal(matchLabel(null), null);
  assert.equal(matchLabel(undefined), null);
  assert.equal(matchLabel(Number.NaN), null);

  // The owner's exact line, with and without a score on the row: the score never appears.
  assert.equal(
    rowLine(rowInput({ matchScore: 87 }), 1),
    '1. **Data Analyst** — Horn Relief — Mogadishu — closes 2026-12-03',
  );
  assert.equal(rowLine(rowInput()), '**Data Analyst** — Horn Relief — Mogadishu — closes 2026-12-03');
});

test('the digest shows each job\'s percentage, carried on the queued notification', () => {
  const delivery: Delivery = {
    notificationId: 'n1',
    userId: 'u1',
    title: 'Jobify notification',
    body: 'body',
    jobId: 'job-7',
    matchScore: 87,
    job: { _id: 'job-7', title: 'WASH Officer', organization: 'Oxfam', location: 'Baidoa', deadline: '2026-12-09' },
  };
  assert.equal(deliveryRow(delivery).matchScore, 87);

  const message = digestMessage([deliveryRow(delivery)], { appBaseUrl: APP });
  assert.ok(message.includes('**WASH Officer** — 87% match — Oxfam — Baidoa — closes 2026-12-09'), message);

  // A row queued before scores existed still renders, just without a percentage.
  const legacy = digestMessage([deliveryRow({ ...delivery, matchScore: null })], { appBaseUrl: APP });
  assert.ok(legacy.includes('**WASH Officer** — Oxfam'), legacy);
  assert.ok(!legacy.includes('% match'));
});

test('the empty state is plain product language and never exposes the scoring', () => {
  const sentence = "No jobs match your qualifications right now. We'll notify you when one is posted.";
  assert.equal(noMatchesCopy(), sentence);
  assert.equal(NO_MATCHES_SENTENCE, sentence);

  // The matcher's bar is internal: no number, no percentage, no scoring vocabulary.
  for (const banned of ['threshold', 'scores above', 'cut-off', '70%', 'score']) {
    assert.ok(!sentence.toLowerCase().includes(banned), `the empty state mentions "${banned}"`);
  }
});

test('the digest with nothing to send says the same sentence, never "0 new matches"', () => {
  assert.equal(digestMessage([], { appBaseUrl: APP }), NO_MATCHES_SENTENCE);
  // And a real digest still carries each row's percentage beside the title.
  const row = rowInput({ matchScore: 87 });
  const message = digestMessage([row], { appBaseUrl: APP });
  assert.ok(message.includes('87% match'), message);
  assert.ok(message.startsWith('Data Analyst'), message);
});

// ---------------------------------------------------------------------------
// The owner's exact line format
// ---------------------------------------------------------------------------

const SOURCE_TWO = 'https://somalijobs.com/jobs/hargeisa/2222/driver';

test('a job block is the numbered line, then the URL in angle brackets, then a blank line', () => {
  const rendered = formatRows(
    [
      rowInput({ url: SOURCE }),
      rowInput({
        jobId: 'job-2',
        title: 'Driver',
        organization: 'NGO',
        location: 'Hargeisa',
        deadline: '2026-01-09',
        url: SOURCE_TWO,
      }),
    ],
    { appBaseUrl: APP, max: 5 },
  );

  // Angle brackets keep Discord from unfurling the source page into a preview card, which
  // otherwise shows every job a second time.
  assert.equal(
    rendered,
    '1. **Data Analyst** — Horn Relief — Mogadishu — closes 2026-12-03\n' +
      `<${SOURCE}>\n\n` +
      '2. **Driver** — NGO — Hargeisa — closes 2026-01-09\n' +
      `<${SOURCE_TWO}>\n`,
  );
  // Nothing around the rows: no header, no footer, no count, no percentage.
  assert.ok(!rendered.includes('%'));
  assert.ok(!rendered.includes('Jobify'));
});

test('a deadline is rendered as a date, and a missing one says so', () => {
  assert.equal(dateOnly('2026-12-03'), '2026-12-03');
  assert.equal(dateOnly('2026-12-03T00:00:00Z'), '2026-12-03');
  assert.equal(
    rowLine({ title: 'Driver', organization: 'NGO', location: 'Baidoa', deadline: '2026-12-03T00:00:00Z' }, 1),
    '1. **Driver** — NGO — Baidoa — closes 2026-12-03',
  );
  assert.equal(
    rowLine({ title: 'Driver', organization: 'NGO', location: 'Baidoa', deadline: null }, 1),
    '1. **Driver** — NGO — Baidoa — no deadline listed',
  );
  // A missing employer keeps its slot rather than shifting the line.
  assert.equal(
    rowLine({ title: 'Driver', organization: '', location: 'Baidoa', deadline: '' }, 1),
    '1. **Driver** — organisation not listed — Baidoa — no deadline listed',
  );
});

test('the same job is printed once even when the caller passes it twice', () => {
  const rows = [
    rowInput({ url: SOURCE }),
    rowInput({ url: SOURCE }),
    rowInput({ jobId: 'job-2', title: 'Driver', url: SOURCE_TWO }),
  ];
  assert.equal(uniqueRows(rows).length, 2);

  const rendered = formatRows(rows, { appBaseUrl: APP });
  assert.equal(rendered.match(/\*\*Data Analyst\*\*/g)?.length, 1);
  assert.equal(rendered.match(/somalijobs\.com/g)?.length, 2, 'one link per surviving row');
  assert.equal((rendered.match(/^\d+\. /gm) ?? []).length, 2, 'numbering never repeats');
});

test('two genuinely different jobs with the same title both survive', () => {
  const rendered = formatRows(
    [
      rowInput({ jobId: 'job-1', organization: 'WADAAG INSURANCE' }),
      rowInput({ jobId: 'job-2', organization: 'DAHABSHIIL MOTORS' }),
    ],
    { appBaseUrl: APP },
  );
  assert.equal(rendered.match(/\*\*Data Analyst\*\*/g)?.length, 2);
});

test('the digest footer points at commands that still exist', () => {
  const message = digestMessage([rowInput()], { appBaseUrl: APP });
  const footer = message.split('\n').at(-1) ?? '';
  assert.ok(footer.includes('`/account`'), footer);
  for (const gone of ['/start', '/alerts', '/stop', '/credits', '/help', '/company']) {
    assert.ok(!footer.includes(gone), `footer still names ${gone}: ${footer}`);
  }
});
