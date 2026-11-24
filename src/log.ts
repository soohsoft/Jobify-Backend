/**
 * Structured logging: one JSON object per line, so systemd/journald and any collector can
 * read it without parsing prose. Secrets are redacted by key name as a second line of
 * defence — the bot never passes the token to the logger in the first place.
 */
export type Level = 'debug' | 'info' | 'warn' | 'error';

const SECRET_KEY = /(token|secret|api[-_]?key|authorization|password|cookie)/i;

function redact(value: unknown, depth = 0): unknown {
  if (depth > 4) return '[deep]';
  if (value === null || typeof value !== 'object') return value;
  if (Array.isArray(value)) return value.map((item) => redact(item, depth + 1));
  const out: Record<string, unknown> = {};
  for (const [key, inner] of Object.entries(value as Record<string, unknown>)) {
    out[key] = SECRET_KEY.test(key) ? '***' : redact(inner, depth + 1);
  }
  return out;
}

export interface Logger {
  debug(msg: string, fields?: Record<string, unknown>): void;
  info(msg: string, fields?: Record<string, unknown>): void;
  warn(msg: string, fields?: Record<string, unknown>): void;
  error(msg: string, fields?: Record<string, unknown>): void;
}

export function createLogger(min: Level = 'info', sink: (line: string) => void = console.log): Logger {
  const order: Record<Level, number> = { debug: 10, info: 20, warn: 30, error: 40 };
  const emit = (level: Level, msg: string, fields?: Record<string, unknown>): void => {
    if (order[level] < order[min]) return;
    const line = JSON.stringify({
      ts: new Date().toISOString(),
      level,
      msg,
      ...(fields ? (redact(fields) as Record<string, unknown>) : {}),
    });
    sink(line);
  };
  return {
    debug: (m, f) => emit('debug', m, f),
    info: (m, f) => emit('info', m, f),
    warn: (m, f) => emit('warn', m, f),
    error: (m, f) => emit('error', m, f),
  };
}

/** Errors are logged by name and message — never by their whole object, which can carry a
 * request body from an HTTP client. */
export function errorFields(err: unknown): Record<string, unknown> {
  if (err instanceof Error) {
    return { error: err.name, message: err.message };
  }
  return { error: 'unknown', message: String(err) };
}
