/**
 * The one client for the Jobify backend.
 *
 * Every call is a short, typed wrapper over the API in `../backend/src/routes/`, with three
 * behaviours that matter operationally:
 *
 * - a 15 second timeout, so one hung request cannot wedge the digest;
 * - retry with backoff on 5xx and network errors only — a 4xx is an answer, not a hiccup,
 *   and retrying a 402 would spend nothing but would still be wrong;
 * - failures surface as `BackendError` carrying the backend's machine `code`, so callers
 *   branch on `out_of_credits` / `rate_limited` / `llm_unavailable` instead of prose, and
 *   never echo a raw response body to Discord.
 */
import type {
  AlertPrefs,
  AssistantAnswer,
  Category,
  Delivery,
  FeedPage,
  LinkInfo,
  LinkToken,
  MatchList,
  MatchRow,
  ProfilePatch,
  UserEnvelope,
} from './types.js';
import type { Logger } from './log.js';

export class BackendError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = 'BackendError';
  }
}

export type FetchImpl = typeof fetch;

export interface BackendOptions {
  baseUrl: string;
  apiKey: string;
  timeoutMs?: number;
  attempts?: number;
  fetchImpl?: FetchImpl;
  sleep?: (ms: number) => Promise<void>;
  logger?: Logger;
}

interface RequestOptions {
  body?: unknown;
  /** Internal routes need the key; `/jobs` and `/categories` are public. */
  authenticate?: boolean;
}

/** The backend's envelope: `{status, data, meta?}`. Callers that need `meta` use this. */
interface Envelope<T> {
  data: T;
  meta?: Record<string, unknown>;
}

const DEFAULT_TIMEOUT_MS = 15_000;
const DEFAULT_ATTEMPTS = 3;
const RETRY_BASE_MS = 400;

const defaultSleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

export class Backend {
  private readonly baseUrl: string;
  private readonly apiKey: string;
  private readonly timeoutMs: number;
  private readonly attempts: number;
  private readonly fetchImpl: FetchImpl;
  private readonly sleep: (ms: number) => Promise<void>;
  private readonly logger?: Logger;

  constructor(options: BackendOptions) {
    this.baseUrl = options.baseUrl.replace(/\/+$/, '');
    this.apiKey = options.apiKey;
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    this.attempts = Math.max(1, options.attempts ?? DEFAULT_ATTEMPTS);
    this.fetchImpl = options.fetchImpl ?? fetch;
    this.sleep = options.sleep ?? defaultSleep;
    if (options.logger) this.logger = options.logger;
  }

  private async request<T>(method: string, path: string, options: RequestOptions = {}): Promise<T> {
    return (await this.requestEnvelope<T>(method, path, options)).data;
  }

  /**
   * The same call, keeping `meta`. Used where the backend reports a decision in `meta`
   * rather than in `data` — `POST /internal/discord/profile` says `onboarding_complete` and
   * `alerts_enabled` there, and deriving them again in the bot is how the two drift.
   */
  private async requestEnvelope<T>(
    method: string,
    path: string,
    options: RequestOptions = {},
  ): Promise<Envelope<T>> {
    const url = `${this.baseUrl}${path}`;
    const headers: Record<string, string> = { accept: 'application/json' };
    if (options.authenticate !== false) headers['x-api-key'] = this.apiKey;
    if (options.body !== undefined) headers['content-type'] = 'application/json';

    let lastError: BackendError | undefined;
    for (let attempt = 1; attempt <= this.attempts; attempt += 1) {
      try {
        return await this.once<T>(method, url, headers, options.body);
      } catch (err) {
        const isBackend = err instanceof BackendError;
        const retryable = isBackend ? err.status === 0 || err.status >= 500 : false;
        if (!isBackend || !retryable || attempt === this.attempts) throw err;
        lastError = err;
        const wait = RETRY_BASE_MS * 2 ** (attempt - 1);
        this.logger?.warn('backend request failed, retrying', {
          method,
          path,
          status: err.status,
          code: err.code,
          attempt,
          wait_ms: wait,
        });
        await this.sleep(wait);
      }
    }
    throw lastError ?? new BackendError(0, 'network', 'Backend request failed');
  }

