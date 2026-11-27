// One-off drain of the Telegram digest queue. Used to verify a notification end to end without
// waiting for 08:00. Not part of the service.
import { loadDotEnv } from '../env.js';
import { Backend } from '../http.js';
import { createLogger } from '../log.js';
import { TelegramClient } from './client.js';
import { runTgDigest } from './digest.js';

loadDotEnv();
const logger = createLogger('info');
const backend = new Backend({
  baseUrl: process.env.BACKEND_BASE_URL ?? 'http://127.0.0.1:3000',
  apiKey: (process.env.INTERNAL_API_KEY ?? process.env.SCRAPER_INTERNAL_API_KEY ?? '').trim(),
  logger,
});
const bot = new TelegramClient({ token: (process.env.TELEGRAM_BOT_TOKEN ?? '').trim(), perSecond: 5 });
const report = await runTgDigest({ bot, backend, logger });
console.log(JSON.stringify(report));
process.exit(0);
