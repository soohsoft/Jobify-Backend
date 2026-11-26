/**
 * The two tabs on the setup-complete message.
 *
 * They are the only buttons in the bot that must work with no interview in progress and no
 * session in memory — the setup message outlives the process that sent it, and a member
 * presses "Tenders" weeks later. That makes the routing order in `handleButton` load-bearing:
 * if the tab ids ever fall through to the session check, every press answers "that step has
 * expired" instead of a list. These tests pin that.
 */
import assert from 'node:assert/strict';
import test from 'node:test';

import { RequestGuard } from '../src/context.js';
import type { BotContext } from '../src/context.js';
import type { Backend } from '../src/http.js';
import { handleInteraction } from '../src/interactions.js';
import { OnboardingSessions } from '../src/onboarding.js';
import { CID } from '../src/onboarding.js';

const silentLogger = {
  info: () => undefined,
  warn: () => undefined,
  error: () => undefined,
  debug: () => undefined,
} as unknown as BotContext['logger'];

interface Fake {
  content?: string;
  components?: unknown[];
  deferred?: boolean;
  followedUp?: string;
}

/** An onboarded member whose company profile is still missing — the tender tab's hard case. */
function makeCtx(options: { company?: boolean; rows?: unknown[]; blankProfile?: boolean } = {}): {
  ctx: BotContext;
  calls: { kinds: string[] };
} {
  const calls = { kinds: [] as string[] };
  const backend = {
    getUser: async () => ({
      user: {
        name: 'Dayib',
        categories: ['ict'],
        // The browse reply now gates on the profile before it searches, so the fixture has to
        // look like a real one: `blankProfile` is the empty case the gate exists for.
        match_profile: options.blankProfile
          ? { categories: [], keywords: [], locations: [], category_source: '', updated_at: '2026-11-26T00:00:00Z' }
          : {
              categories: ['ict'],
              keywords: ['Rust'],
              locations: [],
              category_source: 'llm',
              updated_at: '2026-11-26T00:00:00Z',
            },
        company: options.company === false ? null : { name: 'SOOHSOFT LTD', services: ['networking'] },
      },
    }),
    jobsForUser: async (_id: string, opts: { kind: string }) => {
      calls.kinds.push(opts.kind);
      return {
        rows: options.rows ?? [],
        meta: { reason: 'no_candidates', total: 0, categories: ['ict'], categorySource: 'llm', threshold: 0 },
      };
    },
  };
  const ctx = {
    backend: backend as unknown as Backend,
    appBaseUrl: 'https://jobs.example',
    digestTz: 'Africa/Nairobi',
    digestHour: 8,
    logger: silentLogger,
    // No session at all: pressing a tab must not need one.
    sessions: new OnboardingSessions(),
    categories: { get: async () => [] },
    sendDm: async () => undefined,
    guard: new RequestGuard(),
  } as unknown as BotContext;
  return { ctx, calls };
}

function makeButton(customId: string): { interaction: never; seen: Fake } {
  const seen: Fake = {};
  const interaction = {
    isChatInputCommand: () => false,
    isButton: () => true,
    isStringSelectMenu: () => false,
    isModalSubmit: () => false,
    customId,
    user: { id: '1432771294968942724' },
    deferred: false,
    replied: false,
    deferReply: async () => {
      seen.deferred = true;
    },
    editReply: async (payload: { content?: string; components?: unknown[] }) => {
      seen.content = payload.content;
      seen.components = payload.components;
    },
    reply: async (payload: { content?: string }) => {
      seen.content = payload.content;
    },
    followUp: async (payload: { content?: string }) => {
      seen.followedUp = payload.content;
    },
    showModal: async () => {
      throw new Error('a tab must never open a modal');
    },
  };
  return { interaction: interaction as unknown as never, seen };
}

test('the Jobs tab answers with the job list, with no interview session', async () => {
  const { ctx, calls } = makeCtx();
  const { interaction, seen } = makeButton(CID.browseJobs);

  await handleInteraction(interaction, ctx);

  assert.deepEqual(calls.kinds, ['job'], 'the tab asks the matcher for jobs');
  assert.equal(seen.deferred, true, 'the press is acknowledged before the backend call');
  assert.ok(seen.content?.includes('Nothing live in your fields right now (ict).'), seen.content);
  assert.ok(!seen.followedUp?.includes('expired'), 'a tab is not an interview step');
});

test('a blank profile starts the interview instead of searching', async () => {
  const { ctx, calls } = makeCtx({ blankProfile: true });
  const { interaction, seen } = makeButton(CID.browseJobs);

  await handleInteraction(interaction, ctx);

  assert.deepEqual(calls.kinds, [], 'nothing is searched before a profile exists');
  assert.ok(seen.content?.includes('No work area on file yet'), seen.content);
  assert.ok(seen.content?.includes('What is your name?'), 'the first question is asked, not promised');
  const customIds = (seen.components ?? []).map(
    (row) =>
      (row as { toJSON: () => { components: { custom_id: string }[] } }).toJSON().components[0]?.custom_id,
  );
  // Row 0 is the tab pair; the interview's own row sits under it.
  assert.deepEqual(customIds, ['onb:browse:job', 'onb:name'], 'the first step is live, no extra press needed');
});

test('the Tenders tab asks for a company profile instead of a CV list', async () => {
  const { ctx, calls } = makeCtx({ company: false });
  const { interaction, seen } = makeButton(CID.browseTenders);

  await handleInteraction(interaction, ctx);

  assert.ok(seen.content?.includes('Tenders are matched to a company, not a CV.'), seen.content);
  // The profile prompt, not an empty tender list: nothing is even asked of the matcher.
  assert.deepEqual(calls.kinds, [], 'no tender query runs before a company exists');
  // Row 0 is always the tab pair; the profile prompt sits under it.
  const customIds = (seen.components ?? []).map(
    (row) =>
      (row as { toJSON: () => { components: { custom_id: string }[] } }).toJSON().components[0]?.custom_id,
  );
  assert.deepEqual(customIds, ['onb:browse:job', 'company:open']);
});

test('the Tenders tab lists tenders when the company profile exists', async () => {
  const { ctx, calls } = makeCtx({ company: true });
  const { interaction, seen } = makeButton(CID.browseTenders);

  await handleInteraction(interaction, ctx);

  assert.deepEqual(calls.kinds, ['tender'], 'the tender corpus is queried, not the job one');
  assert.ok(seen.content?.includes('No tenders are live right now.'), seen.content);
});
