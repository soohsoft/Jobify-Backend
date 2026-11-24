/**
 * Every string the bot says, built by a pure function so the wording can be tested without a
 * gateway. The rules encoded here:
 *
 * - a row is only ever rendered from data the backend returned — there is no fallback that
 *   invents a title, an employer or a deadline;
 * - a row links to the job's own source page; the portal is a fallback only, and a local
 *   portal base (or any localhost URL) produces no link at all rather than a dead one;
 * - at most 5 matches per digest DM (Discord punishes walls of text, and a 30-row alert is
 *   read by nobody);
 * - a failure is mapped to the sentence that actually helps, never to a raw error body.
 */
import type { AssistantAnswer, Delivery, JobRow } from './types.js';

export const MAX_DIGEST_ROWS = 5;
export const MAX_REPLY_ROWS = 3;
export const MAX_BROWSE_ROWS = 5;

export interface RowInput {
  title: string;
  organization?: string | null;
  location?: string | null;
  deadline?: string | null;
  kind?: string | null;
  jobId?: string | null;
  /** The job's real page on the source portal. Preferred over any portal link. */
  url?: string | null;
  /** The backend's 0–100 fit for this member. Absent on a surface that has no score. */
  matchScore?: number | null;
}

/** The cut-off every current backend applies; used only when `meta.threshold` is absent. */
export const DEFAULT_MATCH_THRESHOLD = 70;

/**
 * The backend's score as Discord text: `87% match`.
 *
 * The percentage is the backend's decision, never computed here: a missing, non-finite or
 * out-of-range value renders no label at all, because a made-up number is worse than none.
 */
export function matchLabel(score: number | null | undefined): string | null {
  if (typeof score !== 'number' || !Number.isFinite(score)) return null;
  const clamped = Math.max(0, Math.min(100, Math.round(score)));
  return `${clamped}% match`;
}

/**
 * The one sentence for "you have a profile, but nothing cleared the cut-off".
 *
 * This is the below_threshold empty state: the bot must not list weak rows and must not
 * pretend the member has no profile. Naming the number makes the promise checkable.
 */
export function belowThresholdCopy(threshold: number | null | undefined): string {
  const cut =
    typeof threshold === 'number' && Number.isFinite(threshold)
      ? Math.round(threshold)
      : DEFAULT_MATCH_THRESHOLD;
  return `Nothing scores above ${cut}% for you right now — I will message you when something does.`;
}

export function truncate(text: string, max: number): string {
  const chars = [...text];
  return chars.length <= max ? text : `${chars.slice(0, max - 1).join('')}…`;
}

/**
 * A URL is only a link if it is absolute http(s) AND not this machine. A `localhost` link in
 * a Discord DM is dead for every reader: it resolves to THEIR machine, so a row that cannot
 * be linked is rendered as text instead of a promise that goes nowhere.
 */
export function isLoopback(hostname: string): boolean {
  const host = hostname.trim().toLowerCase().replace(/^\[|\]$/g, '');
  if (host === 'localhost' || host === '::1' || host === '0.0.0.0') return true;
  if (host.endsWith('.localhost')) return true;
  return /^127(\.\d{1,3}){3}$/.test(host);
}

