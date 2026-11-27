/**
 * The Telegram digest: the same queue the Discord digest drains, delivered into Telegram chats.
 *
 * The backend owns everything that matters — who has new matches, which rows are pending, the
 * score. This module only renders them and records the outcome, so a member on Telegram and a
 * member on Discord receive the same jobs on the same day.
 */
import type { Backend } from '../http.js';
import type { Logger } from '../log.js';
import type { Delivery } from '../types.js';
import type { TelegramClient } from './client.js';
import { MAX_DIGEST_ROWS } from '../format.js';
import { escapeHtml, jobBlock } from './format.js';

export interface TgDigestDeps {
  bot: TelegramClient;
  backend: Pick<Backend, 'refreshMatches' | 'deliveries' | 'linkForUser' | 'markSent' | 'markFailed'>;
  logger: Logger;
  /** How many rows one chat receives per day, matching the Discord cap. */
  maxPerDm?: number;
  /** False when this member was messaged recently; the rows stay queued for the next cycle. */
  shouldNotify?: (userId: string) => boolean;
  /** Called after a member's digest was delivered, so the caller can record the time. */
  onDelivered?: (userId: string) => void;
}

export interface TgDigestReport {
  refreshedQueued: number;
  pending: number;
  dms: number;
  sent: number;
  failed: number;
}

function row(delivery: Delivery, index: number): string {
  if (delivery.job) return jobBlock(delivery.job, index, delivery.url ?? undefined);
  // A queued row without the embedded job: render what the queue carried.
  const title = delivery.url
    ? `<a href="${escapeHtml(delivery.url)}">${escapeHtml(delivery.title)}</a>`
    : escapeHtml(delivery.title);
  return `${index}. ${title}`;
}

/**
 * Builds the message body. The `$1` line is a footer on a real job message — the shape that
 * stays inside Telegram's rules and inside the member's consent to receive job alerts.
 */
export function tgDigestText(deliveries: Delivery[], total: number): string {
  const shown = deliveries.slice(0, MAX_DIGEST_ROWS);
  const more = total - shown.length;
  const head =
    total === 1 ? '<b>1 new job for you</b>' : `<b>${total} new jobs for you</b>`;
  const tail = more > 0 ? `\n<i>…and ${more} more in My Jobs.</i>` : '';
  return [
    head,
    '',
    ...shown.map((delivery, index) => row(delivery, index + 1)),
    tail,
    '',
    'Never miss a job. Daily Jobs Notifications just for $1/month.',
    'Subscription now → /balance',
  ]
    .filter((line) => line !== '')
    .join('\n');
}

export async function runTgDigest(deps: TgDigestDeps): Promise<TgDigestReport> {
  const report: TgDigestReport = { refreshedQueued: 0, pending: 0, dms: 0, sent: 0, failed: 0 };
  const limit = deps.maxPerDm ?? MAX_DIGEST_ROWS;

  try {
    const refreshed = await deps.backend.refreshMatches();
    report.refreshedQueued = refreshed.queuedTotal;
  } catch (err) {
    deps.logger.warn('telegram digest: refresh failed', { err: String(err) });
  }

  let deliveries: Delivery[] = [];
  try {
    deliveries = await deps.backend.deliveries(200);
  } catch (err) {
    deps.logger.warn('telegram digest: queue read failed', { err: String(err) });
    return report;
  }
  report.pending = deliveries.length;

  const byUser = new Map<string, Delivery[]>();
  for (const delivery of deliveries) {
    const bucket = byUser.get(delivery.userId);
    if (bucket) bucket.push(delivery);
    else byUser.set(delivery.userId, [delivery]);
  }

  const sent: string[] = [];
  const failures: Array<{ id: string; reason: string }> = [];

  for (const [userId, rows] of byUser) {
    // The queue is keyed by user id; the chat identity lives on the link record, the same way
    // the Discord digest resolves it. A member with no link is left pending, never failed.
    if (deps.shouldNotify && !deps.shouldNotify(userId)) continue;
    let chatId = Number.NaN;
    try {
      const link = await deps.backend.linkForUser(userId);
      chatId = Number(link.discord_id);
    } catch {
      continue;
    }
    // Discord snowflakes are ~1e17; Telegram ids are ~1e9-1e10. A row above the split belongs
    // to the Discord digest, so it is LEFT PENDING here — marking it failed would silently eat
    // a Discord member's notification.
    if (!Number.isFinite(chatId) || chatId > 1e12) continue;
    const batch = rows.slice(0, limit);
    const deferred = rows.slice(limit);
    try {
      await deps.bot.sendMessage(chatId, tgDigestText(batch, rows.length), [
        [
          { text: 'My Jobs', callback_data: 'browse:job' },
          { text: 'My Tenders', callback_data: 'browse:tender' },
        ],
      ]);
      report.dms += 1;
      deps.onDelivered?.(userId);
      for (const item of batch) sent.push(item.notificationId);
      // Over the daily cap: leave them pending rather than dropping them.
      for (const item of deferred) report.pending -= 0;
    } catch (err) {
      const reason = err instanceof Error ? err.message : String(err);
      deps.logger.warn('telegram digest: send failed', { chatId, reason });
      for (const item of rows) failures.push({ id: item.notificationId, reason: reason.slice(0, 120) });
    }
  }

  if (sent.length > 0) {
    try {
      report.sent = await deps.backend.markSent(sent);
    } catch (err) {
      deps.logger.warn('telegram digest: markSent failed', { err: String(err) });
    }
  }
  if (failures.length > 0) {
    try {
      await deps.backend.markFailed(failures);
    } catch (err) {
      deps.logger.warn('telegram digest: markFailed failed', { err: String(err) });
    }
    report.failed = failures.length;
  }

  return report;
}
