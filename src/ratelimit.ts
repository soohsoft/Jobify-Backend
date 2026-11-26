/**
 * Outbound Discord pacing.
 *
 * Discord answers a burst it dislikes with 429s and, if a bot keeps it up, with a global
 * restriction that makes the account look like spam. The safe shape is a slow, ordered trickle:
 * a token/slot bucket for the whole bot, a minimum spacing per member, and a retry that obeys
 * `retry_after` instead of hammering.
 *
 * What this deliberately does NOT do: touch a reply the member is waiting for. Interaction
 * answers (`deferUpdate`, `editReply`, `showModal`) go straight out — they are one request per
 * user action, nowhere near a limit — so nothing here adds latency to a conversation. Only
 * PROACTIVE sends (the daily digest, alert DMs) pass through this gate.
 *
 * The clock and the sleep are injected so the pacing is testable in virtual time.
 */

export type Sleep = (ms: number) => Promise<void>;

export const realSleep: Sleep = (ms) =>
  new Promise((resolve) => {
    setTimeout(resolve, ms);
  });

export interface RateLimiterOptions {
  /** Sends allowed back to back before the sustained rate applies. */
  burst: number;
  /** Sustained sends per second, bot-wide. */
  perSecond: number;
  /** Minimum gap between two sends to the same member, in ms. */
  perUserMs: number;
  now?: () => number;
  sleep?: Sleep;
}

/**
 * Discord's own numbers, with headroom: the documented DM creation limit is about 50 requests
 * per second bot-wide, and a single DM channel tolerates roughly one message per second. Half
 * of each keeps a large digest well clear of a 429 without turning a 1 000-member run into an
 * hour.
 */
export const DEFAULT_DM_LIMITS: Omit<RateLimiterOptions, 'now' | 'sleep'> = {
  burst: 4,
  perSecond: 4,
  perUserMs: 1_200,
};

export class DiscordRateLimiter {
  private readonly burst: number;
  private readonly perSecond: number;
  private readonly perUserMs: number;
  private readonly now: () => number;
  private readonly sleep: Sleep;
  /** Tokens available right now; refilled at `perSecond`, capped at `burst`. */
  private tokens: number;
  private lastRefill: number;
  /** Per member: the earliest time THEIR next message may go out. */
  private readonly nextForUser = new Map<string, number>();

  constructor(options: RateLimiterOptions) {
    if (options.burst < 1) throw new Error('burst must be at least 1');
    if (options.perSecond <= 0) throw new Error('perSecond must be positive');
    this.burst = options.burst;
    this.perSecond = options.perSecond;
    this.perUserMs = options.perUserMs;
    this.now = options.now ?? Date.now;
    this.sleep = options.sleep ?? realSleep;
    this.tokens = options.burst;
    this.lastRefill = this.now();
  }

  /**
   * Waits until a send to `key` (a Discord id) is allowed, then spends a token.
   *
   * The per-member gap is claimed first — so two concurrent sends to the same member queue in
   * arrival order instead of both reading a free slot — and the bot-wide bucket is spent after.
   */
  async acquire(key: string): Promise<void> {
    const now = this.now();
    const previous = this.nextForUser.get(key);
    if (previous === undefined) {
      this.nextForUser.set(key, now + this.perUserMs);
    } else {
      const userWait = previous - now;
      this.nextForUser.set(key, previous + this.perUserMs);
      if (userWait > 0) await this.sleep(userWait);
    }
    if (this.nextForUser.size > 2_048) this.prune(now);

    // Token bucket: a short burst, then the sustained rate.
    for (;;) {
      this.refill();
      if (this.tokens >= 1) {
        this.tokens -= 1;
        return;
      }
      const waitMs = Math.ceil(((1 - this.tokens) / this.perSecond) * 1000);
      await this.sleep(waitMs);
    }
  }

  /** Drops members whose window closed long ago: the map must not grow with the user table. */
  private prune(now: number): void {
    const cutoff = now - this.perUserMs * 10;
    for (const [key, next] of this.nextForUser) {
      if (next < cutoff) this.nextForUser.delete(key);
    }
  }

  private refill(): void {
    const now = this.now();
    if (now <= this.lastRefill) return;
    this.tokens = Math.min(this.burst, this.tokens + ((now - this.lastRefill) / 1000) * this.perSecond);
    this.lastRefill = now;
  }
}