/** The row's own source page, or `null` when it is missing, malformed or points at this box. */
export function sourceLink(url: string | null | undefined): string | null {
  if (typeof url !== 'string') return null;
  const candidate = url.trim();
  if (!/^https?:\/\//i.test(candidate)) return null;
  let parsed: URL;
  try {
    parsed = new URL(candidate);
  } catch {
    return null;
  }
  if (!parsed.hostname || isLoopback(parsed.hostname)) return null;
  return candidate;
}

/** The portal's own job page — only ever a fallback, and never when the portal is local. */
export function jobLink(appBaseUrl: string, jobId: string | null | undefined): string | null {
  if (!jobId) return null;
  const base = appBaseUrl.trim().replace(/\/+$/, '');
  let host: string;
  try {
    host = new URL(base).hostname;
  } catch {
    return null;
  }
  if (!host || isLoopback(host)) return null;
  return `${base}/jobs/${jobId}`;
}

/** `Title — 87% match — Org — Location — closes DATE`, skipping what the source did not publish. */
export function rowLine(row: RowInput, index?: number): string {
  const parts: string[] = [`**${truncate(row.title, 110)}**`];
  // The fit sits right beside the title, so the ranking is visible on the line itself.
  const score = matchLabel(row.matchScore);
  if (score) parts.push(score);
  if (row.organization?.trim()) parts.push(truncate(row.organization.trim(), 60));
  parts.push(row.location?.trim() ? truncate(row.location.trim(), 50) : 'location not listed');
  parts.push(row.deadline?.trim() ? `closes ${row.deadline.trim()}` : 'no deadline listed');
  const body = parts.join(' — ');
  return index === undefined ? body : `${index}. ${body}`;
}

export function formatRows(
  rows: RowInput[],
  options: { appBaseUrl: string; max?: number; startAt?: number },
): string {
  const max = options.max ?? MAX_DIGEST_ROWS;
  return rows
    .slice(0, max)
    .map((row, i) => {
      const head = rowLine(row, (options.startAt ?? 1) + i);
      // The source page is the only useful destination; the portal is a fallback for rows the
      // backend could not resolve, and a local portal renders no link at all.
      const link = sourceLink(row.url) ?? jobLink(options.appBaseUrl, row.jobId);
      return link ? `${head}\n${link}` : head;
    })
    .join('\n');
}

/** The reply to one assistant answer: its own sentence, then the real rows as links. */
export function formatAssistantReply(
  answer: AssistantAnswer,
  options: { appBaseUrl: string; maxRows?: number },
): string {
  const reply = answer.reply.trim();
  const maxRows = options.maxRows ?? MAX_REPLY_ROWS;
  if (answer.jobs.length === 0) return reply;
  const rows = formatRows(answer.jobs.map(jobRowToInput), {
    appBaseUrl: options.appBaseUrl,
    max: maxRows,
  });
  const extra = answer.jobs.length > maxRows ? `\n_…and ${answer.jobs.length - maxRows} more in the portal._` : '';
  return `${reply}\n\n${rows}${extra}`;
}

/**
 * The 5-per-DM cap, made explicit rather than implied by a slice somewhere: the caller has to
 * decide what happens to the remainder (it stays queued for tomorrow, unmarked).
 */
export function capRows<T>(rows: T[], max = MAX_DIGEST_ROWS): { delivered: T[]; remaining: number } {
  if (rows.length <= max) return { delivered: rows, remaining: 0 };
  return { delivered: rows.slice(0, max), remaining: rows.length - max };
}

export function digestMessage(
  rows: RowInput[],
  options: { appBaseUrl: string; max?: number; extraWaiting?: number },
): string {
  const max = options.max ?? MAX_DIGEST_ROWS;
  const { delivered, remaining } = capRows(rows, max);
  const title = delivered.length === 1 ? '**1 new match for you on Jobify**' : `**${delivered.length} new matches for you on Jobify**`;
  const lines = [title, '', formatRows(delivered, { appBaseUrl: options.appBaseUrl, max })];
  const waiting = remaining + (options.extraWaiting ?? 0);
  if (waiting > 0) lines.push('', `_${waiting} more waiting — they'll come in tomorrow's digest._`);
  lines.push('', '_Reply here to ask about any of them, /alerts to change the schedule, /stop to switch these off._');
  return lines.join('\n');
}

/** A delivery row, preferring the job document the queue embedded. */
export function deliveryRow(delivery: Delivery): RowInput {
  const job = delivery.job;
  if (job) {
    return {
      title: job.title,
      organization: job.organization,
      location: job.location ?? null,
      deadline: job.deadline ?? null,
      kind: job.kind ?? null,
      jobId: job._id ?? delivery.jobId ?? null,
      // The queue has carried the source page both on the row and inside the embedded job,
      // depending on the backend version that queued it — accept either.
      url: job.url ?? delivery.url ?? null,
      // The score is recorded with the notification, so a row queued before scores existed
      // simply renders without one.
      matchScore: delivery.matchScore ?? null,
    };
  }
  return {
    title: delivery.title,
    organization: null,
    location: null,
    deadline: null,
    jobId: delivery.jobId ?? null,
    url: delivery.url ?? null,
    matchScore: delivery.matchScore ?? null,
  };
}

export function jobRowToInput(row: JobRow): RowInput {
  return {
    title: row.title,
    organization: row.organization,
    location: row.location,
    deadline: row.deadline,
    kind: row.kind,
    jobId: row.job_id,
    url: row.url ?? null,
  };
}

/**
 * Why a DM failed, in the vocabulary the delivery API understands.
 *
 * `blocked` and `deactivated` are terminal on the backend: they pause that member's alerts,
 * because retrying an address that will never work looks identical to a healthy queue from
 * the outside. Anything else is transient and is recorded without pausing.
 */
export function dmFailureReason(err: unknown): 'blocked' | 'deactivated' | 'transient' {
  const candidate = err as { code?: unknown; status?: unknown } | null;
  const code = typeof candidate?.code === 'number' ? candidate.code : undefined;
  const status = typeof candidate?.status === 'number' ? candidate.status : undefined;
  // 50007: Cannot send messages to this user (blocked the bot or closed DMs).
  // 10013/10007: Unknown user / unknown member. 403/404 as plain HTTP answers.
  if (code === 50007 || status === 403) return 'blocked';
  if (code === 10013 || code === 10007 || status === 404) return 'deactivated';
  return 'transient';
}

export type AssistantFailure = {
  /** What to say. Never contains the backend's body. */
  content: string;
  /** Whether to offer the Buy Credits action. */
  offerPurchase: boolean;
};

/** The four answers the assistant route can give that are not a normal reply. */
export function assistantFailureCopy(status: number, code: string): AssistantFailure {
  if (status === 402 || code === 'out_of_credits') {
    return {
      content:
        "You've used your free searches for this period and there are no purchased credits left.\nBuy credits on your account page to keep asking — open it with the button below, or with `/account`.",
      offerPurchase: true,
    };
  }
  if (status === 429 || code === 'rate_limited') {
    return { content: 'Slow down a little — that is a lot of questions at once. Try again in a minute.', offerPurchase: false };
  }
  if (status === 502 || code === 'llm_unavailable') {
    return {
      content: 'The search assistant is unavailable right now. Nothing was charged — please try again shortly.',
      offerPurchase: false,
    };
  }
  if (status === 404 || code === 'not_found') {
    return { content: 'Run `/start` first so I know who you are, then ask me again.', offerPurchase: false };
  }
  if (code === 'timeout' || code === 'network') {
    return { content: 'I could not reach Jobify just now. Please try again in a moment.', offerPurchase: false };
  }
  return { content: 'Something went wrong on my side. Please try again in a moment.', offerPurchase: false };
}

/** The `/help` text: exactly ten lines, no marketing. */
export function helpText(): string {
  return [
    '**Jobify — Somali jobs and tenders, inside Discord**',
    '`/start` — set up your profile (about a minute)',
    '`/jobs [query]` — search jobs, or list the newest in your fields',
    '`/tenders [query]` — the same for tenders',
    '`/alerts` — instant, daily or off; the daily digest arrives at 08:00 EAT',
    '`/credits` — free searches left this period, and how to buy more',
    '`/account` — a private one-time portal link (payment, company profile)',
    '`/company` — set your company name and services',
    '`/help` — this list. `/stop` — alerts off, account and profile kept.',
    'Or just message me here with what you are looking for — that is a search too.',
  ].join('\n');
}
