/**
 * Environment and configuration.
 *
 * The .env file is sourced by run.sh in the deployed shape, but `npm start`, `npm run dev`
 * and `npm test` are also expected to work from a fresh shell, so the file is parsed here
 * too. Values already present in the process environment win: a systemd unit's
 * Environment= must not be silently overridden by a stale file on disk.
 */
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';

export interface Config {
  discordToken: string;
  discordAppId: string;
  backendBaseUrl: string;
  internalApiKey: string;
  appBaseUrl: string;
  digestTz: string;
  digestHour: number;
  /**
   * Outbound DM pacing. Defaults sit at 20/s bot-wide — a fifth of Discord's documented
   * ceiling — so a 5 000-member digest finishes in about four minutes.
   */
  dmPerSecond: number;
  dmBurst: number;
  /** Minimum gap between two DMs to the same member. */
  dmPerUserMs: number;
  /** A guild channel where plain messages are answered without a mention; unset = mention only. */
  freeChannelId?: string;
}

/** Reads KEY=VALUE lines. Missing file is not an error; bad lines are skipped. */
export function loadDotEnv(path = resolve(process.cwd(), '.env'), target: NodeJS.ProcessEnv = process.env): void {
  if (!existsSync(path)) return;
  const raw = readFileSync(path, 'utf8');
  for (const line of raw.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('#')) continue;
    const eq = trimmed.indexOf('=');
    if (eq <= 0) continue;
    const key = trimmed.slice(0, eq).trim();
    let value = trimmed.slice(eq + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"') && value.length >= 2) ||
      (value.startsWith("'") && value.endsWith("'") && value.length >= 2)
    ) {
      value = value.slice(1, -1);
    }
    if (target[key] === undefined) target[key] = value;
  }
}

function required(env: NodeJS.ProcessEnv, name: string): string {
  const value = env[name]?.trim();
  if (!value) {
    // Name the variable, never the value: this string reaches the log on a bad deploy.
    throw new Error(`Missing required environment variable ${name}`);
  }
  return value;
}

/** Parses a positive integer, falling back to `fallback` and clamping into [min, max]. */
function clampInt(raw: string | undefined, fallback: number, min: number, max: number): number {
  const value = Number.parseInt((raw ?? '').trim(), 10);
  if (!Number.isInteger(value)) return fallback;
  return Math.min(max, Math.max(min, value));
}

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  const hourRaw = env.DIGEST_HOUR?.trim() ?? '8';
  const hour = Number.parseInt(hourRaw, 10);
  if (!Number.isInteger(hour) || hour < 0 || hour > 23) {
    throw new Error('DIGEST_HOUR must be an integer hour between 0 and 23');
  }
  return {
    discordToken: required(env, 'DISCORD_TOKEN'),
    discordAppId: required(env, 'DISCORD_APP_ID'),
    backendBaseUrl: (env.BACKEND_BASE_URL?.trim() || 'http://127.0.0.1:3000').replace(/\/+$/, ''),
    internalApiKey: required(env, 'SCRAPER_INTERNAL_API_KEY'),
    appBaseUrl: (env.APP_BASE_URL?.trim() || 'http://localhost:5173').replace(/\/+$/, ''),
    // Africa/Nairobi has no DST, but the scheduler computes the wall clock through Intl so
    // any IANA zone works.
    digestTz: env.DIGEST_TZ?.trim() || 'Africa/Nairobi',
    digestHour: hour,
    // Clamped, not validated: a typo must not either stop the bot or push it into a ban. At
    // 20/s a 5 000-member run takes ~4 minutes; 25 is the highest we will go, which is still
    // half of Discord's documented ~50/s.
    dmPerSecond: clampInt(env.DISCORD_DM_PER_SECOND, 20, 1, 25),
    dmBurst: clampInt(env.DISCORD_DM_BURST, 5, 1, 10),
    dmPerUserMs: clampInt(env.DISCORD_DM_USER_GAP_MS, 1_200, 600, 5_000),
    freeChannelId: env.DISCORD_FREE_CHANNEL_ID?.trim() || undefined,
  };
}
