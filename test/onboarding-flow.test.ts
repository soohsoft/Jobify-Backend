/**
 * The interview, driven as a member drives it: fake interactions against a fake backend.
 *
 * These are the "click a button" tests. They cover the four steps, the two-degree path, the
 * "Other" free-text path, resumability and the alert rule, and they assert on what the bot
 * WRITES (the patch it sends) as well as on what it SAYS (the next prompt / the completion
 * message) — a prompt that advances without a save would look identical in a screenshot.
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';

import type { ButtonInteraction, ModalSubmitInteraction } from 'discord.js';

import { RequestGuard, type BotContext } from '../src/context.js';
import type { Backend } from '../src/http.js';
import type { Logger } from '../src/log.js';
import { handleInteraction } from '../src/interactions.js';
import { CID, MAX_EDUCATION, OnboardingSessions } from '../src/onboarding.js';
import type { DiscordUser, MatchRow, ProfilePatch, UserEnvelope } from '../src/types.js';

const MEMBER = '900000000000000901';
const APP = 'https://app.jobify.test';

const silentLogger: Logger = {
  debug: () => undefined,
  info: () => undefined,
  warn: () => undefined,
  error: () => undefined,
};

/** Five real-shaped rows; the digest ceiling is five, so a sixth is never rendered. */
const matchRows: MatchRow[] = Array.from({ length: 6 }, (_, index) => ({
  matchScore: 4 - index * 0.1,
  job: {
    _id: `job-${index + 1}`,
    title: `Programme Officer ${index + 1}`,
    organization: 'WFP',
    location: 'Mogadishu',
    deadline: '2026-12-31',
    category: 'health',
    kind: 'job',
  },
}));

interface Harness {
  ctx: BotContext;
  sessions: OnboardingSessions;
  user: DiscordUser;
  calls: { saves: ProfilePatch[]; alerts: Array<{ enabled: boolean; mode?: string }>; matches: number };
}

