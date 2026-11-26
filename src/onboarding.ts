/**
 * The onboarding interview: state, copy and component builders.
 *
 * Four steps, in the owner's order: name → education (level + field, with an
 * "another degree?" loop) → job history (last position + years) → done. There is no
 * location step and no skills step: Jobify covers all of Somalia, and the matching
 * inputs are derived by the backend from what these answers say.
 *
 * Two rules shape this file:
 * - every answer is written to the backend as it is given, so a half-finished interview
 *   still leaves a usable profile (the matcher reads whatever exists);
 * - the education list is sent WHOLE on every save, so a re-run replaces the stored
 *   degrees instead of appending a second copy of them. The in-memory session therefore
 *   starts with an empty list and remembers what is on file only to display it.
 *
 * The session itself is in memory only. That is deliberate: losing a step on restart costs
 * one re-click, and the answers are already stored server-side.
 */
import {
  ActionRowBuilder,
  ButtonBuilder,
  ButtonStyle,
  ModalBuilder,
  StringSelectMenuBuilder,
  StringSelectMenuOptionBuilder,
  TextInputBuilder,
  TextInputStyle,
} from 'discord.js';

import type { AnyActionRow, MessagePayload } from './context.js';
import type { DiscordUser, EducationEntry, MatchRow } from './types.js';
import { formatRows, MAX_DIGEST_ROWS, noMatchesCopy } from './format.js';

/** Four steps: name, education, job history, done. */
export const STEP_COUNT = 4;

/** The backend accepts at most four degrees; the interview stops offering a fifth. */
export const MAX_EDUCATION = 4;

export const EDUCATION_OPTIONS: Array<{ value: string; label: string }> = [
  { value: 'none', label: 'None' },
  { value: 'primary', label: 'Primary' },
  { value: 'secondary', label: 'Secondary' },
  { value: 'diploma', label: 'Diploma' },
  { value: 'bachelor', label: 'Bachelor' },
  { value: 'master', label: 'Master' },
  { value: 'phd', label: 'PhD' },
];

/** Only these ask the follow-up "which field?" question. Mirrors the backend's list. */
export const DEGREE_LEVELS = ['bachelor', 'master', 'phd'];

/**
 * The field buttons, and the exact label the backend stores and maps to a job category.
 * `key` is only the custom-id fragment, so a label with a space or an ampersand never
 * becomes part of a custom id.
 */
export const FIELD_OPTIONS: Array<{ key: string; label: string }> = [
  { key: 'management', label: 'Management' },
  { key: 'computing', label: 'Computing' },
  { key: 'social_science', label: 'Social Science' },
  { key: 'economics', label: 'Economics' },
  { key: 'law', label: 'Law' },
  { key: 'education', label: 'Education' },
  { key: 'engineering', label: 'Engineering' },
  { key: 'accounting', label: 'Accounting' },
  { key: 'health_medicine', label: 'Health and Medicine' },
  { key: 'other', label: 'Other' },
];

/**
 * Experience is stored as a number of years, so each band records its LOWER bound: "3–5"
 * becomes 3. Storing the upper bound would claim more experience than the member has.
 */
export const EXPERIENCE_OPTIONS: Array<{ value: string; label: string; years: number }> = [
  { value: '0', label: 'No experience', years: 0 },
  { value: '1-2', label: '1–2 years', years: 1 },
  { value: '3-5', label: '3–5 years', years: 3 },
  { value: '6-10', label: '6–10 years', years: 6 },
  { value: '10+', label: '10+ years', years: 11 },
];

export type Step =
  | 'name'
  | 'education_level'
  | 'education_field'
  | 'another_degree'
  | 'position'
  | 'years'
  | 'employer_types'
  | 'positions';

export interface OnboardingSession {
  step: Step;
  /** What THIS run has collected. Sent whole, so it replaces the stored list. */
  education: EducationEntry[];
  /** What is already on file, shown as "answering this replaces them". */
  previousEducation: EducationEntry[];
  /** The level waiting for its field, between the two education prompts. */
  pendingLevel?: string;
  name?: string;
  lastPosition?: string;
  experienceYears?: number;
  /** Where the member wants to work: "ngo", "government", "private". */
  employerTypes: string[];
  /** Category slugs the member picked to be notified about; empty = all of their fields. */
  positions: string[];
}

