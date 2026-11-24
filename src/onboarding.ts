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
  TextInputBuilder,
  TextInputStyle,
} from 'discord.js';

import type { DiscordUser, EducationEntry, MatchRow } from './types.js';
import { belowThresholdCopy, formatRows, MAX_DIGEST_ROWS } from './format.js';

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
  { key: 'health', label: 'Health' },
  { key: 'education', label: 'Education' },
  { key: 'business', label: 'Business administration' },
  { key: 'finance', label: 'Finance & accounting' },
  { key: 'ict', label: 'ICT' },
  { key: 'engineering', label: 'Engineering' },
  { key: 'agriculture', label: 'Agriculture' },
  { key: 'law', label: 'Law' },
  { key: 'logistics', label: 'Logistics & procurement' },
  { key: 'social', label: 'Social science & development' },
  { key: 'wash', label: 'WASH' },
  { key: 'nutrition', label: 'Nutrition' },
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
  | 'years';

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
}

/** Custom ids in one place: a typo here would make a button silently do nothing. */
export const CID = {
  nameOpen: 'onb:name',
  nameModal: 'onb:name:modal',
  education: (level: string): string => `onb:edu:${level}`,
  field: (key: string): string => `onb:field:${key}`,
  fieldModal: 'onb:field:modal',
  another: (value: 'yes' | 'no'): string => `onb:another:${value}`,
  positionOpen: 'onb:position',
  positionModal: 'onb:position:modal',
  experience: (value: string): string => `onb:exp:${value}`,
  restart: 'onb:restart',
} as const;

export class OnboardingSessions {
  private readonly sessions = new Map<string, OnboardingSession>();

  start(discordId: string, existing: Partial<OnboardingSession> = {}): OnboardingSession {
    const session: OnboardingSession = {
      step: 'name',
      education: [],
      previousEducation: existing.previousEducation ?? [],
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

export function fieldPrompt(level: string): {
  content: string;
  components: ActionRowBuilder<ButtonBuilder>[];
} {
  const buttons = FIELD_OPTIONS.map((option) =>
    new ButtonBuilder().setCustomId(CID.field(option.key)).setLabel(option.label).setStyle(ButtonStyle.Secondary),
  );
  const rows = [
    new ActionRowBuilder<ButtonBuilder>().addComponents(...buttons.slice(0, 5)),
    new ActionRowBuilder<ButtonBuilder>().addComponents(...buttons.slice(5, 10)),
    new ActionRowBuilder<ButtonBuilder>().addComponents(...buttons.slice(10)),
  ];
  return {
    content: stepHeader(
      2,
      `Which field is your ${educationLabel(level)} in?`,
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

export function anotherPrompt(session: OnboardingSession): {
  content: string;
  components: ActionRowBuilder<ButtonBuilder>[];
} {
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
  components: ActionRowBuilder<ButtonBuilder>[];
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
   *  nothing reached the cut-off, which is a different sentence from "nothing live". */
  matchReason?: string | null;
  /** `meta.threshold`, so the sentence names the number the backend actually applied. */
  matchThreshold?: number | null;
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
 * The interview never asks them to pick a sector: the backend derives the categories and the
 * role titles at the end, and this is where that decision is said out loud — "I will watch
 * for Data Analyst, MIS Officer and Business Analyst" — so it is visible rather than implied.
 *
 * Written to survive a degraded derivation: with role titles it names them; with only the
 * categories (the rules fallback, i.e. the model was unavailable) it names those; with
 * neither it returns `null` and the caller omits the line rather than printing an empty list.
 */
export function watchingLine(profile: DiscordUser['match_profile']): string | null {
  const roles = profile?.subcategories ?? [];
  if (roles.length > 0) return `I will watch for ${plainList(roles)}.`;
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
      : 'Alerts: off — switch them on with `/alerts` whenever you want the digest.',
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
        { appBaseUrl: input.appBaseUrl, max: MAX_DIGEST_ROWS },
      ),
    );
  } else if (input.matchReason === 'below_threshold') {
    // A profile exists and the matcher scored rows; none reached the bar. The cut-off is a
    // promise, so the member is told the number rather than shown weak rows.
    lines.push(belowThresholdCopy(input.matchThreshold));
  } else {
    lines.push(
      'Nothing live matches your answers this minute — I check again every day and will DM you the moment something does.',
    );
  }
  lines.push('', '`/jobs` lists everything for you at any time, not just these five.');

  return {
    content: lines.join('\n').slice(0, 1900),
    components: [
      new ActionRowBuilder<ButtonBuilder>().addComponents(
        new ButtonBuilder().setCustomId(CID.restart).setLabel('Change my answers').setStyle(ButtonStyle.Secondary),
      ),
    ],
  };
}