function harness(
  options: { user?: Partial<DiscordUser>; matches?: MatchRow[] } = {},
): Harness {
  const user: DiscordUser = {
    id: 'u1',
    name: 'Discord user',
    email: '',
    role: 'user',
    provider: 'discord',
    email_verified: true,
    location: null,
    match_profile: null,
    alerts: null,
    education_level: null,
    education: [],
    last_position: null,
    experience_years: null,
    sectors: [],
    skills: [],
    wants: null,
    ...options.user,
  };
  const calls = { saves: [] as ProfilePatch[], alerts: [] as Array<{ enabled: boolean; mode?: string }>, matches: 0 };

  const backend = {
    saveProfile: async (patch: ProfilePatch) => {
      calls.saves.push(patch);
      if (patch.name !== undefined) user.name = patch.name;
      if (patch.education !== undefined) user.education = patch.education;
      if (patch.last_position !== undefined) user.last_position = patch.last_position;
      if (patch.experience_years !== undefined) user.experience_years = patch.experience_years;

      // The backend's rules, reproduced so the completion copy is read off real state:
      // name + one education entry completes it, and completing enables alerts unless the
      // member already said /stop (alerts.user_disabled).
      const alerts = (user.alerts ??= {
        enabled: false,
        mode: '',
        last_sent_at: '',
        user_disabled: false,
        excluded_employers: [],
        excluded_categories: [],
      });
      const complete = user.name.trim().length > 0 && user.education.length > 0;
      if (complete && !alerts.user_disabled && !alerts.enabled) {
        alerts.enabled = true;
        alerts.mode = 'daily';
      }
      // What the backend decides at completion. The bot only ever READS this: it must never
      // derive categories of its own, so the fixture makes the decision here
      // exactly where the real backend does.
      if (complete && user.match_profile === null) {
        user.match_profile = {
          categories: ['health'],
          keywords: ['programme'],
          locations: [],
          category_source: 'llm',
          updated_at: '',
        };
      }
      return { user, onboarding_complete: complete, alerts_enabled: user.alerts.enabled };
    },
    getUser: async (): Promise<UserEnvelope> => ({
      user,
      subscription_status: 'free',
      onboarding_complete: user.name.trim().length > 0 && user.education.length > 0,
      alerts_enabled: user.alerts?.enabled ?? false,
      alerts: user.alerts ?? {
        enabled: false,
        mode: '',
        last_sent_at: '',
        user_disabled: false,
        excluded_employers: [],
        excluded_categories: [],
      },
      credits: { balance: 0, monthly_allowance: 30, used_this_period: 0, period_end: '', remaining_this_period: 30 },
    }),
    jobsForUser: async () => {
      calls.matches += 1;
      const rows = options.matches ?? matchRows;
      return {
        rows,
        meta: {
          reason: user.match_profile ? 'ok' : 'no_match_profile',
          total: rows.length,
          categories: user.match_profile?.categories ?? [],
          categorySource: user.match_profile?.category_source ?? '',
        },
      };
    },
    setAlerts: async (_discordId: string, enabled: boolean, mode?: string) => {
      calls.alerts.push({ enabled, ...(mode ? { mode } : {}) });
      const alerts = user.alerts;
      if (!alerts) throw new Error('no alerts to update in this fixture');
      alerts.enabled = enabled;
      alerts.user_disabled = !enabled;
      if (mode) alerts.mode = mode;
      return { alerts, alerts_enabled: enabled };
    },
    upsertUser: async () => ({ user, created: false, onboarding_complete: false }),
    mintLink: async () => ({ url: 'https://app.jobify.test/enter?t=x', token: 'x', expires_at: '', expires_in_seconds: 900 }),
    setCompany: async () => ({ user, company_set: false }),
    assistantQuery: async () => {
      throw new Error('the interview must never call the assistant');
    },
  };

  const sessions = new OnboardingSessions();
  const ctx: BotContext = {
    backend: backend as unknown as Backend,
    appBaseUrl: APP,
    digestTz: 'Africa/Nairobi',
    digestHour: 8,
    logger: silentLogger,
    sessions,
    categories: async () => [],
    sendDm: async () => undefined,
    guard: new RequestGuard(),
  };

  return { ctx, sessions, user, calls };
}

// ---------------------------------------------------------------------------
// Fake interactions: only the surface the handlers touch
// ---------------------------------------------------------------------------

interface Rendered {
  content: string;
  components?: Array<{ toJSON(): { components: Array<{ custom_id?: string; disabled?: boolean }> } }>;
}

function renderedContent(payload: unknown): string {
  if (typeof payload === 'string') return payload;
  return (payload as { content?: string }).content ?? '';
}

function renders(payloads: unknown[]): string {
  return payloads.map(renderedContent).join('\n---\n');
}

function buttonIds(payload: Rendered | undefined): string[] {
  return (payload?.components ?? []).flatMap((row) =>
    row.toJSON().components.map((component) => component.custom_id ?? ''),
  );
}

function fakeButton(customId: string, userId = MEMBER): {
  interaction: ButtonInteraction;
  updates: unknown[];
  edits: unknown[];
  replies: unknown[];
  modals: string[];
} {
  const updates: unknown[] = [];
  const edits: unknown[] = [];
  const replies: unknown[] = [];
  const modals: string[] = [];
  const interaction = {
    customId,
    user: { id: userId, username: 'sahra', globalName: 'Sahra' },
    replied: false,
    deferred: false,
    isButton: () => true,
    isChatInputCommand: () => false,
    isStringSelectMenu: () => false,
    isModalSubmit: () => false,
    deferUpdate: async () => {
      interaction.deferred = true;
    },
    deferReply: async () => {
      interaction.deferred = true;
    },
    update: async (payload: unknown) => {
      updates.push(payload);
    },
    editReply: async (payload: unknown) => {
      edits.push(payload);
    },
    reply: async (payload: unknown) => {
      replies.push(payload);
    },
    followUp: async (payload: unknown) => {
      replies.push(payload);
    },
    showModal: async (modal: { toJSON(): { custom_id: string } }) => {
      modals.push(modal.toJSON().custom_id);
    },
  };
  return { interaction: interaction as unknown as ButtonInteraction, updates, edits, replies, modals };
}