/** Custom ids in one place: a typo here would make a button silently do nothing. */
export const CID = {
  nameOpen: 'onb:name',
  nameModal: 'onb:name:modal',
  education: (level: string): string => `onb:edu:${level}`,
  field: (key: string): string => `onb:field:${key}`,
  fieldModal: 'onb:field:modal',
  another: (value: 'yes' | 'no'): string => `onb:another:${value}`,
  master: (value: 'yes' | 'no'): string => `onb:master:${value}`,
  positionOpen: 'onb:position',
  positionModal: 'onb:position:modal',
  experience: (value: string): string => `onb:exp:${value}`,
  employer: 'onb:employer',
  positions: (page: number): string => `onb:positions:${page}`,
  positionsAll: 'onb:positions:all',
  positionsDone: 'onb:positions:done',
  restart: 'onb:restart',
  // The two tabs the completion message offers. They stay outside the interview: pressing one
  // never touches a session, so an old setup message still works weeks later.
  browseJobs: 'onb:browse:job',
  browseTenders: 'onb:browse:tender',
} as const;

export class OnboardingSessions {
  private readonly sessions = new Map<string, OnboardingSession>();

  start(discordId: string, existing: Partial<OnboardingSession> = {}): OnboardingSession {
    const session: OnboardingSession = {
      step: 'name',
      education: [],
      previousEducation: existing.previousEducation ?? [],
      employerTypes: existing.employerTypes ?? [],
      positions: existing.positions ?? [],
    };
    if (existing.name !== undefined) session.name = existing.name;
    if (existing.lastPosition !== undefined) session.lastPosition = existing.lastPosition;
    if (existing.experienceYears !== undefined) session.experienceYears = existing.experienceYears;
    this.sessions.set(discordId, session);
    return session;
  }

  get(discordId: string): OnboardingSession | undefined {
    return this.sessions.get(discordId);
  }

  clear(discordId: string): void {
    this.sessions.delete(discordId);
  }

  get size(): number {
    return this.sessions.size;
  }
}

export function educationLabel(value: string | null | undefined): string {
  if (!value) return 'not set';
  if (value === 'none') return 'None';
  return EDUCATION_OPTIONS.find((option) => option.value === value)?.label ?? value;
}

export function experienceLabel(years: number | null | undefined): string {
  if (years === null || years === undefined) return 'not set';
  const option = EXPERIENCE_OPTIONS.find((candidate) => candidate.years === years);
  if (option) return option.label;
  // A value set on the portal rather than through a band still renders honestly.
  return `${years} year${years === 1 ? '' : 's'}`;
}

export function wantsLabel(wants: string | null | undefined): string {
  if (wants === 'jobs') return 'jobs only';
  if (wants === 'tenders') return 'tenders only';
  return 'jobs and tenders';
}

/** "bachelor — Health", or the level alone when it was not asked for a field. */
export function educationEntryLabel(entry: EducationEntry): string {
  const level = educationLabel(entry.level);
  return entry.field ? `${level} — ${entry.field}` : level;
}

export function educationSummary(education: EducationEntry[] | null | undefined): string {
  if (!education || education.length === 0) return 'not set';
  return education.map(educationEntryLabel).join('; ');
}

function stepHeader(step: number, title: string, hint?: string): string {
  const lines = [`**Step ${step} of ${STEP_COUNT} — ${title}**`];
  if (hint) lines.push(hint);
  return lines.join('\n');
}

// ---------------------------------------------------------------------------
// Step 1: name
// ---------------------------------------------------------------------------

export function namePrompt(): { content: string; components: ActionRowBuilder<ButtonBuilder>[] } {
  return {
    content: stepHeader(
      1,
      'What is your name?',
      'I use it in your digests. Jobify covers all of Somalia, so there is no city to pick.',
    ),
    components: [
      new ActionRowBuilder<ButtonBuilder>().addComponents(
        new ButtonBuilder().setCustomId(CID.nameOpen).setLabel('Enter my name').setStyle(ButtonStyle.Primary),
      ),
    ],
  };
}

