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
  };
}