function fakeModal(customId: string, fields: Record<string, string>, userId = MEMBER): {
  interaction: ModalSubmitInteraction;
  edits: unknown[];
  replies: unknown[];
} {
  const edits: unknown[] = [];
  const replies: unknown[] = [];
  const interaction = {
    customId,
    user: { id: userId, username: 'sahra', globalName: 'Sahra' },
    replied: false,
    deferred: false,
    isButton: () => false,
    isChatInputCommand: () => false,
    isStringSelectMenu: () => false,
    isModalSubmit: () => true,
    isFromMessage: () => true,
    fields: {
      getTextInputValue: (name: string) => {
        if (!(name in fields)) throw new Error(`unexpected field ${name}`);
        return fields[name];
      },
    },
    deferUpdate: async () => {
      interaction.deferred = true;
    },
    deferReply: async () => {
      interaction.deferred = true;
    },
    editReply: async (payload: unknown) => {
      edits.push(payload);
    },
    reply: async (payload: unknown) => {
      replies.push(payload);
    },
    followUp: async (payload: unknown) => {
      replies.push(payload);
    },
  };
  return { interaction: interaction as unknown as ModalSubmitInteraction, edits, replies };
}

async function click(ctx: BotContext, customId: string): Promise<ReturnType<typeof fakeButton>> {
  const click = fakeButton(customId);
  await handleInteraction(click.interaction, ctx);
  return click;
}

async function submit(
  ctx: BotContext,
  customId: string,
  fields: Record<string, string>,
): Promise<{ interaction: ModalSubmitInteraction; edits: unknown[]; replies: unknown[] }> {
  const submission = fakeModal(customId, fields);
  await handleInteraction(submission.interaction, ctx);
  return submission;
}

function startSession(ctx: BotContext): void {
  ctx.sessions.start(MEMBER, { previousEducation: [] });
}

// ---------------------------------------------------------------------------
// The happy path
// ---------------------------------------------------------------------------

test('the happy path: name → bachelor Health → no second degree → position → years → Setup complete + matches', async () => {
  const { ctx, sessions, user, calls } = harness();
  startSession(ctx);

  // Step 1 — the name button only opens a modal; nothing is written yet.
  const nameButton = await click(ctx, CID.nameOpen);
  assert.deepEqual(nameButton.modals, [CID.nameModal]);
  assert.equal(calls.saves.length, 0);

  const name = await submit(ctx, CID.nameModal, { name: ' Sahra Ali ' });
  assert.deepEqual(calls.saves[0], { discord_id: MEMBER, name: 'Sahra Ali' }, 'the name is written; the trim is not');
  assert.ok(renders(name.edits).includes('Step 2 of 4'));

  // Step 2 — a degree level opens the field question and is NOT saved on its own.
  const level = await click(ctx, CID.education('bachelor'));
  assert.ok(renders(level.updates).includes('Which field is your Bachelor in'));
  assert.equal(calls.saves.length, 1, 'a level alone is not a degree');
  assert.equal(sessions.get(MEMBER)?.step, 'education_field');
  assert.equal(sessions.get(MEMBER)?.pendingLevel, 'bachelor');

  const field = await click(ctx, CID.field('health'));
  assert.deepEqual(
    calls.saves[1],
    { discord_id: MEMBER, education: [{ level: 'bachelor', field: 'Health' }] },
    'the whole list is sent, with the canonical label the backend maps to a category',
  );
  assert.ok(renders(field.edits).includes('Do you have another degree?'));

  const noSecond = await click(ctx, CID.another('no'));
  assert.ok(renders(noSecond.edits).includes('What was your last job?'));
  assert.equal(sessions.get(MEMBER)?.step, 'position');

  // Step 3 — the position modal, then the years band.
  const positionButton = await click(ctx, CID.positionOpen);
  assert.deepEqual(positionButton.modals, [CID.positionModal]);

  const position = await submit(ctx, CID.positionModal, { position: 'Senior Programme Officer' });
  assert.deepEqual(calls.saves[2], { discord_id: MEMBER, last_position: 'Senior Programme Officer' });
  assert.ok(renders(position.edits).includes('How much relevant work experience'));

  const years = await click(ctx, CID.experience('3-5'));
  assert.deepEqual(calls.saves[3], { discord_id: MEMBER, experience_years: 3 });

  // Step 4 — Setup complete, the first digest, and the /jobs hint.
  const completion = renders(years.edits);
  assert.ok(completion.startsWith('**Setup complete**'), completion);
  assert.ok(completion.includes('Sahra Ali'));
  assert.ok(completion.includes('Bachelor — Health'));
  assert.ok(completion.includes('Senior Programme Officer'));
  assert.ok(completion.includes('3–5 years'));
  assert.equal(completion.match(/\/jobs\/job-/g)?.length, 5, 'five rows, the digest ceiling');
  assert.ok(completion.includes('**Jobs** and **Tenders** below open the full list'));
  assert.ok(completion.includes('Alerts: on'));

  assert.equal(calls.matches, 1);
  assert.equal(sessions.get(MEMBER), undefined, 'the interview is over');
  assert.equal(user.education.length, 1);
  assert.equal(user.last_position, 'Senior Programme Officer');
});

