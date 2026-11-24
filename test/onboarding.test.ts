import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  CID,
  EXPERIENCE_OPTIONS,
  OnboardingSessions,
  SECTOR_OPTIONS_PER_PAGE,
  STEP_COUNT,
  educationPrompt,
  experienceLabel,
  experiencePrompt,
  sectorPage,
  sectorsPrompt,
  summaryPrompt,
  wantsLabel,
  wantsPrompt,
} from '../src/onboarding.js';
import type { Category, DiscordUser } from '../src/types.js';

const categories: Category[] = Array.from({ length: 28 }, (_, index) => ({
  slug: `sector_${index + 1}`,
  label: `Sector ${index + 1}`,
  group: 'all_fields',
}));

/** The API component union includes SKU buttons, which have no custom_id. */
function buttonId(component: unknown): string | undefined {
  return (component as { custom_id?: string }).custom_id;
}

function user(overrides: Partial<DiscordUser> = {}): DiscordUser {
  return {
    id: 'u1',
    name: 'Sahra',
    email: '',
    role: 'user',
    provider: 'discord',
    email_verified: true,
    location: 'Mogadishu',
    match_profile: { categories: ['ict'], keywords: ['python'], locations: ['Mogadishu'], updated_at: '' },
    alerts: { enabled: true, mode: 'daily', last_sent_at: '', excluded_employers: [], excluded_categories: [] },
    education_level: 'bachelor',
    experience_years: 3,
    sectors: ['ict'],
    skills: ['python'],
    wants: 'jobs',
    ...overrides,
  };
}

test('the education step offers all seven answers across two rows', () => {
  const prompt = educationPrompt();
  const rows = prompt.components.map((row) => row.toJSON().components);
  assert.equal(rows.length, 2);
  assert.equal(rows[0]?.length, 5);
  assert.equal(rows[1]?.length, 2);
  const ids = rows.flat().map(buttonId);
  assert.deepEqual(ids, [
    CID.education('none'),
    CID.education('primary'),
    CID.education('secondary'),
    CID.education('diploma'),
    CID.education('bachelor'),
    CID.education('master'),
    CID.education('phd'),
  ]);
  assert.ok(prompt.content.includes(`Step 1 of ${STEP_COUNT}`));
});

test('the experience step maps each band to its lower bound in years', () => {
  const years = EXPERIENCE_OPTIONS.map((option) => option.years);
  assert.deepEqual(years, [0, 1, 3, 6, 11]);
  const prompt = experiencePrompt();
  const buttons = prompt.components[0]?.toJSON().components ?? [];
  assert.equal(buttons.length, 5);
  assert.equal(buttonId(buttons[2]), CID.experience('3-5'));
});

test('28 sectors page as 25 + 3, and the page number is clamped', () => {
  assert.equal(SECTOR_OPTIONS_PER_PAGE, 25);
  const first = sectorPage(categories, 0);
  assert.equal(first.items.length, 25);
  assert.equal(first.pages, 2);
  assert.equal(first.offset, 0);

  const second = sectorPage(categories, 1);
  assert.equal(second.items.length, 3);
  assert.equal(second.offset, 25);

  // A stale custom id cannot point past the end.
  assert.equal(sectorPage(categories, 9).page, 1);
  assert.equal(sectorPage(categories, -3).page, 0);
});

test('the sector menu pre-selects what the member already chose', () => {
  const sessions = new OnboardingSessions();
  const session = sessions.start('900000000000000955', { sectors: ['sector_2', 'sector_27'] });

  const prompt = sectorsPrompt(session, categories);
  const menu = prompt.components[0]?.toJSON().components[0];
  assert.ok(menu && 'options' in menu);
  const defaults = menu.options.filter((option) => option.default).map((option) => option.value);
  // Only page-1 choices can be shown as default on page 1.
  assert.deepEqual(defaults, ['sector_2']);
  assert.ok(prompt.content.includes('Sector 27'));

  session.sectorPage = 1;
  const secondPage = sectorsPrompt(session, categories);
  const menuTwo = secondPage.components[0]?.toJSON().components[0];
  assert.ok(menuTwo && 'options' in menuTwo);
  assert.deepEqual(
    menuTwo.options.filter((option) => option.default).map((option) => option.value),
    ['sector_27'],
  );
});

test('the last step asks what to watch and keeps three answers', () => {
  const prompt = wantsPrompt();
  const buttons = prompt.components[0]?.toJSON().components ?? [];
  assert.deepEqual(
    buttons.map(buttonId),
    [CID.wants('jobs'), CID.wants('tenders'), CID.wants('both')],
  );
});

test('a session survives until it is cleared, and carries the previous sectors forward', () => {
  const sessions = new OnboardingSessions();
  const session = sessions.start('1', { sectors: ['health'] });
  assert.equal(sessions.get('1'), session);
  assert.deepEqual(session.sectors, ['health']);
  assert.equal(session.step, 'education');
  sessions.clear('1');
  assert.equal(sessions.get('1'), undefined);
});

test('labels render what was actually stored', () => {
  assert.equal(experienceLabel(3), '3–5 years');
  assert.equal(experienceLabel(0), 'No experience');
  assert.equal(experienceLabel(7), '7 years');
  assert.equal(experienceLabel(null), 'not set');
  assert.equal(wantsLabel('tenders'), 'tenders only');
  assert.equal(wantsLabel(null), 'jobs and tenders');
});

test('the summary states the real match count and the alert schedule', () => {
  const prompt = summaryPrompt({
    user: user(),
    matchCount: 12,
    digestTz: 'Africa/Nairobi',
    digestHour: 8,
    appBaseUrl: 'https://app.jobify.test',
  });
  assert.ok(prompt.content.includes('**12 live openings**'));
  assert.ok(prompt.content.includes('Alerts: daily at 08:00 (Africa/Nairobi)'));
  assert.ok(prompt.content.includes('Bachelor'));
  assert.ok(prompt.content.length <= 1900);
});

test('the summary says plainly when nothing matches yet', () => {
  const prompt = summaryPrompt({
    user: user({ match_profile: null }),
    matchCount: 0,
    digestTz: 'Africa/Nairobi',
    digestHour: 8,
    appBaseUrl: 'https://app.jobify.test',
  });
  assert.ok(!prompt.content.includes('**0 live'));
  assert.ok(prompt.content.includes('Nothing live matches your sectors'));
});
