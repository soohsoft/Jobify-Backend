/**
 * The backend's wire shapes, exactly as `../backend/src/{models,assistant}.rs` produce them.
 * Nothing here is invented: a field the backend does not send is not declared optional "just
 * in case", so a payload change surfaces as a typecheck failure rather than a silent
 * `undefined` in a Discord message.
 */

export interface MatchProfile {
  categories: string[];
  keywords: string[];
  locations: string[];
  updated_at: string;
}

export interface AlertPrefs {
  enabled: boolean;
  mode: string;
  last_sent_at: string;
  excluded_employers: string[];
  excluded_categories: string[];
}

export interface CompanyProfile {
  name: string;
  services: string[];
  website?: string | null;
  description?: string | null;
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
  experience_years?: number | null;
  sectors: string[];
  skills: string[];
  wants?: string | null;
  company?: CompanyProfile | null;
  discord_id?: string | null;
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
  location?: string;
  education_level?: string;
  experience_years?: number;
  sectors?: string[];
  skills?: string[];
  wants?: string;
}