test('two degrees accumulate in one list and both fields reach the backend', async () => {
  const { ctx, user, calls } = harness();
  startSession(ctx);
  await submit(ctx, CID.nameModal, { name: 'Sahra Ali' });

  await click(ctx, CID.education('bachelor'));
  await click(ctx, CID.field('health'));
  await click(ctx, CID.another('yes'));
  await click(ctx, CID.education('master'));
  await click(ctx, CID.field('business'));
  const last = await click(ctx, CID.another('no'));

  const educationSaves = calls.saves.filter((save) => save.education !== undefined);
  assert.deepEqual(educationSaves.map((save) => save.education), [
    [{ level: 'bachelor', field: 'Health' }],
    [
      { level: 'bachelor', field: 'Health' },
      { level: 'master', field: 'Business administration' },
    ],
  ]);
  assert.equal(user.education.length, 2, 'the second save replaces the list with both degrees');
  assert.equal(user.education[1]?.field, 'Business administration');
  assert.ok(renders(last.edits).includes('What was your last job?'));
});

test('"Other" goes to keywords as free text, never to a category button label', async () => {
  const { ctx, user, calls } = harness();
  startSession(ctx);
  await submit(ctx, CID.nameModal, { name: 'Sahra Ali' });

  await click(ctx, CID.education('phd'));
  const other = await click(ctx, CID.field('other'));
  assert.deepEqual(other.modals, [CID.fieldModal], 'Other opens a text box');
  assert.equal(calls.saves.length, 1, 'nothing is saved until the words exist');

  const detail = await submit(ctx, CID.fieldModal, { field: ' Marine biology ' });
  assert.deepEqual(calls.saves[1], {
    discord_id: MEMBER,
    education: [{ level: 'phd', field: 'Other: Marine biology' }],
  });
  assert.equal(user.education[0]?.field, 'Other: Marine biology');
  assert.ok(renders(detail.edits).includes('Do you have another degree?'));
});

test('the cap is four degrees: at four, “Yes” is not offered and the interview moves on', async () => {
  const { ctx, sessions } = harness();
  startSession(ctx);
  const session = sessions.get(MEMBER);
  assert.ok(session);
  session.education = Array.from({ length: MAX_EDUCATION }, () => ({ level: 'bachelor', field: 'Health' }));
  session.step = 'another_degree';

  await click(ctx, CID.another('yes'));
  assert.equal(sessions.get(MEMBER)?.step, 'position', 'a fifth degree is never started');
});

// ---------------------------------------------------------------------------
// Resumability and expiry
// ---------------------------------------------------------------------------

