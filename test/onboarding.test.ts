import assert from 'node:assert/strict';
import { test } from 'node:test';

import type { ActionRowBuilder, ButtonBuilder } from 'discord.js';

import {
  CID,
  DEGREE_LEVELS,
  EDUCATION_OPTIONS,
  EXPERIENCE_OPTIONS,
  FIELD_OPTIONS,
  MAX_EDUCATION,
  OnboardingSessions,
  STEP_COUNT,
  anotherPrompt,
  completionPrompt,
  educationEntryLabel,
  educationLabel,
  educationPrompt,
  educationSummary,
  watchingLine,
  experienceLabel,
  experiencePrompt,
  fieldPrompt,
  namePrompt,
  positionPrompt,
} from '../src/onboarding.js';
import type { DiscordUser, MatchRow } from '../src/types.js';

/** The API component union includes SKU buttons, which have no custom_id. */
function buttonId(component: unknown): string | undefined {
  return (component as { custom_id?: string }).custom_id;
}

function user(overrides: Partial<DiscordUser> = {}): DiscordUser {
  return {
    id: 'u1',
    name: 'Sahra Ali',
    email: '',
    role: 'user',
    provider: 'discord',
    email_verified: true,
    location: null,
    match_profile: {
      categories: ['health'],
      subcategories: ['Programme Officer', 'MIS Officer'],
      keywords: ['programme'],
      locations: [],
      category_source: 'llm',
      updated_at: '',
    },
    alerts: { enabled: true, mode: 'daily', last_sent_at: '', user_disabled: false, excluded_employers: [], excluded_categories: [] },
    education_level: 'bachelor',
    education: [{ level: 'bachelor', field: 'Health' }],
    last_position: 'Senior Programme Officer',
    experience_years: 3,
    sectors: ['health'],
    skills: [],
    wants: null,
    ...overrides,
  };
}

function rows(components: ActionRowBuilder<ButtonBuilder>[]): string[][] {
  return components.map((row) =>
    row.toJSON().components.map((button) => (button as { custom_id?: string }).custom_id ?? ''),
  );
}

test('there are four steps and the first one asks for a name', () => {
  assert.equal(STEP_COUNT, 4);
  const prompt = namePrompt();
  const ids = rows(prompt.components);
  assert.deepEqual(ids, [[CID.nameOpen]]);
  assert.ok(prompt.content.includes('Step 1 of 4'));
  assert.ok(prompt.content.includes('no city to pick'), 'the location step is gone for good');
});

test('the education step offers all seven levels across two rows', () => {
  const sessions = new OnboardingSessions();
  const session = sessions.start('1');
  const prompt = educationPrompt(session);
  const ids = rows(prompt.components);
  assert.equal(ids.length, 2);
  assert.equal(ids[0]?.length, 5);
  assert.equal(ids[1]?.length, 2);
  assert.deepEqual(ids.flat(), EDUCATION_OPTIONS.map((option) => CID.education(option.value)));
  assert.ok(prompt.content.includes('Step 2 of 4'));
  assert.ok(!prompt.content.includes('On file'), 'a first run has nothing to replace');
});

test('a re-run shows what is on file and says the next answer replaces it', () => {
  const sessions = new OnboardingSessions();
  const session = sessions.start('1', {
    previousEducation: [
      { level: 'bachelor', field: 'Health' },
      { level: 'master', field: 'Business administration' },
    ],
  });
  const prompt = educationPrompt(session);
  assert.ok(prompt.content.includes('Bachelor — Health'));
  assert.ok(prompt.content.includes('Master — Business administration'));
  assert.ok(prompt.content.includes('replaces'));
});

test('only degree levels are asked for a field, and all thirteen answers are buttons', () => {
  assert.deepEqual(DEGREE_LEVELS, ['bachelor', 'master', 'phd']);
  const prompt = fieldPrompt('master');
  const ids = rows(prompt.components).flat();
  assert.equal(ids.length, FIELD_OPTIONS.length);
  assert.equal(ids.length, 13);
  assert.deepEqual(ids, FIELD_OPTIONS.map((option) => CID.field(option.key)));
  assert.ok(ids.includes(CID.field('other')), 'Other is a button that opens a modal');
  assert.ok(prompt.content.includes('Master'));
});