/** True when a discord.js error is a rate limit. */
export function isRateLimitError(err: unknown): boolean {
  if (typeof err !== 'object' || err === null) return false;
  const candidate = err as { status?: number; httpStatus?: number; name?: string; retryAfter?: number };
  return (
    candidate.status === 429 ||
    candidate.httpStatus === 429 ||
    candidate.name === 'RateLimitError' ||
    typeof candidate.retryAfter === 'number'
  );
}

/**
 * How long Discord asked us to wait, in ms, or null when the error says nothing.
 *
 * discord.js exposes it as `retryAfter` (seconds) and often as `rawError.retry_after`; the REST
 * helpers also put it on `retry_after`. All three are read so a plain HTTP error object works
 * in tests and a library error works in production.
 */
export function retryAfterMs(err: unknown): number | null {
  if (typeof err !== 'object' || err === null) return null;
  const candidate = err as {
    retryAfter?: number;
    retry_after?: number;
    rawError?: { retry_after?: number };
  };
  const seconds = candidate.retryAfter ?? candidate.retry_after ?? candidate.rawError?.retry_after;
  if (typeof seconds !== 'number' || !Number.isFinite(seconds) || seconds < 0) return null;
  return Math.ceil(seconds * 1000);
}

/** Discord's `10003 Unknown Channel`: the cached DM channel is gone, not rate limited. */
export function isUnknownChannel(err: unknown): boolean {
  if (typeof err !== 'object' || err === null) return false;
  const candidate = err as { code?: number | string; message?: string };
  if (candidate.code === 10003 || candidate.code === '10003') return true;
  return typeof candidate.message === 'string' && /unknown channel/i.test(candidate.message);
}

export interface RetryOptions {
  attempts?: number;
  /** Base backoff for a 5xx or a 429 with no stated delay. */
  baseMs?: number;
  sleep?: Sleep;
  /** Deterministic in tests; production adds up to 250 ms of jitter. */
  jitterMs?: number;
  random?: () => number;
  /** Called before each wait, so a rate limit shows up in the log instead of vanishing. */
  onRetry?: (waitMs: number, err: unknown) => void;
  /** Called before a retry when the cached channel is stale, to re-open it. */
  onRetryable?: () => Promise<void> | void;
}

/**
 * Runs `send`, waiting out a 429 for exactly as long as Discord asked.
 *
 * A 429 is not a failure — it is an instruction. Retrying immediately is what turns one 429 into
 * a restriction; waiting `retry_after` plus a little jitter is what keeps the account clean.
 */
export async function withDiscordRetry<T>(
  send: () => Promise<T>,
  options: RetryOptions = {},
): Promise<T> {
  const attempts = options.attempts ?? 3;
  const baseMs = options.baseMs ?? 500;
  const sleep = options.sleep ?? realSleep;
  const jitterMs = options.jitterMs ?? 250;
  const random = options.random ?? Math.random;

  let lastError: unknown;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      return await send();
    } catch (err) {
      lastError = err;
      const isLast = attempt === attempts - 1;
      if (isLast) break;
      const asked = retryAfterMs(err);
      const backoff = asked ?? baseMs * 2 ** attempt;
      const jitter = jitterMs > 0 ? Math.floor(random() * jitterMs) : 0;
      // A stale cached channel is worth re-opening; a 403 (blocked DMs) is final.
      const staleChannel = isUnknownChannel(err);
      const retryable =
        staleChannel || asked !== null || isRateLimitError(err) || isServerError(err) || isNetworkError(err);
      if (!retryable) break;
      if (staleChannel) await options.onRetryable?.();
      options.onRetry?.(backoff + jitter, err);
      await sleep(backoff + jitter);
    }
  }
  throw lastError;
}

function isServerError(err: unknown): boolean {
  if (typeof err !== 'object' || err === null) return false;
  const status = (err as { status?: number; httpStatus?: number }).status ?? (err as { httpStatus?: number }).httpStatus;
  return typeof status === 'number' && status >= 500 && status < 600;
}

function isNetworkError(err: unknown): boolean {
  if (typeof err !== 'object' || err === null) return false;
  const code = (err as { code?: string }).code;
  return code === 'ECONNRESET' || code === 'ETIMEDOUT' || code === 'ENOTFOUND' || code === 'UND_ERR_CONNECT_TIMEOUT';
}