test('an answer given mid-interview is saved, and the session keeps its place', async () => {
  const { ctx, sessions, calls } = harness();
  startSession(ctx);
  await submit(ctx, CID.nameModal, { name: 'Sahra Ali' });

  const session = sessions.get(MEMBER);
  assert.ok(session);
  assert.equal(session.step, 'education_level');
  assert.equal(session.name, 'Sahra Ali', 'the session remembers the name it just wrote');

  // A second button from the same (old) message still works, because the session is alive.
  await click(ctx, CID.education('diploma'));
  assert.deepEqual(calls.saves.at(-1), {
    discord_id: MEMBER,
    education: [{ level: 'diploma', field: '' }],
  });
  assert.equal(sessions.get(MEMBER)?.step, 'another_degree');
});

test('a click with no session says how to resume and writes nothing', async () => {
  const { ctx, calls } = harness();
  const clickResult = await click(ctx, CID.education('bachelor'));
  assert.deepEqual(clickResult.updates, []);
  assert.equal(calls.saves.length, 0);
  assert.ok(renders(clickResult.replies).includes('Send me a message'));
  assert.ok(!renders(clickResult.replies).includes('/start'), 'no removed command in the copy');
});

// ---------------------------------------------------------------------------
// Alerts
// ---------------------------------------------------------------------------

test('completion never re-enables alerts: the bot makes no setAlerts call at all', async () => {
  const { ctx, calls } = harness();
  startSession(ctx);
  await submit(ctx, CID.nameModal, { name: 'Sahra Ali' });
  await click(ctx, CID.education('bachelor'));
  await click(ctx, CID.field('health'));
  await click(ctx, CID.another('no'));
  await submit(ctx, CID.positionModal, { position: 'Programme Officer' });
  await click(ctx, CID.experience('1-2'));

  assert.deepEqual(calls.alerts, [], 'the backend owns the enable rule, so /stop survives');
  assert.equal(calls.matches, 1);
});

test('a member who said /stop is shown alerts off after finishing', async () => {
  const { ctx } = harness({
    user: {
      alerts: {
        enabled: false,
        mode: 'daily',
        last_sent_at: '2026-11-24T08:00:00Z',
        user_disabled: true,
        excluded_employers: [],
        excluded_categories: [],
      },
    },
  });
  startSession(ctx);
  await submit(ctx, CID.nameModal, { name: 'Sahra Ali' });
  await click(ctx, CID.education('bachelor'));
  await click(ctx, CID.field('health'));
  await click(ctx, CID.another('no'));
  await submit(ctx, CID.positionModal, { position: 'Programme Officer' });
  const years = await click(ctx, CID.experience('6-10'));

  const completion = renders(years.edits);
  assert.ok(completion.startsWith('**Setup complete**'));
  assert.ok(completion.includes('Alerts: off'), completion);
  assert.ok(completion.includes('`/account`'), 'and tells them where to switch them back on');
});

test('when the match read fails the setup still completes', async () => {
  const { ctx, calls } = harness();
  // The for-user read throws, as it would if the backend were mid-deploy.
  (ctx.backend as unknown as { jobsForUser: () => Promise<unknown> }).jobsForUser = async () => {
    calls.matches += 1;
    throw new Error('404');
  };
  startSession(ctx);
  await submit(ctx, CID.nameModal, { name: 'Sahra Ali' });
  await click(ctx, CID.education('bachelor'));
  await click(ctx, CID.field('health'));
  await click(ctx, CID.another('no'));
  await submit(ctx, CID.positionModal, { position: 'Programme Officer' });
  const years = await click(ctx, CID.experience('0'));

  const completion = renders(years.edits);
  assert.ok(completion.startsWith('**Setup complete**'));
  assert.ok(completion.includes('Nothing live matches your answers this minute'));
  // The fallback copy ends on the same invitation: the two tabs sit right under it.
  assert.ok(
    completion.includes('**Jobs** and **Tenders** below open the full list'),
    'the fallback path still offers the two tabs',
  );
});