test('the another-degree question drops the Yes button at the cap', () => {
  const sessions = new OnboardingSessions();
  const session = sessions.start('1');
  session.education = [{ level: 'bachelor', field: 'Health' }];

  const first = anotherPrompt(session);
  assert.deepEqual(rows(first.components).flat(), [CID.another('yes'), CID.another('no')]);

  session.education = Array.from({ length: MAX_EDUCATION }, () => ({ level: 'bachelor', field: 'Health' }));
  const atCap = anotherPrompt(session);
  assert.deepEqual(rows(atCap.components).flat(), [CID.another('no')], 'no fifth degree is offered');
  assert.ok(atCap.content.includes('maximum'));
});

test('the job-history step asks for the position and then the years band', () => {
  const position = positionPrompt();
  assert.deepEqual(rows(position.components).flat(), [CID.positionOpen]);
  assert.ok(position.content.includes('Step 3 of 4'));

  const years = experiencePrompt();
  assert.deepEqual(rows(years.components).flat(), EXPERIENCE_OPTIONS.map((option) => CID.experience(option.value)));
  // Each band stores its LOWER bound: "3–5" is three years, never five.
  assert.deepEqual(EXPERIENCE_OPTIONS.map((option) => option.years), [0, 1, 3, 6, 11]);
});

test('labels render what was actually stored', () => {
  assert.equal(educationLabel('bachelor'), 'Bachelor');
  assert.equal(educationLabel('phd'), 'PhD');
  assert.equal(educationLabel(null), 'not set');
  assert.equal(experienceLabel(3), '3–5 years');
  assert.equal(experienceLabel(0), 'No experience');
  assert.equal(experienceLabel(null), 'not set');
  assert.equal(educationEntryLabel({ level: 'master', field: 'Other: Marine biology' }), 'Master — Other: Marine biology');
  assert.equal(educationEntryLabel({ level: 'diploma', field: '' }), 'Diploma');
  assert.equal(
    educationSummary([
      { level: 'bachelor', field: 'Health' },
      { level: 'master', field: 'Business administration' },
    ]),
    'Bachelor — Health; Master — Business administration',
  );
  assert.equal(educationSummary([]), 'not set');
});

test('a session starts at the name step and is cleared when the interview ends', () => {
  const sessions = new OnboardingSessions();
  const session = sessions.start('1', { previousEducation: [{ level: 'bachelor', field: 'Health' }], name: 'Sahra' });
  assert.equal(sessions.get('1'), session);
  assert.equal(session.step, 'name');
  assert.deepEqual(session.education, [], 'the new run collects its own list');
  assert.equal(session.name, 'Sahra');
  sessions.clear('1');
  assert.equal(sessions.get('1'), undefined);
});

test('the completion message says Setup complete, repeats the answers, shows five matches and points at /jobs', () => {
  const matches: MatchRow[] = Array.from({ length: 7 }, (_, index) => ({
    matchScore: 4 - index * 0.1,
    job: {
      _id: `job-${index + 1}`,
      title: `Programme Officer ${index + 1}`,
      organization: 'WFP',
      location: 'Mogadishu',
      deadline: null,
      category: 'health',
      kind: 'job',
    },
  }));

  const prompt = completionPrompt({
    user: user(),
    matches,
    digestTz: 'Africa/Nairobi',
    digestHour: 8,
    appBaseUrl: 'https://app.jobify.test',
  });

  assert.ok(prompt.content.startsWith('**Setup complete**'));
  assert.ok(prompt.content.includes('Sahra Ali'));
  assert.ok(prompt.content.includes('Bachelor — Health'));
  assert.ok(prompt.content.includes('Senior Programme Officer'));
  assert.ok(prompt.content.includes('3–5 years'));
  assert.ok(prompt.content.includes('Alerts: on, daily at 08:00 (Africa/Nairobi)'));
  // The derived role titles are said out loud: the member never picks a sector, so this is
  // where they see what the backend decided to watch for.
  assert.ok(
    prompt.content.includes('I will watch for Programme Officer and MIS Officer.'),
    prompt.content,
  );
  // Five rows is the digest ceiling, and each one is a real link to the job.
  assert.equal(prompt.content.match(/\/jobs\/job-/g)?.length, 5);
  assert.ok(!prompt.content.includes('Programme Officer 6'), 'the sixth row is not shown');
  assert.ok(prompt.content.includes('`/jobs` lists everything for you at any time'));
  assert.equal(
    (prompt.components[0]?.toJSON().components[0] as { custom_id?: string } | undefined)?.custom_id,
    CID.restart,
  );
});

