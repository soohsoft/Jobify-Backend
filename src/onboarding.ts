/**
 * The onboarding interview: state, copy and component builders.
 *
 * Two rules shape this file:
 * - every answer is written to the backend as it is given, so a half-finished interview
 *   still leaves a usable profile (the matcher reads whatever exists);
 * - the interview state itself is in memory only. That is deliberate: losing a step on
 *   restart costs one re-click, and the answers are already stored server-side.
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

import type { Category, DiscordUser } from './types.js';

export const STEP_COUNT = 6;

/** Discord allows at most 25 options in one select, so the 28 sectors are paged. */
export const SECTOR_OPTIONS_PER_PAGE = 25;
export const MAX_SECTORS = 10;

export const EDUCATION_OPTIONS: Array<{ value: string; label: string }> = [
  { value: 'none', label: 'None' },
  { value: 'primary', label: 'Primary' },
  { value: 'secondary', label: 'Secondary' },
  { value: 'diploma', label: 'Diploma' },
  { value: 'bachelor', label: 'Bachelor' },
  { value: 'master', label: 'Master' },
  { value: 'phd', label: 'PhD' },
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

export type Step = 'education' | 'experience' | 'sectors' | 'location' | 'skills' | 'wants';

export interface OnboardingSession {
  step: Step;
  education?: string;
  experienceYears?: number;
  sectors: string[];
  sectorPage: number;
}

/** Custom ids in one place: a typo here would make a button silently do nothing. */
export const CID = {
  education: (value: string): string => `onb:edu:${value}`,
  experience: (value: string): string => `onb:exp:${value}`,
  sectorSelect: 'onb:sectors:select',
  sectorPage: (page: number): string => `onb:sectors:page:${page}`,
  sectorsDone: 'onb:sectors:done',
  locationOpen: 'onb:location:open',
  locationModal: 'onb:location:modal',
  skillsOpen: 'onb:skills:open',
  skillsModal: 'onb:skills:modal',
  wants: (value: string): string => `onb:want:${value}`,
  restart: 'onb:restart',
} as const;

export class OnboardingSessions {
  private readonly sessions = new Map<string, OnboardingSession>();

  start(discordId: string, existing: Partial<OnboardingSession> = {}): OnboardingSession {
    const session: OnboardingSession = {
      step: 'education',
      sectors: existing.sectors ?? [],
      sectorPage: 0,
    };
    if (existing.education !== undefined) session.education = existing.education;
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

function stepHeader(step: number, title: string, hint?: string): string {
  const lines = [`**Step ${step} of ${STEP_COUNT} — ${title}**`];
  if (hint) lines.push(hint);
  return lines.join('\n');
}

// ---------------------------------------------------------------------------
// Step 1: education
// ---------------------------------------------------------------------------

export function educationPrompt(): { content: string; components: ActionRowBuilder<ButtonBuilder>[] } {
  const buttons = EDUCATION_OPTIONS.map((option) =>
    new ButtonBuilder().setCustomId(CID.education(option.value)).setLabel(option.label).setStyle(ButtonStyle.Secondary),
  );
  // Five per row is Discord's ceiling; seven options therefore span two rows.
  const rows = [
    new ActionRowBuilder<ButtonBuilder>().addComponents(...buttons.slice(0, 5)),
    new ActionRowBuilder<ButtonBuilder>().addComponents(...buttons.slice(5)),
  ];
  return {
    content: stepHeader(
      1,
      'What is your highest level of education?',
      'This shapes which openings I filter for — you can change it later by running `/start` again.',
    ),
    components: rows,
  };
}

// ---------------------------------------------------------------------------
// Step 2: experience
// ---------------------------------------------------------------------------

export function experiencePrompt(): { content: string; components: ActionRowBuilder<ButtonBuilder>[] } {
  const buttons = EXPERIENCE_OPTIONS.map((option) =>
    new ButtonBuilder().setCustomId(CID.experience(option.value)).setLabel(option.label).setStyle(ButtonStyle.Secondary),
  );
  return {
    content: stepHeader(2, 'How much relevant work experience do you have?'),
    components: [new ActionRowBuilder<ButtonBuilder>().addComponents(...buttons)],
  };
}

// ---------------------------------------------------------------------------
// Step 3: sectors (paged multi-select)
// ---------------------------------------------------------------------------

export function sectorPage(categories: Category[], page: number): { page: number; pages: number; items: Category[]; offset: number } {
  const pages = Math.max(1, Math.ceil(categories.length / SECTOR_OPTIONS_PER_PAGE));
  const clamped = Math.min(Math.max(0, page), pages - 1);
  const offset = clamped * SECTOR_OPTIONS_PER_PAGE;
  return { page: clamped, pages, items: categories.slice(offset, offset + SECTOR_OPTIONS_PER_PAGE), offset };
}

export function sectorsPrompt(
  session: OnboardingSession,
  categories: Category[],
): { content: string; components: Array<ActionRowBuilder<StringSelectMenuBuilder> | ActionRowBuilder<ButtonBuilder>> } {
  const view = sectorPage(categories, session.sectorPage);
  session.sectorPage = view.page;

  const menu = new StringSelectMenuBuilder()
    .setCustomId(CID.sectorSelect)
    .setPlaceholder(`Sectors — page ${view.page + 1} of ${view.pages}`)
    .setMinValues(0)
    .setMaxValues(MAX_SECTORS);

  for (const category of view.items) {
    const option = new StringSelectMenuOptionBuilder()
      .setLabel(category.label.slice(0, 100))
      .setValue(category.slug);
    if (session.sectors.includes(category.slug)) option.setDefault(true);
    menu.addOptions(option);
  }

  const navRow = new ActionRowBuilder<ButtonBuilder>();
  if (view.page > 0) {
    navRow.addComponents(
      new ButtonBuilder().setCustomId(CID.sectorPage(view.page - 1)).setLabel('◂ Previous sectors').setStyle(ButtonStyle.Secondary),
    );
  }
  if (view.page < view.pages - 1) {
    navRow.addComponents(
      new ButtonBuilder().setCustomId(CID.sectorPage(view.page + 1)).setLabel('More sectors ▸').setStyle(ButtonStyle.Secondary),
    );
  }
  navRow.addComponents(
    new ButtonBuilder().setCustomId(CID.sectorsDone).setLabel('Done').setStyle(ButtonStyle.Primary),
  );

  const chosen = session.sectors.length
    ? session.sectors
        .map((slug) => categories.find((category) => category.slug === slug)?.label ?? slug)
        .join(', ')
    : 'nothing yet';

  return {
    content: stepHeader(
      3,
      'Which sectors do you want to work in?',
      `Pick up to ${MAX_SECTORS}. Selected (${session.sectors.length}): ${chosen}`,
    ),
    components: [new ActionRowBuilder<StringSelectMenuBuilder>().addComponents(menu), navRow],
  };
}

// ---------------------------------------------------------------------------
// Step 4: location
// ---------------------------------------------------------------------------

export function locationPrompt(): { content: string; components: ActionRowBuilder<ButtonBuilder>[] } {
  return {
    content: stepHeader(
      4,
      'Where are you based — and where would you work?',
      'A city or region is enough (for example `Mogadishu` or `Hargeisa`). It is used to rank nearby openings.',
    ),
    components: [
      new ActionRowBuilder<ButtonBuilder>().addComponents(
        new ButtonBuilder().setCustomId(CID.locationOpen).setLabel('Enter my location').setStyle(ButtonStyle.Primary),
      ),
    ],
  };
}

export function locationModal(): ModalBuilder {
  return new ModalBuilder()
    .setCustomId(CID.locationModal)
    .setTitle('Your location')
    .addComponents(
      new ActionRowBuilder<TextInputBuilder>().addComponents(
        new TextInputBuilder()
          .setCustomId('location')
          .setLabel('City or region')
          .setStyle(TextInputStyle.Short)
          .setMaxLength(100)
          .setRequired(true),
      ),
    );
}

// ---------------------------------------------------------------------------
// Step 5: skills
// ---------------------------------------------------------------------------

export function skillsPrompt(): { content: string; components: ActionRowBuilder<ButtonBuilder>[] } {
  return {
    content: stepHeader(
      5,
      'Which skills should I look for?',
      'Comma separated, up to 20 — for example `python, monitoring and evaluation, proposal writing`. They become the keywords I rank results by.',
    ),
    components: [
      new ActionRowBuilder<ButtonBuilder>().addComponents(
        new ButtonBuilder().setCustomId(CID.skillsOpen).setLabel('Enter my skills').setStyle(ButtonStyle.Primary),
      ),
    ],
  };
}

export function skillsModal(): ModalBuilder {
  return new ModalBuilder()
    .setCustomId(CID.skillsModal)
    .setTitle('Your skills')
    .addComponents(
      new ActionRowBuilder<TextInputBuilder>().addComponents(
        new TextInputBuilder()
          .setCustomId('skills')
          .setLabel('Skills, comma separated')
          .setStyle(TextInputStyle.Short)
          .setMaxLength(400)
          .setRequired(false),
      ),
    );
}

// ---------------------------------------------------------------------------
// Step 6: what to watch
// ---------------------------------------------------------------------------

export function wantsPrompt(): { content: string; components: ActionRowBuilder<ButtonBuilder>[] } {
  return {
    content: stepHeader(
      6,
      'What should I watch for you?',
      'This decides what the daily digest and your alerts include.',
    ),
    components: [
      new ActionRowBuilder<ButtonBuilder>().addComponents(
        new ButtonBuilder().setCustomId(CID.wants('jobs')).setLabel('Jobs').setStyle(ButtonStyle.Secondary),
        new ButtonBuilder().setCustomId(CID.wants('tenders')).setLabel('Tenders').setStyle(ButtonStyle.Secondary),
        new ButtonBuilder().setCustomId(CID.wants('both')).setLabel('Both').setStyle(ButtonStyle.Primary),
      ),
    ],
  };
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

export interface SummaryInput {
  user: DiscordUser;
  matchCount: number;
  digestTz: string;
  digestHour: number;
  appBaseUrl: string;
}

export function summaryPrompt(input: SummaryInput): {
  content: string;
  components: ActionRowBuilder<ButtonBuilder>[];
} {
  const { user } = input;
  const categories = user.match_profile?.categories ?? [];
  const sectorLabels = categories.length ? categories.join(', ') : 'none';
  const skills = user.skills.length ? user.skills.join(', ') : 'none';
  const hours = `${String(input.digestHour).padStart(2, '0')}:00`;
  const lines = [
    `**You're set up${user.name ? `, ${user.name}` : ''}.**`,
    `Education: ${educationLabel(user.education_level)} · Experience: ${experienceLabel(user.experience_years)}`,
    `Sectors: ${sectorLabels}`,
    `Location: ${user.location ?? 'not set'} · Keywords: ${skills}`,
    `Watching for: ${wantsLabel(user.wants)}`,
    `Alerts: daily at ${hours} (${input.digestTz})`,
    '',
    input.matchCount > 0
      ? `**${input.matchCount} live opening${input.matchCount === 1 ? '' : 's'}** match your sectors right now.`
      : 'Nothing live matches your sectors this minute — the digest will tell you the moment something does.',
    'Try `/jobs` to browse, or just tell me what you are looking for.',
    `<${input.appBaseUrl}/settings|Open the portal> to manage payment and your company profile.`,
  ];
  return {
    content: lines.join('\n').slice(0, 1900),
    components: [
      new ActionRowBuilder<ButtonBuilder>().addComponents(
        new ButtonBuilder().setCustomId(CID.restart).setLabel('Change my answers').setStyle(ButtonStyle.Secondary),
      ),
    ],
  };
}
