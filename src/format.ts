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
 * The one empty state: "you have a profile, but nothing matches right now".
 *
 * The matcher's score and its cut-off are internal — a member is told what happens next, not
 * how the ranking was computed. This single sentence is shared by `/jobs`, `/tenders`, the
 * completion message, the assistant when it finds no rows and the digest, so the wording can
 * never drift between surfaces.
 */
export const NO_MATCHES_SENTENCE =
  "No jobs match your qualifications right now. We'll notify you when one is posted.";

/** The backend's own "found nothing" line, mapped to the product sentence above. */
const BACKEND_NOTHING_MATCHES = 'Nothing live matches that right now.';

export function noMatchesCopy(): string {
  return NO_MATCHES_SENTENCE;
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

/**
 * The date part of a deadline, as the owner's line format spells it.
 *
 * A source that sends `2026-12-03T00:00:00Z` must render `closes 2026-12-03`, not the raw
 * timestamp: the line is a date, and a time of day is noise the board never published.
 */
export function dateOnly(value: string): string {
  const match = /^(\d{4}-\d{2}-\d{2})/.exec(value.trim());
  return match ? match[1] : value.trim();
}

/**
 * One job, in the owner's exact shape:
 *
 *   1. **<Title>** — <ORGANISATION> — <Location> — closes <YYYY-MM-DD>
 *
 * A missing organisation or location keeps its slot ("organisation not listed", "location
 * not listed") so every line reads the same way, and an undated job ends with
 * "no deadline listed" instead of an invented date.
 *
 * `matchScore` is deliberately NOT part of this line unless `withScore` says so, and only
 * the daily digest says so. The percentage belongs to the digest; a chat list is a list of
 * jobs, and a score beside each one is noise the member did not ask for.
 */
export function rowLine(row: RowInput, index?: number, withScore = false): string {
  const parts: string[] = [`**${truncate(row.title, 110)}**`];
  if (withScore) {
    const score = matchLabel(row.matchScore);
    if (score) parts.push(score);
  }
  parts.push(row.organization?.trim() ? truncate(row.organization.trim(), 60) : 'organisation not listed');
  parts.push(row.location?.trim() ? truncate(row.location.trim(), 50) : 'location not listed');
  parts.push(row.deadline?.trim() ? `closes ${dateOnly(row.deadline)}` : 'no deadline listed');
  const body = parts.join(' — ');
  return index === undefined ? body : `${index}. ${body}`;
}

/** The identity of a row, so the same job can never be printed twice in one list. */
function rowKey(row: RowInput): string {
  const norm = (value: string | null | undefined): string =>
    (value ?? '')
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, ' ')
      .trim();
  if (row.jobId) return `id:${row.jobId}`;
  return `t:${norm(row.title)}|${norm(row.organization)}|${norm(row.location)}`;
}

/**
 * Rows with duplicates removed, first occurrence kept.
 *
 * The backend de-duplicates too, but a list is the thing the member reads: if two rows ever
 * arrive for one vacancy (a retry, a second source, a caller that concatenated two pages)
 * this is what stops the same job appearing twice on screen.
 */
