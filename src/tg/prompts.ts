/**
 * The Telegram interview: state, copy and keyboards.
 *
 * Same questions, same order, same wording as the Discord interview — the backend is the one
 * that decides categories, so the two fronts must not drift. What changes is the shape: a
 * Telegram inline keyboard is a grid of callback buttons, and a select menu does not exist, so
 * the multi-choice questions are toggles redrawn on each tap with a ✓ on the chosen ones.
 */
import type { InlineKeyboard } from './client.js';

export type TgStep =
  | 'name'
  | 'education_level'
  | 'education_field'
  | 'another_degree'
  | 'position'
  | 'years'
  | 'employer_types'
  | 'positions'
  | 'company_name'
  | 'company_services'
  | 'company_city'
  | 'done';

export interface TgSession {
  step: TgStep;
  name?: string;
  education: Array<{ level: string; field: string }>;
  pendingLevel?: string;
  lastPosition?: string;
  experienceYears?: number;
  employerTypes: string[];
  positions: string[];
  /** The position-picker page (0 or 1), because 28 toggles do not fit one keyboard. */
  positionsPage: number;
  /** True when finished: the completion card should offer the tabs. */
  completed?: boolean;
  companyName?: string;
  companyServices?: string;
  updatedAt: number;
}

export class TgSessions {
  private readonly sessions = new Map<number, TgSession>();
  constructor(private readonly ttlMs = 2 * 60 * 60_000) {}

  start(id: number): TgSession {
    const session: TgSession = {
      step: 'name',
      education: [],
      employerTypes: [],
      positions: [],
      positionsPage: 0,
      updatedAt: Date.now(),
    };
    this.sessions.set(id, session);
    return session;
  }

  get(id: number): TgSession | undefined {
    const session = this.sessions.get(id);
    if (!session) return undefined;
    if (Date.now() - session.updatedAt > this.ttlMs) {
      this.sessions.delete(id);
      return undefined;
    }
    return session;
  }

  clear(id: number): void {
    this.sessions.delete(id);
  }
}

export const CID = {
  name: 'onb:name',
  level: (level: string): string => `onb:lvl:${level}`,
  field: (key: string): string => `onb:fld:${key}`,
  master: (value: 'yes' | 'no'): string => `onb:master:${value}`,
  another: (value: 'yes' | 'no'): string => `onb:another:${value}`,
  years: (value: string): string => `onb:yrs:${value}`,
  employer: (slug: string): string => `onb:emp:${slug}`,
  employerDone: 'onb:emp:done',
  position: (slug: string): string => `onb:pos:${slug}`,
  positionsPage: (page: number): string => `onb:posp:${page}`,
  positionsAll: 'onb:pos:all',
  positionsDone: 'onb:pos:done',
  browseJobs: 'browse:job',
  browseTenders: 'browse:tender',
  account: 'account:link',
  companyOpen: 'company:open',
  companyCity: (city: string): string => `company:city:${city}`,
} as const;

export const LEVELS: ReadonlyArray<{ key: string; label: string }> = [
  { key: 'none', label: 'None' },
  { key: 'primary', label: 'Primary' },
  { key: 'secondary', label: 'Secondary' },
  { key: 'diploma', label: 'Diploma' },
  { key: 'bachelor', label: 'Bachelor' },
  { key: 'master', label: 'Master' },
  { key: 'phd', label: 'PhD' },
];

/** Bachelor and above ask for a field — the only levels that map to a category. */
export const FIELDS_ASKED: ReadonlyArray<string> = ['bachelor', 'master', 'phd'];