export function nameModal(current?: string | null): ModalBuilder {
  const input = new TextInputBuilder()
    .setCustomId('name')
    .setLabel('Your name')
    .setStyle(TextInputStyle.Short)
    .setMaxLength(80)
    .setRequired(true);
  if (current && current.trim().length > 0) input.setValue(current.trim().slice(0, 80));
  return new ModalBuilder()
    .setCustomId(CID.nameModal)
    .setTitle('Your name')
    .addComponents(new ActionRowBuilder<TextInputBuilder>().addComponents(input));
}

// ---------------------------------------------------------------------------
// Step 2: education — level, then field, then "another degree?"
// ---------------------------------------------------------------------------

export function educationPrompt(session: OnboardingSession): {
  content: string;
  components: ActionRowBuilder<ButtonBuilder>[];
} {
  const buttons = EDUCATION_OPTIONS.map((option) =>
    new ButtonBuilder().setCustomId(CID.education(option.value)).setLabel(option.label).setStyle(ButtonStyle.Secondary),
  );
  // Five per row is Discord's ceiling; seven options therefore span two rows.
  const rows = [
    new ActionRowBuilder<ButtonBuilder>().addComponents(...buttons.slice(0, 5)),
    new ActionRowBuilder<ButtonBuilder>().addComponents(...buttons.slice(5)),
  ];

  const ordering = session.education.length === 0 ? 'highest level' : 'level';
  const parts = [
    stepHeader(2, `What is your ${ordering} of education?`, 'Only bachelor, master and PhD ask for a field.'),
  ];
  if (session.education.length > 0) {
    parts.push(`Already added: ${educationSummary(session.education)}`);
  } else if (session.previousEducation.length > 0) {
    parts.push(
      `On file: ${educationSummary(session.previousEducation)}. Answering this replaces that list — you can add every degree back in the next questions.`,
    );
  }
  return { content: parts.join('\n'), components: rows };
}


// ---------------------------------------------------------------------------
// After the last job: where to work, and which positions to be notified about
// ---------------------------------------------------------------------------

/** The three employer types, in the owner's order. Slugs match the backend's `EMPLOYER_TYPES`. */
export const EMPLOYER_CHOICES: ReadonlyArray<{ slug: string; label: string; description: string }> = [
  { slug: 'ngo', label: 'Non-Profit Organisations', description: 'UN agencies, international and local NGOs' },
  { slug: 'government', label: 'Government', description: 'Ministries, agencies, federal and local authorities' },
  { slug: 'private', label: 'Private Companies', description: 'Businesses, banks, telecoms, consultancies' },
];

/**
 * The 28 job categories, in the backend's display order.
 *
 * Duplicated here on purpose: the bot keeps no backend taxonomy at boot (it starts before the
 * backend may be up), and a select menu needs labels and slugs in the same payload. The backend
 * re-validates every slug, so a drift shows up as a refusal, never as a silently unmatched
 * profile.
 */
export const POSITION_CHOICES: ReadonlyArray<{ slug: string; label: string }> = [
  { slug: 'health', label: 'Health' },
  { slug: 'education', label: 'Education' },
  { slug: 'business_and_finance', label: 'Business & Finance' },
  { slug: 'ict', label: 'ICT' },
  { slug: 'energy', label: 'Energy (Electrical & Solar)' },
  { slug: 'agriculture', label: 'Agriculture' },
  { slug: 'fishery', label: 'Fishery' },
  { slug: 'livestock', label: 'Livestock' },
  { slug: 'engineering_and_construction', label: 'Engineering & Construction' },
  { slug: 'logistics_and_supply_chain', label: 'Logistics & Supply Chain' },
  { slug: 'telecommunications_and_media', label: 'Telecommunications & Media' },
  { slug: 'legal_and_regulatory_services', label: 'Legal & Regulatory Services' },
  { slug: 'environment_and_climate', label: 'Environment & Climate' },
  { slug: 'manufacturing_and_production', label: 'Manufacturing & Production' },
  { slug: 'hospitality_and_tourism', label: 'Hospitality & Tourism' },
  { slug: 'human_rights_and_advocacy', label: 'Human Rights, Civil Rights & Advocacy' },
  { slug: 'humanitarian_aid_and_emergency_relief', label: 'Humanitarian Aid & Emergency Relief' },
  { slug: 'peacebuilding_and_security', label: 'Peacebuilding, Conflict Resolution & Security' },
  { slug: 'gender_equality_and_social_inclusion', label: 'Gender Equality & Social Inclusion' },
  { slug: 'youth_and_child_development', label: 'Youth & Child Development' },
  { slug: 'migration_refugees_and_displaced', label: 'Migration, Refugees & Displaced Persons' },
  { slug: 'arts_culture_and_heritage', label: 'Arts, Culture & Heritage Preservation' },
  { slug: 'governance_democracy_and_civic_participation', label: 'Governance, Democracy & Civic Participation' },
  { slug: 'protection', label: 'Protection' },
  { slug: 'livelihoods_and_resilience', label: 'Livelihoods & Resilience' },
  { slug: 'food_security', label: 'Food Security' },
  { slug: 'shelter_and_non_food_items', label: 'Shelter & Non-Food Items (NFIs)' },
  { slug: 'wash', label: 'WASH (Water, Sanitation & Hygiene)' },
];