  private async once<T>(
    method: string,
    url: string,
    headers: Record<string, string>,
    body: unknown,
  ): Promise<Envelope<T>> {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);
    let response: Response;
    try {
      response = await this.fetchImpl(url, {
        method,
        headers,
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: controller.signal,
      });
    } catch (err) {
      const aborted = err instanceof Error && (err.name === 'AbortError' || err.name === 'TimeoutError');
      throw new BackendError(
        0,
        aborted ? 'timeout' : 'network',
        aborted ? 'The backend did not answer in time' : 'The backend is unreachable',
      );
    } finally {
      clearTimeout(timer);
    }

    const text = await response.text();
    let parsed: unknown = undefined;
    if (text.length > 0) {
      try {
        parsed = JSON.parse(text);
      } catch {
        parsed = undefined;
      }
    }

    if (!response.ok) {
      const envelope = parsed as { code?: unknown; message?: unknown } | undefined;
      const code = typeof envelope?.code === 'string' ? envelope.code : 'http_error';
      const message = typeof envelope?.message === 'string' ? envelope.message : `HTTP ${response.status}`;
      throw new BackendError(response.status, code, message);
    }

    const envelope = parsed as { status?: string; data?: T; meta?: Record<string, unknown> } | undefined;
    if (!envelope || envelope.data === undefined) {
      throw new BackendError(response.status, 'bad_payload', 'The backend returned an unexpected payload');
    }
    return { data: envelope.data, ...(envelope.meta ? { meta: envelope.meta } : {}) };
  }

  // -------------------------------------------------------------------------
  // Identity and profile
  // -------------------------------------------------------------------------

  async upsertUser(input: {
    discord_id: string;
    username: string;
    global_name?: string | null;
  }): Promise<{ user: UserEnvelope['user']; created: boolean; onboarding_complete: boolean }> {
    return this.request('POST', '/internal/discord/users', { body: input });
  }

  async saveProfile(
    patch: ProfilePatch,
  ): Promise<{ user: UserEnvelope['user']; onboarding_complete: boolean; alerts_enabled: boolean }> {
    const envelope = await this.requestEnvelope<UserEnvelope['user']>('POST', '/internal/discord/profile', {
      body: patch,
    });
    const meta = envelope.meta ?? {};
    // The backend is the authority on both: it knows that an "Other" degree completes
    // onboarding and whether the member switched alerts off, and the bot must not re-derive
    // either.
    return {
      user: envelope.data,
      onboarding_complete:
        typeof meta.onboarding_complete === 'boolean'
          ? meta.onboarding_complete
          : envelope.data.name.trim().length > 0 && envelope.data.education.length > 0,
      alerts_enabled: typeof meta.alerts_enabled === 'boolean' ? meta.alerts_enabled : false,
    };
  }

  /**
   * The member's own list, from the route that addresses them by their snowflake.
   *
   * The backend decides the categories at the end of onboarding and this is
   * the same read the portal's `/jobs/match` makes: identical rows, identical `matchScore`,
   * identical `meta`. `kind` narrows to jobs or tenders and is applied to the query, not after
   * the fact, so `/tenders` cannot be crowded out by jobs.
   */
  async jobsForUser(
    discordId: string,
    options: { limit?: number; kind?: 'job' | 'tender'; threshold?: number } = {},
  ): Promise<MatchList> {
    const search = new URLSearchParams();
    search.set('limit', String(options.limit ?? 5));
    if (options.kind) search.set('kind', options.kind);
    // Browsing asks for the member's areas, not only the rows that would justify an alert:
    // `threshold=0` lists everything live in them and the digest keeps the configured bar.
    if (typeof options.threshold === 'number') search.set('threshold', String(options.threshold));
    const envelope = await this.requestEnvelope<MatchRow[]>(
      'GET',
      `/internal/jobs/for-user/${encodeURIComponent(discordId)}?${search.toString()}`,
    );
    const meta = envelope.meta ?? {};
    const strings = (value: unknown): string[] =>
      Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
    return {
      rows: envelope.data,
      meta: {
        reason: typeof meta.reason === 'string' ? meta.reason : 'ok',
        total: typeof meta.total === 'number' ? meta.total : envelope.data.length,
        categories: strings(meta.categories),
        ...(typeof meta.categorySource === 'string' ? { categorySource: meta.categorySource } : {}),
        ...(typeof meta.threshold === 'number' ? { threshold: meta.threshold } : {}),
        ...(typeof meta.considered === 'number' ? { considered: meta.considered } : {}),
      },
    };
  }

  async getUser(discordId: string): Promise<UserEnvelope> {
    return this.request('GET', `/internal/discord/user/${encodeURIComponent(discordId)}`);
  }

  async linkForUser(userId: string): Promise<LinkInfo> {
    return this.request('GET', `/internal/discord/link/${encodeURIComponent(userId)}`);
  }

  async mintLink(discordId: string, ttlSeconds = 600): Promise<LinkToken> {
    return this.request('POST', '/internal/discord/link-token', {
      body: { discord_id: discordId, ttl_seconds: ttlSeconds },
    });
  }

  async setAlerts(discordId: string, enabled: boolean, mode?: 'daily' | 'instant'): Promise<{ alerts: AlertPrefs; alerts_enabled: boolean }> {
    return this.request('POST', '/internal/discord/alerts', {
      body: mode === undefined ? { discord_id: discordId, enabled } : { discord_id: discordId, enabled, mode },
    });
  }

  async setCompany(input: {
    discord_id: string;
    name: string;
    services: string[];
    location?: string | null;
    website?: string | null;
    description?: string | null;
  }): Promise<{ user: UserEnvelope['user']; company_set: boolean }> {
    const user = await this.request<UserEnvelope['user']>('POST', '/internal/discord/company', {
      body: input,
    });
    return { user, company_set: user.company != null };
  }

  // -------------------------------------------------------------------------
  // Assistant and catalogue
  // -------------------------------------------------------------------------

  async assistantQuery(discordId: string, message: string): Promise<AssistantAnswer> {
    return this.request('POST', '/internal/assistant/query', {
      body: { discord_id: discordId, message },
    });
  }

  async categories(): Promise<Category[]> {
    const data = await this.request<{ categories: Category[]; count: number }>('GET', '/categories', {
      authenticate: false,
    });
    return data.categories;
  }

  async listJobs(params: {
    kind?: 'job' | 'tender';
    category?: string;
    location?: string;
    query?: string;
    limit?: number;
  }): Promise<FeedPage> {
    const search = new URLSearchParams();
    if (params.kind) search.set('kind', params.kind);
    if (params.category) search.set('category', params.category);
    if (params.location) search.set('location', params.location);
    if (params.query) search.set('q', params.query);
    search.set('limit', String(params.limit ?? 20));
    // `GET /jobs` answers `{data: [...], meta: {total}}`, not `{data: {items}}`.
    const raw = await this.request<unknown>('GET', `/jobs?${search.toString()}`, { authenticate: false });
    return normalizeFeedPage(raw);
  }

  // -------------------------------------------------------------------------
  // Delivery queue
  // -------------------------------------------------------------------------

  async deliveries(limit = 100): Promise<Delivery[]> {
    const data = await this.request<{ deliveries: Delivery[]; count: number }>(
      'GET',
      `/internal/deliveries?limit=${limit}`,
    );
    return data.deliveries;
  }

  async markSent(ids: string[]): Promise<number> {
    if (ids.length === 0) return 0;
    const data = await this.request<{ marked: number }>('POST', '/internal/deliveries/sent', { body: { ids } });
    return data.marked;
  }

  async markFailed(failures: Array<{ id: string; reason: string }>): Promise<{ marked: number; alertsPaused: number }> {
    if (failures.length === 0) return { marked: 0, alertsPaused: 0 };
    return this.request('POST', '/internal/deliveries/failed', { body: { failures } });
  }

  async refreshMatches(): Promise<{ subscribers: number; queuedTotal: number }> {
    return this.request('POST', '/internal/matches/refresh');
  }
}

/**
 * `GET /jobs` returns the array in `data`, so `request` hands back a list. The wrapper keeps
 * `{items, total}` so the caller has one shape to read, with `total` = rows on this page
 * when the backend's `meta.total` is not part of the payload.
 */
export function normalizeFeedPage(raw: unknown): FeedPage {
  if (Array.isArray(raw)) return { items: raw as FeedPage['items'], total: raw.length };
  const withItems = raw as { items?: FeedPage['items']; total?: number } | undefined;
  if (withItems?.items) return { items: withItems.items, total: withItems.total ?? withItems.items.length };
  return { items: [], total: 0 };
}
