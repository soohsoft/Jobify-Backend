/**
 * Boot for the Telegram front end.
 *
 * Separate entry point from the Discord bot (`dist/index.js`) so the two can run side by side or
 * independently: `node dist/tg/index.js`. Same backend, same queue, same digest hour.
 *
 * Required env: TELEGRAM_BOT_TOKEN, BACKEND_BASE_URL (or the Discord bot's defaults),
 * INTERNAL_API_KEY. Everything else has a sane default.
 */
import { loadDotEnv } from '../env.js';
import { Backend } from '../http.js';
import { createLogger } from '../log.js';
import { localClock, isDigestDue } from '../scheduler.js';
import { TgBot } from './bot.js';
import { TelegramClient } from './client.js';
import { runTgDigest } from './digest.js';
import { TgSessions } from './prompts.js';

export interface TgConfig {
  token: string;
  backendBaseUrl: string;
  internalApiKey: string;
  appBaseUrl: string;
  digestTz: string;
  digestHour: number;
  perSecond: number;
}

function required(...names: string[]): string {
  for (const name of names) {
    const value = process.env[name]?.trim();
    if (value) return value;
  }
  throw new Error(`${names.join(' or ')} is required to run the Telegram bot`);
}

export function loadTgConfig(): TgConfig {
  const hour = Number(process.env.DIGEST_HOUR ?? 8);
  return {
    token: required('TELEGRAM_BOT_TOKEN'),
    backendBaseUrl: (process.env.BACKEND_BASE_URL ?? 'http://127.0.0.1:3000').replace(/\/+$/, ''),
    // The Discord bot's env calls this SCRAPER_INTERNAL_API_KEY; accept either name so one
    // file can serve both fronts.
    internalApiKey: required('INTERNAL_API_KEY', 'SCRAPER_INTERNAL_API_KEY'),
    appBaseUrl: (process.env.APP_BASE_URL ?? 'https://jobify.soohsoft.com').replace(/\/+$/, ''),
    digestTz: process.env.DIGEST_TZ ?? 'Africa/Nairobi',
    digestHour: Number.isFinite(hour) && hour >= 0 && hour <= 23 ? hour : 8,
    perSecond: Math.min(Math.max(Number(process.env.TELEGRAM_PER_SECOND ?? 25), 1), 30),
  };
}

async function main(): Promise<void> {
  loadDotEnv();
  const config = loadTgConfig();
  const logger = createLogger((process.env.LOG_LEVEL as 'debug' | 'info' | 'warn' | 'error') ?? 'info');

  const backend = new Backend({
    baseUrl: config.backendBaseUrl,
    apiKey: config.internalApiKey,
    logger,
  });
  const bot = new TelegramClient({ token: config.token, perSecond: config.perSecond });
  const sessions = new TgSessions();
  const handler = new TgBot({
    bot,
    backend,
    sessions,
    logger,
    appBaseUrl: config.appBaseUrl,
    digestTz: config.digestTz,
    digestHour: config.digestHour,
  });

  await handler.registerCommands();
  const me = await bot.call<{ id: number; username?: string }>('getMe', {});
  logger.info('telegram bot ready', { username: me.username ?? String(me.id), perSecond: config.perSecond });

  // TWO triggers, one drain:
  //
  // 1. the daily slot at DIGEST_HOUR, the same local-clock rule as Discord, so the two fronts
  //    cannot drift apart;
  // 2. a two-minute ticker that drains the queue the moment a scrape has queued something.
  //    That is what makes a fresh posting reach a member in minutes instead of at 08:00 —
  //    the scraper writes jobs, the matcher queues them, and this sends them.
  //
  // A member is messaged at most once every QUIET_HOURS, so three scrape cycles a day cannot
  // turn into three notifications.
  const QUIET_HOURS = Number(process.env.TELEGRAM_QUIET_HOURS ?? 5);
  const quietMs = (Number.isFinite(QUIET_HOURS) ? QUIET_HOURS : 5) * 60 * 60_000;
  const lastSent = new Map<string, number>();
  let lastRunDate = '';

  const drain = async (reason: 'slot' | 'queue'): Promise<void> => {
    const report = await runTgDigest({
      bot,
      backend,
      logger,
      shouldNotify: (userId) => {
        const sentAt = lastSent.get(userId) ?? 0;
        return Date.now() - sentAt >= quietMs;
      },
      onDelivered: (userId) => lastSent.set(userId, Date.now()),
    });
    if (report.dms > 0 || report.sent > 0) logger.info('telegram digest', { reason, ...report });
  };

  const tick = async (): Promise<void> => {
    const now = new Date();
    const due = isDigestDue(localClock(now, config.digestTz), config.digestHour, lastRunDate);
    try {
      if (due) {
        lastRunDate = localClock(now, config.digestTz).date;
        await drain('slot');
      } else {
        await drain('queue');
      }
    } catch (err) {
      logger.error('telegram digest failed', { err: String(err) });
    }
  };
  const timer = setInterval(() => void tick(), 120_000);
  timer.unref();
  void tick();

  const controller = new AbortController();
  const shutdown = (signal: string): void => {
    logger.info('telegram bot stopping', { signal });
    controller.abort();
    process.exit(0);
  };
  process.on('SIGINT', () => shutdown('SIGINT'));
  process.on('SIGTERM', () => shutdown('SIGTERM'));

  await bot.run((update) => handler.handle(update), controller.signal);
}

if (process.argv[1]?.endsWith('tg/index.js') || process.argv[1]?.endsWith('tg/index.ts')) {
  main().catch((err) => {
    console.error(JSON.stringify({ level: 'error', msg: 'telegram boot failed', err: String(err) }));
    process.exit(1);
  });
}