/** Discord caps a select at 25 options, so 28 categories need two menus of 14. */
const POSITIONS_PER_PAGE = 14;

/** The employer types, as a multi-select. */
export function employerPrompt(): StepPrompt {
  const menu = new StringSelectMenuBuilder()
    .setCustomId(CID.employer)
    .setPlaceholder('Where would you like to work?')
    .setMinValues(1)
    .setMaxValues(EMPLOYER_CHOICES.length)
    .addOptions(
      EMPLOYER_CHOICES.map((choice) =>
        new StringSelectMenuOptionBuilder()
          .setLabel(choice.label)
          .setValue(choice.slug)
          .setDescription(choice.description),
      ),
    );

  return {
    content: [
      '**Step 3 of 5 — Where would you like to work?**',
      '',
      'Pick one or more. This nudges jobs with that kind of employer up your alerts — it never hides the rest.',
    ].join('\n'),
    components: [new ActionRowBuilder<StringSelectMenuBuilder>().addComponents(menu)],
    hideTabs: true,
  };
}

/**
 * The position picker: two 14-option menus (Discord's cap is 25 per select), with the whole
 * list and a Done button underneath. Selections accumulate in the session; Done writes them.
 */
export function positionsPrompt(session: OnboardingSession, page = 0): StepPrompt {
  const start = page * POSITIONS_PER_PAGE;
  const slice = POSITION_CHOICES.slice(start, start + POSITIONS_PER_PAGE);
  const menu = new StringSelectMenuBuilder()
    .setCustomId(CID.positions(page))
    .setPlaceholder(`Positions ${start + 1}–${start + slice.length} of ${POSITION_CHOICES.length}`)
    .setMinValues(1)
    .setMaxValues(slice.length)
    .addOptions(
      slice.map((choice) => {
        const picked = session.positions.includes(choice.slug);
        const option = new StringSelectMenuOptionBuilder()
          .setLabel(choice.label.slice(0, 100))
          .setValue(choice.slug)
          .setDefault(picked);
        return option;
      }),
    );

  const shown = session.positions.length === 0
    ? 'Nothing picked yet — press **All categories** to take everything.'
    : `Picked: ${session.positions
        .map((slug) => POSITION_CHOICES.find((choice) => choice.slug === slug)?.label ?? slug)
        .join(', ')}`;

  const pages = Math.ceil(POSITION_CHOICES.length / POSITIONS_PER_PAGE);
  const buttons = new ActionRowBuilder<ButtonBuilder>();
  if (page > 0) {
    buttons.addComponents(
      new ButtonBuilder().setCustomId(`onb:positions:page:${page - 1}`).setLabel('‹ Previous').setStyle(ButtonStyle.Secondary),
    );
  }
  if (page < pages - 1) {
    buttons.addComponents(
      new ButtonBuilder().setCustomId(`onb:positions:page:${page + 1}`).setLabel('More categories ›').setStyle(ButtonStyle.Secondary),
    );
  }
  buttons.addComponents(
    new ButtonBuilder().setCustomId(CID.positionsAll).setLabel('All categories').setStyle(ButtonStyle.Secondary),
    new ButtonBuilder().setCustomId(CID.positionsDone).setLabel('Done ✓').setStyle(ButtonStyle.Success),
  );

  return {
    content: [
      '**Step 4 of 5 — Which sectors are you interested to get notified about?**',
      '',
      'Pick as many as you like, then press **Done**. Or take **All categories**.',
      '',
      shown,
    ].join('\n'),
    components: [new ActionRowBuilder<StringSelectMenuBuilder>().addComponents(menu), buttons],
    hideTabs: true,
  };
}