export const FIELDS: ReadonlyArray<{ key: string; label: string }> = [
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

export const EMPLOYERS: ReadonlyArray<{ slug: string; label: string }> = [
  { slug: 'ngo', label: 'Non-Profit Organisations' },
  { slug: 'government', label: 'Government' },
  { slug: 'private', label: 'Private Companies' },
];

export const POSITIONS: ReadonlyArray<{ slug: string; label: string }> = [
  { slug: 'health', label: 'Health' },
  { slug: 'education', label: 'Education' },
  { slug: 'business_and_finance', label: 'Business & Finance' },
  { slug: 'ict', label: 'ICT' },
  { slug: 'energy', label: 'Energy' },
  { slug: 'agriculture', label: 'Agriculture' },
  { slug: 'fishery', label: 'Fishery' },
  { slug: 'livestock', label: 'Livestock' },
  { slug: 'engineering_and_construction', label: 'Engineering' },
  { slug: 'logistics_and_supply_chain', label: 'Logistics' },
  { slug: 'telecommunications_and_media', label: 'Media & Telecom' },
  { slug: 'legal_and_regulatory_services', label: 'Legal' },
  { slug: 'environment_and_climate', label: 'Environment' },
  { slug: 'manufacturing_and_production', label: 'Manufacturing' },
  { slug: 'hospitality_and_tourism', label: 'Hospitality' },
  { slug: 'human_rights_and_advocacy', label: 'Human Rights' },
  { slug: 'humanitarian_aid_and_emergency_relief', label: 'Humanitarian' },
  { slug: 'peacebuilding_and_security', label: 'Peace & Security' },
  { slug: 'gender_equality_and_social_inclusion', label: 'Gender' },
  { slug: 'youth_and_child_development', label: 'Youth & Children' },
  { slug: 'migration_refugees_and_displaced', label: 'Refugees' },
  { slug: 'arts_culture_and_heritage', label: 'Arts & Culture' },
  { slug: 'governance_democracy_and_civic_participation', label: 'Governance' },
  { slug: 'protection', label: 'Protection' },
  { slug: 'livelihoods_and_resilience', label: 'Livelihoods' },
  { slug: 'food_security', label: 'Food Security' },
  { slug: 'shelter_and_non_food_items', label: 'Shelter & NFI' },
  { slug: 'wash', label: 'WASH' },
];

const POSITIONS_PER_PAGE = 14;

export const YEARS: ReadonlyArray<{ key: string; label: string }> = [
  { key: '0', label: 'Less than 1' },
  { key: '1', label: '1–2' },
  { key: '3', label: '3–5' },
  { key: '6', label: '6–10' },
  { key: '10', label: '10+' },
];

/** Three per row, so a phone shows a compact grid instead of a wall of buttons. */
function grid(items: Array<{ text: string; callback_data: string }>, perRow = 3): InlineKeyboard {
  const rows: InlineKeyboard = [];
  for (let i = 0; i < items.length; i += perRow) rows.push(items.slice(i, i + perRow));
  return rows;
}

export function namePrompt(): { text: string; keyboard: InlineKeyboard } {
  return {
    text: '👋 <b>Welcome to Jobify</b> — jobs and tenders in Somalia.\n\n<b>Step 1 of 5 — What is your name?</b>\n\nJust type it and send.',
    keyboard: [],
  };
}

export function levelPrompt(): { text: string; keyboard: InlineKeyboard } {
  return {
    text: '<b>Step 2 of 5 — What is the highest level you finished?</b>',
    keyboard: grid(LEVELS.map((level) => ({ text: level.label, callback_data: CID.level(level.key) }))),
  };
}

export function fieldPrompt(level: string): { text: string; keyboard: InlineKeyboard } {
  const label = LEVELS.find((entry) => entry.key === level)?.label ?? 'degree';
  return {
    text: `<b>Step 2 of 5 — Which field is your ${label} in?</b>`,
    keyboard: grid(FIELDS.map((field) => ({ text: field.label, callback_data: CID.field(field.key) }))),
  };
}

export function masterPrompt(): { text: string; keyboard: InlineKeyboard } {
  return {
    text: "<b>Step 2 of 5 — Do you have a Master's degree?</b>",
    keyboard: [
      [
        { text: 'Yes', callback_data: CID.master('yes') },
        { text: 'No', callback_data: CID.master('no') },
      ],
    ],
  };
}

export function anotherPrompt(education: Array<{ level: string; field: string }>): {
  text: string;
  keyboard: InlineKeyboard;
} {
  // The field is stored as a slug; the member must never see `ict` where they tapped
  // "Computing".
  const added = education
    .map((entry) => {
      const level = LEVELS.find((item) => item.key === entry.level)?.label ?? entry.level;
      const field = FIELDS.find((item) => item.key === entry.field)?.label ?? entry.field;
      return `• ${level}${field ? ` — ${field}` : ''}`;
    })
    .join('\n');
  return {
    text: `<b>Added:</b>\n${added}\n\n<b>Any other degree or certificate?</b>`,
    keyboard: [
      [
        { text: 'Yes', callback_data: CID.another('yes') },
        { text: 'No', callback_data: CID.another('no') },
      ],
    ],
  };
}

export function positionPrompt(): { text: string; keyboard: InlineKeyboard } {
  return {
    text: '<b>Step 3 of 5 — List the last three positions you worked?</b>\n\nSeparate them with commas, most recent first.\n<i>For example: WASH Officer, Field Assistant, Data Clerk</i>',
    keyboard: [],
  };
}

export function yearsPrompt(): { text: string; keyboard: InlineKeyboard } {
  return {
    text: '<b>How much relevant work experience do you have?</b>',
    keyboard: grid(YEARS.map((band) => ({ text: band.label, callback_data: CID.years(band.key) })), 3),
  };
}

export function employerPrompt(session: TgSession): { text: string; keyboard: InlineKeyboard } {
  const chosen = new Set(session.employerTypes);
  const buttons = EMPLOYERS.map((employer) => ({
    text: `${chosen.has(employer.slug) ? '✓ ' : ''}${employer.label}`,
    callback_data: CID.employer(employer.slug),
  }));
  return {
    text: '<b>Step 5 of 5 — Where would you like to work?</b>\n\nTap one or more, then Done.',
    keyboard: [...buttons.map((button) => [button]), [{ text: 'Done ✓', callback_data: CID.employerDone }]],
  };
}

export function positionsPrompt(session: TgSession): { text: string; keyboard: InlineKeyboard } {
  const start = session.positionsPage * POSITIONS_PER_PAGE;
  const slice = POSITIONS.slice(start, start + POSITIONS_PER_PAGE);
  const chosen = new Set(session.positions);
  const buttons = slice.map((position) => ({
    text: `${chosen.has(position.slug) ? '✓ ' : ''}${position.label}`,
    callback_data: CID.position(position.slug),
  }));
  const rows = grid(buttons, 2);
  const nav: InlineKeyboard[number] = [];
  if (session.positionsPage > 0) nav.push({ text: '‹', callback_data: CID.positionsPage(session.positionsPage - 1) });
  if (start + POSITIONS_PER_PAGE < POSITIONS.length) {
    nav.push({ text: 'More ›', callback_data: CID.positionsPage(session.positionsPage + 1) });
  }
  nav.push({ text: 'All', callback_data: CID.positionsAll });
  nav.push({ text: 'Done ✓', callback_data: CID.positionsDone });
  rows.push(nav);
  const picked = session.positions
    .map((slug) => POSITIONS.find((position) => position.slug === slug)?.label ?? slug)
    .join(', ');
  return {
    text: [
      '<b>Step 5 of 5 — Which other sectors are you interested to get notified about?</b>',
      '',
      'Tap as many as you like, then Done. Or All.',
      '',
      picked ? `<i>Picked: ${picked}</i>` : '<i>Nothing picked yet.</i>',
    ].join('\n'),
    keyboard: rows,
  };
}

export function companyNamePrompt(): { text: string; keyboard: InlineKeyboard } {
  return {
    text: '<b>Company information (1 of 3)</b>\n\nWhat is the company name?',
    keyboard: [],
  };
}

export function companyServicesPrompt(): { text: string; keyboard: InlineKeyboard } {
  return {
    text: '<b>Company information (2 of 3)</b>\n\nWhat services does the company offer?\n<i>For example: Construction, Logistics</i>',
    keyboard: [],
  };
}

export function companyCityPrompt(): { text: string; keyboard: InlineKeyboard } {
  const cities = ['Mogadishu', 'Hargeisa', 'Bosaso', 'Kismayo', 'Baidoa', 'Galkayo', 'Garowe', 'Beledweyne'];
  return {
    text: '<b>Company information (3 of 3)</b>\n\nWhere is the company based?',
    keyboard: grid(cities.map((city) => ({ text: city, callback_data: CID.companyCity(city) })), 2),
  };
}

/** The card shown when the interview ends. Matches the Discord completion message. */
export function completionText(input: {
  name: string;
  matches: number;
  alertsHour: number;
  timeZone: string;
}): string {
  return [
    '<b>Setup complete ✓</b>',
    '',
    `Thanks ${input.name} — your profile is saved.`,
    `Alerts: on, daily at ${input.alertsHour} (${input.timeZone}).`,
    '',
    input.matches > 0
      ? `I found <b>${input.matches}</b> matching jobs for you.`
      : 'Nothing live matches your answers this minute — I check again every day and will message you the moment something does.',
    '',
    'Never miss a job. Daily Jobs Notifications just for $1/month.\nSubscription now → /balance',
  ].join('\n');
}

export function browseKeyboard(): InlineKeyboard {
  return [
    [
      { text: 'My Jobs', callback_data: CID.browseJobs },
      { text: 'My Tenders', callback_data: CID.browseTenders },
    ],
    // No My Account here on purpose: the account link lives behind /balance only.
  ];
}
