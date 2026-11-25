/**
 * The backend's wire shapes, exactly as `../backend/src/{models,assistant}.rs` produce them.
 * Nothing here is invented: a field the backend does not send is not declared optional "just
 * in case", so a payload change surfaces as a typecheck failure rather than a silent
 * `undefined` in a Discord message.
 */

export interface MatchProfile {
  /** Canonical category slugs. These are the backend's query filter for this member. */
  categories: string[];
  keywords: string[];
  locations: string[];
  /** "llm" or "rules" — recorded by the backend, never chosen here. */
  category_source: string;
  updated_at: string;
}

export interface AlertPrefs {
  enabled: boolean;
  mode: string;
  last_sent_at: string;
  /** True when the MEMBER switched alerts off (the alerts Off button, the portal toggle), so onboarding
   *  (including a re-run) must not switch them back on. */
  user_disabled: boolean;
  excluded_employers: string[];
  excluded_categories: string[];
}

export interface CompanyProfile {
  name: string;
  services: string[];
  website?: string | null;
  description?: string | null;
}

export interface EducationEntry {
  level: string;
  /** The backend's canonical label, `Other: <their words>`, or empty for a non-degree level. */
  field: string;
}

export interface DiscordUser {
  id: string;
  name: string;
  email: string;
  role: string;
  provider: string;
  email_verified: boolean;
  location?: string | null;
  match_profile?: MatchProfile | null;
  alerts?: AlertPrefs | null;
  education_level?: string | null;
  /** Always present: an empty list means no degree has been recorded yet. */
  education: EducationEntry[];
  last_position?: string | null;
  experience_years?: number | null;
  sectors: string[];
  skills: string[];
  wants?: string | null;
  company?: CompanyProfile | null;
  discord_id?: string | null;
}

/** One scored row from `GET /jobs/match` or `GET /internal/jobs/for-user/{discord_id}`. */
export interface MatchRow {
  job: FeedJob;
  matchScore: number;
}

/**
 * `meta` of the two match reads. The backend reports the decision it made there, so the bot
 * renders what the backend decided instead of re-deriving it from the profile.
 */
export interface MatchMeta {
  /**
   * `no_match_profile` is a normal state on the way into onboarding; `below_threshold` means
   * there IS a profile but nothing matched closely enough — an honest empty list, answered
   * with the plain product sentence rather than a list of weak rows.
   */
  reason: string;
  total: number;
  categories: string[];
  categorySource?: string;
  /**
   * The backend's internal cut-off. Reported by the contract but never shown to a member:
   * the 0–100 fit on a row is displayed beside the title, the bar itself is not.
   */
  threshold?: number;
  /** Rows the matcher scored, so a fuller empty state could say "none of N" — never shown. */
  considered?: number;
}

export interface MatchList {
  rows: MatchRow[];
  meta: MatchMeta;
}

export interface Credits {
  balance: number;
  monthly_allowance: number;
  used_this_period: number;
  period_end: string;
  remaining_this_period: number;
}

/** `GET /internal/discord/user/{discord_id}` */
export interface UserEnvelope {
  user: DiscordUser;
  subscription_status: string;
  onboarding_complete: boolean;
  alerts_enabled: boolean;
  alerts: AlertPrefs;
  credits: Credits;
}

/** One row of an assistant answer (`SearchRow::to_json`). */
export interface JobRow {
  job_id: string;
  title: string;
  organization: string;
  location: string | null;
  kind: string;
  deadline: string | null;
  /** The job's page on the source portal, when the backend knows it. A row without one is
   *  rendered as text rather than linked to a portal path that may not exist publicly. */
  url?: string | null;
}

export interface AssistantAnswer {
  intent: string;
  reply: string;
  jobs: JobRow[];
  credits: Credits;
}

/** A feed item from the public `GET /jobs` — used for browsing and for the match count. */
export interface FeedJob {
  _id: string;
  title: string;
  organization: string;
  location?: string | null;
  deadline?: string | null;
  category?: string | null;
  kind?: string | null;
  url?: string | null;
  created_at?: string | null;
}

export interface FeedPage {
  items: FeedJob[];
  total: number;
}

/** One pending notification from `GET /internal/deliveries`. */
export interface Delivery {
  notificationId: string;
  userId: string;
  title: string;
  body: string;
  jobId?: string | null;
  /** The source page, when the queue carried it on the row itself. */
  url?: string | null;
  /** The 0–100 the job scored when it was queued, so the digest shows the same percentage
   *  the list did. Absent on rows queued before scores existed. */
  matchScore?: number | null;
  job?: FeedJob | null;
  createdAt?: string | null;
}

export interface Category {
  slug: string;
  label: string;
  group: string;
}

export interface LinkInfo {
  user_id: string;
  discord_id: string;
  username: string;
  global_name?: string | null;
}

export interface LinkToken {
  url: string;
  token: string;
  expires_at: string;
  expires_in_seconds: number;
}

export interface ProfilePatch {
  discord_id: string;
  name?: string;
  education_level?: string;
  /** Sent WHOLE on every save: the backend replaces the stored list with it. */
  education?: EducationEntry[];
  last_position?: string;
  experience_years?: number;
  sectors?: string[];
  skills?: string[];
  wants?: string;
  location?: string;
}