export function employerLabels(slugs: string[]): string {
  return slugs
    .map((slug) => EMPLOYER_CHOICES.find((choice) => choice.slug === slug)?.label ?? slug)
    .join(', ');
}

export function fieldPrompt(level: string): {
  content: string;
  components: ActionRowBuilder<ButtonBuilder>[];
} {
  const buttons = FIELD_OPTIONS.map((option) =>
    new ButtonBuilder().setCustomId(CID.field(option.key)).setLabel(option.label).setStyle(ButtonStyle.Secondary),
  );
  // Five per row is Discord's ceiling, and an empty row is a payload error — a 10-option list
  // must produce exactly two rows, not three with a blank one.
  const rows: ActionRowBuilder<ButtonBuilder>[] = [];
  for (let index = 0; index < buttons.length; index += 5) {
    rows.push(new ActionRowBuilder<ButtonBuilder>().addComponents(...buttons.slice(index, index + 5)));
  }
  // A master's is asked as a programme, not as a field, because that is how the degree reads.
  const question = level === 'master' ? 'Which Master\u2019s programme did you take?' : `Which field is your ${educationLabel(level)} in?`;
  return {
    content: stepHeader(
      2,
      question,
      'This is what your matches are filtered by. "Other" asks you to type it, and becomes keywords instead.',
    ),
    components: rows,
  };
}

export function fieldModal(): ModalBuilder {
  return new ModalBuilder()
    .setCustomId(CID.fieldModal)
    .setTitle('Your field of study')
    .addComponents(
      new ActionRowBuilder<TextInputBuilder>().addComponents(
        new TextInputBuilder()
          .setCustomId('field')
          .setLabel('Field of study')
          .setStyle(TextInputStyle.Short)
          .setMaxLength(60)
          .setRequired(true),
      ),
    );
}

/**
 * The question after a degree is added.
 *
 * After a BACHELOR the owner wants the specific question — "do you have a Master's?" — because
 * that is the only degree a bachelor holder can add next; "do you have another degree?" made them
 * answer a level question they had already answered implicitly. Any other level keeps the general
 * prompt, which can still add a PhD.
 */
export function anotherPrompt(session: OnboardingSession): {
  content: string;
  components: ActionRowBuilder<ButtonBuilder>[];
} {
  if (asksForMasters(session)) return mastersPrompt(session);

  const atLimit = session.education.length >= MAX_EDUCATION;
  const hint = atLimit
    ? `That is ${MAX_EDUCATION} degrees, which is the maximum. Next: your last job.`
    : `Added: ${educationSummary(session.education)}`;
  const row = new ActionRowBuilder<ButtonBuilder>();
  if (!atLimit) {
    row.addComponents(
      new ButtonBuilder().setCustomId(CID.another('yes')).setLabel('Yes, add another').setStyle(ButtonStyle.Secondary),
    );
  }
  row.addComponents(new ButtonBuilder().setCustomId(CID.another('no')).setLabel('No, that is all').setStyle(ButtonStyle.Primary));
  return {
    content: stepHeader(2, 'Do you have another degree?', hint),
    components: [row],
  };
}

/** True when the last degree added was a bachelor and no master's is on file yet. */
export function asksForMasters(session: OnboardingSession): boolean {
  const last = session.education[session.education.length - 1];
  if (!last || last.level !== 'bachelor') return false;
  if (session.education.length >= MAX_EDUCATION) return false;
  return !session.education.some((entry) => entry.level === 'master');
}

/** "Do you have a Master's?" — the one question that follows a bachelor. */
export function mastersPrompt(session: OnboardingSession): {
  content: string;
  components: ActionRowBuilder<ButtonBuilder>[];
} {
  const row = new ActionRowBuilder<ButtonBuilder>().addComponents(
    new ButtonBuilder().setCustomId(CID.master('yes')).setLabel('Yes, a Master\'s').setStyle(ButtonStyle.Secondary),
    new ButtonBuilder().setCustomId(CID.master('no')).setLabel('No').setStyle(ButtonStyle.Primary),
  );
  return {
    content: stepHeader(2, 'Do you have a Master\’s degree?', `Added: ${educationSummary(session.education)}`),
    components: [row],
  };
}