export function uniqueRows(rows: RowInput[]): RowInput[] {
  const seen = new Set<string>();
  return rows.filter((row) => {
    const key = rowKey(row);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/**
 * The rows, each on its own blank-line-separated block: the numbered line, the bare source
 * URL on the next line, then a blank line. Nothing else — no header, no footer, no count.
 */
/**
 * The rule that frames a list.
 *
 * Discord has no horizontal rule, so the line is drawn with box-drawing characters: they are
 * monospaced in both the desktop and mobile clients, which keeps the two ends aligned no
 * matter what the font does to the label between them. The label is what makes the frame do
 * work — a member scanning their DMs sees "JOBS" or "TENDERS" before reading a single row,
 * and "END" tells them the list finished rather than the message being cut off.
 */
export function dividerLine(label: string): string {
  return `${'─'.repeat(7)}  ${label}  ${'─'.repeat(7)}`;
}

export function formatRows(
  rows: RowInput[],
  options: { appBaseUrl: string; max?: number; startAt?: number; showScore?: boolean; title?: string },
): string {
  const max = options.max ?? MAX_DIGEST_ROWS;
  const blocks = uniqueRows(rows)
    .slice(0, max)
    .map((row, i) => {
      const head = rowLine(row, (options.startAt ?? 1) + i, options.showScore ?? false);
      // The source page is the only useful destination; the portal is a fallback for rows the
      // backend could not resolve, and a local portal renders no link at all.
      const link = sourceLink(row.url) ?? jobLink(options.appBaseUrl, row.jobId);
      // Angle brackets stop Discord unfurling the source page. Without them every row appears
      // twice in the channel: once as the line the bot wrote and once as the site's preview
      // card, which reads as a duplicate job. The link stays clickable.
      return link ? `${head}\n<${link}>` : head;
    });
  if (blocks.length === 0) return '';
  const body = blocks.join('\n\n');
  // No title, no frame: an empty heading would draw two rules around nothing.
  if (!options.title) return `${body}\n`;
  return `${dividerLine(options.title)}\n${body}\n${dividerLine('END')}\n`;
}

/** The reply to one assistant answer: its own sentence, then the real rows as links. */
export function formatAssistantReply(
  answer: AssistantAnswer,
  options: { appBaseUrl: string; maxRows?: number },
): string {
  const reply = answer.reply.trim();
  const maxRows = options.maxRows ?? MAX_REPLY_ROWS;
  // The assistant is told to say the backend's "nothing live" line when it found no rows.
  // Users see the product sentence instead: whether a score existed is not their business.
  if (answer.jobs.length === 0) {
    return reply === BACKEND_NOTHING_MATCHES ? NO_MATCHES_SENTENCE : reply;
  }
  // ONLY the rows. The owner's list format is the whole message: no sentence about what was
  // searched for, and no "…and N more" footer — a chat list is the jobs, nothing around them.
  return formatRows(answer.jobs.map(jobRowToInput), {
    appBaseUrl: options.appBaseUrl,
    max: maxRows,
  }).trimEnd();
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
  // Count the rows the member will actually see: the same vacancy delivered twice is one
  // match, and a header saying "2 new matches" over a single line is a lie.
  const { delivered, remaining } = capRows(uniqueRows(rows), max);
  // A digest with nothing to send says the product sentence, never "0 new matches".
  if (delivered.length === 0) return NO_MATCHES_SENTENCE;
  // The opening sentence is what a phone shows on its lock screen, so it carries the news
  // itself: count, title, employer. Push previews are plain text — bold markers arrive as
  // literal `**` — so this line has no markdown in it at all, and the framed list below it is
  // what the member sees once they open the message.
  const lead = delivered[0];
  const where = [lead?.organization, lead?.location].filter(Boolean).join(' — ');
  const leadLine = `${lead?.title ?? ''}${where ? ` — ${where}` : ''}`.slice(0, 110);
  const title =
    delivered.length === 1
      ? `1 new match for you on Jobify: ${leadLine}`
      : `${delivered.length} new matches for you on Jobify — first: ${leadLine}`;
  // The digest is the ONE surface that keeps the percentage: it is the list the member gets
  // unprompted, and the score is how the backend decided to send it at all.
  const lines = [
    title,
    '',
    // Framed like every other list: the DM arrives unprompted, so it must announce itself.
    // trimEnd: formatRows closes with a newline for block layout, and inside a message with
    // lines after it that newline becomes a second blank line before the footer.
    formatRows(delivered, {
      appBaseUrl: options.appBaseUrl,
      max,
      showScore: true,
      title: 'NEW FOR YOU',
    }).trimEnd(),
  ];
  const waiting = remaining + (options.extraWaiting ?? 0);
  if (waiting > 0) lines.push('', `_${waiting} more waiting — they'll come in tomorrow's digest._`);
  lines.push('', '_Reply here to ask about any of them. Your account page (`/account`) changes the schedule or switches these off._');
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
    return { content: 'I do not have your account yet — run `/account` once so I know who you are, then ask me again.', offerPurchase: false };
  }
  if (code === 'timeout' || code === 'network') {
    return { content: 'I could not reach Jobify just now. Please try again in a moment.', offerPurchase: false };
  }
  return { content: 'Something went wrong on my side. Please try again in a moment.', offerPurchase: false };
}