test('the completion message links each match to its source page, never a local portal', () => {
  const source = 'https://somalijobs.com/jobs/mogadishu/14525213046612058/reach-data-analyst';
  const matches: MatchRow[] = [
    {
      matchScore: 3,
      job: {
        _id: 'job-1',
        title: 'REACH Data Analyst',
        organization: 'ACTED',
        location: 'Mogadishu',
        deadline: null,
        category: 'ict',
        kind: 'job',
        url: source,
      },
    },
    { matchScore: 2, job: { _id: 'job-2', title: 'Driver', organization: 'NGO', location: null, deadline: null, kind: 'job' } },
  ];

  const prompt = completionPrompt({
    user: user(),
    matches,
    digestTz: 'Africa/Nairobi',
    digestHour: 8,
    // The bot's own default: a local portal must not become a link in anyone's DM.
    appBaseUrl: 'http://localhost:5173',
  });

  assert.equal(prompt.content.match(/somalijobs\.com/g)?.length, 1);
  assert.ok(!prompt.content.includes('localhost'), prompt.content);
});

test('the completion message is honest when nothing matches or alerts are off', () => {
  const prompt = completionPrompt({
    user: user({
      match_profile: null,
      alerts: {
        enabled: false,
        mode: 'daily',
        last_sent_at: '',
        user_disabled: true,
        excluded_employers: [],
        excluded_categories: [],
      },
    }),
    matches: [],
    digestTz: 'Africa/Nairobi',
    digestHour: 8,
    appBaseUrl: 'https://app.jobify.test',
  });

  assert.ok(prompt.content.includes('Nothing live matches your answers this minute'));
  assert.ok(prompt.content.includes('Alerts: off'), 'an explicit /stop is reported as off, not silently overridden');
  assert.ok(!prompt.content.includes('/jobs/job-'));
  assert.ok(!prompt.content.includes('I will watch for'), 'no role line when the backend decided nothing');
});

test('the watching line names the derived role titles, and degrades rather than lying', () => {
  // The normal case: the backend's derivation produced role titles.
  assert.equal(
    watchingLine({
      categories: ['ict'],
      subcategories: ['Data Analyst', 'MIS Officer', 'Programme Officer'],
      keywords: [],
      locations: [],
      category_source: 'llm',
      updated_at: '',
    }),
    'I will watch for Data Analyst, MIS Officer and Programme Officer.',
  );
  // The rules fallback: the model was down, so only categories were stored. Say what is
  // actually known instead of naming roles nobody derived.
  assert.equal(
    watchingLine({
      categories: ['ict', 'business_and_finance'],
      subcategories: [],
      keywords: [],
      locations: [],
      category_source: 'rules',
      updated_at: '',
    }),
    'I will watch for roles in ict and business_and_finance.',
  );
  // Nothing decided at all (e.g. a degree the map does not know): no line, not an empty list.
  assert.equal(
    watchingLine({
      categories: [],
      subcategories: [],
      keywords: [],
      locations: [],
      category_source: 'rules',
      updated_at: '',
    }),
    null,
  );
  assert.equal(watchingLine(null), null);
});

test('the completion message still completes when the subcategories are absent', () => {
  const prompt = completionPrompt({
    user: user({
      match_profile: {
        categories: ['health'],
        subcategories: [],
        keywords: ['programme'],
        locations: [],
        category_source: 'rules',
        updated_at: '',
      },
    }),
    matches: [],
    digestTz: 'Africa/Nairobi',
    digestHour: 8,
    appBaseUrl: 'https://app.jobify.test',
  });

  assert.ok(prompt.content.startsWith('**Setup complete**'));
  assert.ok(prompt.content.includes('I will watch for roles in health.'));
  assert.ok(!prompt.content.includes('I will watch for .'), 'no empty role list is printed');
});