// ---------------------------------------------------------------------------
// Step 3: job history — last position, then years
// ---------------------------------------------------------------------------

export function positionPrompt(): { content: string; components: ActionRowBuilder<ButtonBuilder>[] } {
  return {
    content: stepHeader(
      3,
      'What was your last job?',
      'The job title you last held — for example `Programme Officer`. It becomes the keywords your matches are ranked by.',
    ),
    components: [
      new ActionRowBuilder<ButtonBuilder>().addComponents(
        new ButtonBuilder().setCustomId(CID.positionOpen).setLabel('Enter my last job title').setStyle(ButtonStyle.Primary),
      ),
    ],
  };
}

export function positionModal(current?: string | null): ModalBuilder {
  const input = new TextInputBuilder()
    .setCustomId('position')
    .setLabel('Last job title')
    .setStyle(TextInputStyle.Short)
    .setMaxLength(120)
    .setRequired(true);
  if (current && current.trim().length > 0) input.setValue(current.trim().slice(0, 120));
  return new ModalBuilder()
    .setCustomId(CID.positionModal)
    .setTitle('Your last job')
    .addComponents(new ActionRowBuilder<TextInputBuilder>().addComponents(input));
}

export function experiencePrompt(): { content: string; components: ActionRowBuilder<ButtonBuilder>[] } {
  const buttons = EXPERIENCE_OPTIONS.map((option) =>
    new ButtonBuilder().setCustomId(CID.experience(option.value)).setLabel(option.label).setStyle(ButtonStyle.Secondary),
  );
  return {
    content: stepHeader(3, 'How much relevant work experience do you have?'),
    components: [new ActionRowBuilder<ButtonBuilder>().addComponents(...buttons)],
  };
}

// ---------------------------------------------------------------------------
// The prompt for wherever a session currently is
// ---------------------------------------------------------------------------

export interface StepPrompt {
  content: string;
  components: AnyActionRow[];
  /** Interview steps never carry the Jobs/Tenders tabs. */
  hideTabs?: boolean;
}

/**
 * Shared by the free-text path when a member writes again mid-interview: re-sending the step
 * they were on is what keeps their place without starting a second session (and without
 * losing answers already written server-side).
 */
export function promptForStep(step: Step, session: OnboardingSession): StepPrompt {
  switch (step) {
    case 'name':
      return namePrompt();
    case 'education_level':
      return educationPrompt(session);
    case 'education_field':
      return fieldPrompt(session.pendingLevel ?? 'bachelor');
    case 'another_degree':
      return anotherPrompt(session);
    case 'position':
      return positionPrompt();
    case 'years':
      return experiencePrompt();
    case 'employer_types':
      return employerPrompt();
    case 'positions':
      return positionsPrompt(session);
  }
}

// ---------------------------------------------------------------------------
// Step 4: completion — "Setup complete", the current matches, and /jobs
// ---------------------------------------------------------------------------

export interface CompletionInput {
  user: DiscordUser;
  /** Rows from `GET /internal/jobs/for-user/{discord_id}` — what the backend decided. */
  matches: MatchRow[];
  /** `meta.reason` of the same read: `below_threshold` means the list is empty because
   *  nothing matched closely enough, which is a different sentence from "nothing live". */
  matchReason?: string | null;
  digestTz: string;
  digestHour: number;
  appBaseUrl: string;
}

/** "a, b and c" — a Discord sentence, not a comma list with a trailing separator. */
export function plainList(items: string[]): string {
  const clean = items.map((item) => item.trim()).filter((item) => item.length > 0);
  if (clean.length <= 1) return clean[0] ?? '';
  return `${clean.slice(0, -1).join(', ')} and ${clean[clean.length - 1]}`;
}

/**
 * The one sentence that shows the member what the backend decided for them.
 *
 * The interview never asks them to pick a sector: the backend derives the categories from
 * their education at the end, and this is where that decision is said out loud — "I will
 * watch for roles in health" — so it is visible rather than implied.
 *
 * With no categories decided (a degree the map does not know) it returns `null` and the
 * caller omits the line rather than printing an empty list.
 */
