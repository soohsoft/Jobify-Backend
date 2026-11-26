/**
 * The two questions asked after the last job: where to work, and which positions to be
 * notified about. Both are select menus, so the tests read the components as Discord renders
 * them (`toJSON`) rather than trusting the builder call.
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';

import { RequestGuard, type BotContext } from '../src/context.js';
import type { Backend } from '../src/http.js';
import type { Logger } from '../src/log.js';
import { handleInteraction } from '../src/interactions.js';
import {
  CID,
  OnboardingSessions,
  POSITION_CHOICES,
  employerPrompt,
  positionsPrompt,
} from '../src/onboarding.js';
import type { DiscordUser, ProfilePatch, UserEnvelope } from '../src/types.js';

const MEMBER = '900000000000000901';

function optionsOf(payload: { components?: unknown[] }, row = 0): Array<{ label: string; value: string }> {
  const rows = (payload.components ?? []) as Array<{ toJSON(): { components: Array<{ options?: Array<{ label: string; value: string }> }> } }>;
  return rows[row]?.toJSON().components[0]?.options ?? [];
}

function buttonIds(payload: { components?: unknown[] }): string[] {
  const rows = (payload.components ?? []) as Array<{ toJSON(): { components: Array<{ custom_id?: string }> } }>;
  return rows.flatMap((row) =>
    row.toJSON().components.map((component) => component.custom_id ?? ''),
  );
}

test('the employer question offers exactly the three owner types, as one multi-select', () => {
  const prompt = employerPrompt();
  assert.ok(prompt.content.includes('Where would you like to work?'));
  assert.equal(prompt.hideTabs, true, 'no Jobs/Tenders tabs mid-interview');
  assert.deepEqual(
    optionsOf(prompt).map((option) => option.value),
    ['ngo', 'government', 'private'],
  );
  assert.deepEqual(
    optionsOf(prompt).map((option) => option.label),
    ['Non-Profit Organisations', 'Government', 'Private Companies'],
  );
});

test('the position picker is paged 14 at a time and covers all 28 categories', () => {
  assert.equal(POSITION_CHOICES.length, 28);

  const session = { positions: [] } as unknown as Parameters<typeof positionsPrompt>[0];
  const first = positionsPrompt(session, 0);
  const second = positionsPrompt(session, 1);

  assert.equal(optionsOf(first).length, 14, 'Discord caps a select at 25 options');
  assert.equal(optionsOf(second).length, 14);
  assert.equal(optionsOf(first)[0].value, 'health');
  assert.equal(optionsOf(second)[13].value, 'wash');

  // Page 1 offers Next, All and Done; the last page drops Next.
  assert.ok(buttonIds(first).includes('onb:positions:page:1'));
  assert.ok(buttonIds(first).includes(CID.positionsAll));
  assert.ok(buttonIds(first).includes(CID.positionsDone));
  assert.ok(buttonIds(second).includes('onb:positions:page:0'));
  assert.ok(!buttonIds(second).includes('onb:positions:page:2'));
});

test('a pick is remembered and shown above the menu', () => {
  const session = { positions: ['health', 'wash'] } as unknown as Parameters<typeof positionsPrompt>[0];
  const prompt = positionsPrompt(session, 0);
  assert.ok(prompt.content.includes('Health'));
  assert.ok(prompt.content.includes('WASH'));
});

function harness(): {
  ctx: BotContext;
  sessions: OnboardingSessions;
  saves: ProfilePatch[];
} {
  const sessions = new OnboardingSessions();
  const saves: ProfilePatch[] = [];
  const user = {
    discord_id: MEMBER,
    name: 'Sahra Ali',
    email: null,
    education_level: 'bachelor',
    education: [],
    last_position: 'Programme Officer',
    experience_years: 3,
    location: null,
    company: null,
    match_profile: null,
    alerts_enabled: true,
    onboarding_complete: true,
    created_at: '',
    updated_at: '',
  } as unknown as DiscordUser;

  const backend = {
    saveProfile: async (patch: ProfilePatch) => {
      saves.push(patch);
      return { user, onboarding_complete: true, alerts_enabled: true };
    },
    getUser: async (): Promise<UserEnvelope> => ({ user } as unknown as UserEnvelope),
    jobsForUser: async () => ({ rows: [], meta: { total: 0, limit: 5, offset: 0, kind: 'job' } }),
  } as unknown as Backend;

  const logger = { info() {}, warn() {}, error() {} } as unknown as Logger;
  const ctx = {
    backend,
    sessions,
    logger,
    guard: new RequestGuard(),
    digestTz: 'Africa/Nairobi',
    digestHour: 8,
    appBaseUrl: 'https://jobify.soohsoft.com',
  } as unknown as BotContext;
  return { ctx, sessions, saves };
}

function select(customId: string, values: string[]) {
  const updates: unknown[] = [];
  const interaction = {
    customId,
    values,
    user: { id: MEMBER, username: 'sahra', globalName: 'Sahra' },
    replied: false,
    deferred: false,
    isButton: () => false,
    isChatInputCommand: () => false,
    isStringSelectMenu: () => true,
    isModalSubmit: () => false,
    deferUpdate: async () => {
      interaction.deferred = true;
    },
    update: async (payload: unknown) => {
      updates.push(payload);
    },
    editReply: async (payload: unknown) => {
      updates.push(payload);
    },
    reply: async (payload: unknown) => {
      updates.push(payload);
    },
  };
  return { interaction, updates };
}

test('choosing an employer type moves straight on, and a pick on one page survives the next', async () => {
  const { ctx, sessions } = harness();
  const session = sessions.start(MEMBER, {});
  session.step = 'employer_types';

  const employer = select(CID.employer, ['government']);
  await handleInteraction(employer.interaction as never, ctx);
  assert.deepEqual(sessions.get(MEMBER)?.employerTypes, ['government']);
  assert.equal(sessions.get(MEMBER)?.step, 'positions');
  assert.ok(
    (employer.updates[0] as { content: string }).content.includes('Which positions do you want'),
  );

  // Page 1 → health, then page 2 → wash: both stay picked.
  const pageOne = select(CID.positions(0), ['health']);
  await handleInteraction(pageOne.interaction as never, ctx);
  const pageTwo = select(CID.positions(1), ['wash']);
  await handleInteraction(pageTwo.interaction as never, ctx);
  assert.deepEqual(sessions.get(MEMBER)?.positions, ['health', 'wash']);

  // A re-pick on page 1 must not drop the page-2 choice.
  const again = select(CID.positions(0), ['ict']);
  await handleInteraction(again.interaction as never, ctx);
  assert.deepEqual(sessions.get(MEMBER)?.positions, ['health', 'wash', 'ict']);
});
