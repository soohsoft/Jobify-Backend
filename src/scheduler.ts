/**
 * The daily digest timer.
 *
 * A minute-granularity tick rather than a computed `setTimeout(msUntilTomorrow)`: a timer
 * that long drifts across suspend, NTP steps and DST, and the failure mode (silence) is
 * invisible. The tick reads the wall clock in DIGEST_TZ through Intl, which handles any IANA
 * zone, and a per-day guard makes the run idempotent across restarts.
 *
 * The window is deliberately 10 minutes rather than exactly :00 — a bot that restarts at
 * 08:03 during a deploy still sends that morning's digest; `refresh_matches` refuses to queue
 * twice in one Nairobi day, so a second run inside the window can only drain leftovers.
 */
import type { Logger } from './log.js';

export interface LocalClock {
  /** YYYY-MM-DD in the configured zone. */
  date: string;
  hour: number;
  minute: number;
}

export function localClock(now: Date, timeZone: string): LocalClock {
  const parts = new Intl.DateTimeFormat('en-CA', {
    timeZone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
  }).formatToParts(now);
  const get = (type: Intl.DateTimeFormatPartTypes): string =>
    parts.find((part) => part.type === type)?.value ?? '00';
  return {
    date: `${get('year')}-${get('month')}-${get('day')}`,
    // Some engines report midnight as 24 with hour12: false; normalise it.
    hour: Number(get('hour')) % 24,
    minute: Number(get('minute')),
  };
}

export function isDigestDue(
  clock: LocalClock,
  digestHour: number,
  lastRunDate: string | null,
  windowMinutes = 10,
): boolean {
  if (clock.date === lastRunDate) return false;
  if (clock.hour !== digestHour) return false;
  return clock.minute < windowMinutes;
}

export interface DigestSchedulerOptions {
  timeZone: string;
  hour: number;
  run: () => Promise<unknown>;
  logger: Logger;
  intervalMs?: number;
  windowMinutes?: number;
  now?: () => Date;
}

export class DailyDigestScheduler {
  private timer: NodeJS.Timeout | undefined;
  private lastRunDate: string | null = null;
  private running = false;
  private readonly options: DigestSchedulerOptions;

  constructor(options: DigestSchedulerOptions) {
    this.options = options;
  }

  start(): void {
    if (this.timer) return;
    const interval = this.options.intervalMs ?? 30_000;
    this.timer = setInterval(() => {
      void this.tick();
    }, interval);
    // Do not hold the process open on the timer alone; the gateway connection does that.
    this.timer.unref?.();
    this.options.logger.info('digest scheduler armed', {
      time_zone: this.options.timeZone,
      hour: this.options.hour,
      check_every_ms: interval,
    });
    void this.tick();
  }

  stop(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = undefined;
    }
  }

  /** Exposed so a test (or an operator) can force one evaluation. */
  async tick(): Promise<void> {
    if (this.running) return;
    const now = this.options.now ? this.options.now() : new Date();
    const clock = localClock(now, this.options.timeZone);
    if (!isDigestDue(clock, this.options.hour, this.lastRunDate, this.options.windowMinutes)) return;

    this.running = true;
    this.lastRunDate = clock.date;
    const started = Date.now();
    try {
      this.options.logger.info('digest starting', { date: clock.date, hour: clock.hour });
      await this.options.run();
      this.options.logger.info('digest complete', { duration_ms: Date.now() - started });
    } catch (err) {
      // A thrown digest is logged, not fatal: the next day's tick tries again, and the queue
      // is the durable state, not this process.
      this.options.logger.error('digest failed', {
        message: err instanceof Error ? err.message : String(err),
      });
    } finally {
      this.running = false;
    }
  }

  /** For diagnostics and tests. */
  get lastRun(): string | null {
    return this.lastRunDate;
  }
}