export function watchingLine(profile: DiscordUser['match_profile']): string | null {
  const categories = profile?.categories ?? [];
  if (categories.length > 0) return `I will watch for roles in ${plainList(categories)}.`;
  return null;
}

/**
 * The end of the interview. It repeats what was recorded (so the member can see the answers
 * landed), then shows up to five CURRENT matches — the first digest — and the two things they
 * can do next. Rows come from the backend's own matcher through `formatRows`, so this is the
 * same renderer the daily digest uses.
 */
/**
 * The Jobs / Tenders pair, as a row that can stand on its own.
 *
 * It is reused wherever the member needs a way in without typing a command: the setup
 * message, the welcome of a member who is already set up, and any greeting in the DM. One
 * builder means the three surfaces cannot drift apart.
 */
export function browseTabsRow(): ActionRowBuilder<ButtonBuilder> {
  return new ActionRowBuilder<ButtonBuilder>().addComponents(
    new ButtonBuilder().setCustomId(CID.browseJobs).setLabel('Jobs').setStyle(ButtonStyle.Primary),
    new ButtonBuilder().setCustomId(CID.browseTenders).setLabel('Tenders').setStyle(ButtonStyle.Primary),
  );
}

export function completionPrompt(input: CompletionInput): {
  content: string;
  components: ActionRowBuilder<ButtonBuilder>[];
} {
  const { user } = input;
  const hours = `${String(input.digestHour).padStart(2, '0')}:00`;
  const lines = [
    '**Setup complete**',
    `Name: ${user.name || 'not set'}`,
    `Education: ${educationSummary(user.education)}`,
    `Last job: ${user.last_position || 'not set'} · Experience: ${experienceLabel(user.experience_years)}`,
  ];

  // What the backend decided to watch for, in the member's own language. This is the only
  // place the decision is visible: they were never asked to choose a sector.
  const watching = watchingLine(user.match_profile);
  if (watching) lines.push(watching);

  const alerts = user.alerts;
  lines.push(
    alerts?.enabled
      ? `Alerts: on, ${alerts.mode === 'instant' ? 'instant' : `daily at ${hours} (${input.digestTz})`}`
      : 'Alerts: off — your account page (`/account`) turns them back on whenever you want the digest.',
  );

  lines.push('');
  if (input.matches.length > 0) {
    lines.push(`**${input.matches.length === 1 ? '1 opening matches' : `${input.matches.length} openings match`} you right now:**`, '');
    lines.push(
      formatRows(
        input.matches.map((row) => ({
          title: row.job.title,
          organization: row.job.organization,
          location: row.job.location ?? null,
          deadline: row.job.deadline ?? null,
          kind: row.job.kind ?? null,
          jobId: row.job._id,
          url: row.job.url ?? null,
          matchScore: row.matchScore,
        })),
        { appBaseUrl: input.appBaseUrl, max: MAX_DIGEST_ROWS, title: 'MATCHES FOR YOU' },
      ),
    );
  } else if (input.matchReason === 'below_threshold') {
    // A profile exists and the matcher scored rows; none matched closely enough. The member is
    // told the plain outcome — never the internal score or its cut-off — and shown no weak rows.
    lines.push(noMatchesCopy());
  } else {
    lines.push(
      'Nothing live matches your answers this minute — I check again every day and will DM you the moment something does.',
    );
  }
  // The tabs are the real invitation; "these five" only makes sense when there are five.
  const tabsLine =
    input.matches.length > 0
      ? '**Jobs** and **Tenders** below open the full list — not just these five.'
      : '**Jobs** and **Tenders** below open the full list at any time.';
  lines.push('', tabsLine);

  return {
    content: lines.join('\n').slice(0, 1900),
    components: [
      // Two tabs on their own row — the same row every other surface uses — with the re-run
      // beneath them so it is not mistaken for a third list.
      browseTabsRow(),
      new ActionRowBuilder<ButtonBuilder>().addComponents(
        new ButtonBuilder().setCustomId(CID.restart).setLabel('Change my answers').setStyle(ButtonStyle.Secondary),
      ),
    ],
  };
}
