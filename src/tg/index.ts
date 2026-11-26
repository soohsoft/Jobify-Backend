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

function required(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required to run the Telegram bot`);
  return value;
}

export function loadTgConfig(): TgConfig {
  const hour = Number(process.env.DIGEST_HOUR ?? 8);
  return {
    token: required('TELEGRAM_BOT_TOKEN'),
    backendBaseUrl: (process.env.BACKEND_BASE_URL ?? 'http://127.0.0.1:3000').replace(/\/+$/, ''),
    internalApiKey: required('INTERNAL_API_KEY'),
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

  // One digest a day at DIGEST_HOUR local time; the same local-clock rule as the Discord
  // scheduler, so the two fronts cannot drift apart.
  let lastRunDate = '';
  const tick = async (): Promise<void> => {
    const now = new Date();
    if (!isDigestDue(localClock(now, config.digestTz), config.digestHour, lastRunDate)) return;
    lastRunDate = localClock(now, config.digestTz).date;
    try {
      const report = await runTgDigest({ bot, backend, logger });
      logger.info('telegram digest', { ...report });
    } catch (err) {
      logger.error('telegram digest failed', { err: String(err) });
    }
  };
  const timer = setInterval(() => void tick(), 60_000);
  timer.unref();

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
