/**
 * The daily digest: refresh matches, drain the delivery queue, DM each member, report the
 * outcome per notification.
 *
 * The queue is channel-agnostic — it carries a `userId` — so the bot resolves each one to a
 * Discord address through the reverse link lookup. Three decisions worth stating:
 *
 * - at most 5 rows per DM, with the remainder left PENDING (not failed) so tomorrow's run
 *   picks them up instead of losing them;
 * - a member with no Discord identity gets their rows recorded as failed with a
 *   non-terminal reason, so nothing is retried but their web alerts are not paused;
 * - a blocked bot (403 / 50007) IS terminal: the backend pauses that member's alerts, which
 *   is the only way to stop a queue that can never be delivered from looking healthy.
 */
import type { MessagePayload } from './context.js';
import { BackendError } from './http.js';
import type { Backend } from './http.js';
import type { Logger } from './log.js';
import { browseTabsRow } from './onboarding.js';
import { capRows, deliveryRow, dmFailureReason, digestMessage, MAX_DIGEST_ROWS } from './format.js';
import type { Delivery } from './types.js';

export interface DigestDeps {
  backend: Pick<
    Backend,
    'refreshMatches' | 'deliveries' | 'linkForUser' | 'markSent' | 'markFailed'
  >;
  /** A DM with the digest text and the two tabs, so the message it lands on is usable. */
  sendDm: (discordId: string, payload: MessagePayload) => Promise<void>;
  appBaseUrl: string;
  logger: Logger;
  pageSize?: number;
  maxPerDm?: number;
}

export interface DigestReport {
  /** How many notifications the refresh queued. */
  refreshedQueued: number;
  /** How many were pending when the drain started. */
  pending: number;
  dms: number;
  /** Notifications recorded as delivered. */
  sent: number;
  /** Notifications recorded as failed. */
  failed: number;
  /** Rows left pending on purpose because the member already got 5 today. */
  deferred: number;
  /** Rows whose recipient has no Discord account linked. */
  unlinked: number;
}

const EMPTY_REPORT: DigestReport = {
  refreshedQueued: 0,
  pending: 0,
  dms: 0,
  sent: 0,
  failed: 0,
  deferred: 0,
  unlinked: 0,
};

/** Groups the page by recipient, preserving the queue's oldest-first order. */
export function groupByUser(deliveries: Delivery[]): Map<string, Delivery[]> {
  const groups = new Map<string, Delivery[]>();
  for (const delivery of deliveries) {
    const existing = groups.get(delivery.userId);
    if (existing) existing.push(delivery);
    else groups.set(delivery.userId, [delivery]);
  }
  return groups;
}

export async function runDigest(deps: DigestDeps): Promise<DigestReport> {
  const { backend, logger } = deps;
  const report: DigestReport = { ...EMPTY_REPORT };
  const maxPerDm = deps.maxPerDm ?? MAX_DIGEST_ROWS;

  // A refresh failure must not stop the drain: whatever is already queued is still worth
  // sending, and the queue is the source of truth.
  try {
    const refreshed = await backend.refreshMatches();
    report.refreshedQueued = refreshed.queuedTotal ?? 0;
    logger.info('digest: matches refreshed', {
      subscribers: refreshed.subscribers,
      queued: refreshed.queuedTotal,
    });
  } catch (err) {
    logger.error('digest: match refresh failed, draining what is already queued', {
      error: err instanceof Error ? err.name : 'unknown',
      message: err instanceof Error ? err.message : String(err),
    });
  }

  let deliveries: Delivery[];
  try {
    deliveries = await backend.deliveries(deps.pageSize ?? 200);
  } catch (err) {
    logger.error('digest: could not read the delivery queue', {
      message: err instanceof Error ? err.message : String(err),
    });
    return report;
  }
  report.pending = deliveries.length;
  if (deliveries.length === 0) {
    logger.info('digest: nothing to send');
    return report;
  }

  for (const [userId, group] of groupByUser(deliveries)) {
    const { delivered, remaining } = capRows(group, maxPerDm);
    if (delivered.length === 0) continue;
    const ids = delivered.map((delivery) => delivery.notificationId);

    let discordId: string;
    try {
      const link = await backend.linkForUser(userId);
      discordId = link.discord_id;
    } catch (err) {
      if (err instanceof BackendError && err.status === 404) {
        // A web-only account: no Discord identity to send to. Recorded (so it is not retried
        // forever) with a NON-terminal reason, which leaves their alerts switched on.
        await safeFail(backend, logger, ids.map((id) => ({ id, reason: 'no_discord_link' })), userId);
        report.failed += ids.length;
        report.unlinked += ids.length;
        continue;
      }
      // A lookup hiccup leaves the rows pending: the next run can still deliver them.
      logger.warn('digest: could not resolve a Discord identity, leaving rows queued', { userId });
      report.deferred += remaining + delivered.length;
      continue;
    }

    const content = digestMessage(
      delivered.map(deliveryRow),
      { appBaseUrl: deps.appBaseUrl, max: maxPerDm, extraWaiting: remaining },
    );

    try {
      // The tabs ride on the notification itself: tapping a push opens this message, and a
      // message with no buttons is a dead end for a member who wants to look further.
      await deps.sendDm(discordId, { content, components: [browseTabsRow()] });
    } catch (err) {
      const reason = dmFailureReason(err);
      logger.warn('digest: DM failed', {
        discordId,
        reason,
        rows: ids.length,
      });
      await safeFail(backend, logger, ids.map((id) => ({ id, reason })), userId);
      report.failed += ids.length;
      report.deferred += remaining;
      continue;
    }

    try {
      await backend.markSent(ids);
      report.sent += ids.length;
      report.dms += 1;
      report.deferred += remaining;
      logger.info('digest: delivered', { discordId, rows: ids.length, waiting: remaining });
    } catch (err) {
      // The DM went out but the receipt failed: the rows stay pending and may be sent again
      // tomorrow. Logged loudly because a duplicate DM is better than a lost one, but it is
      // still a defect worth seeing.
      logger.error('digest: DM sent but the delivery receipt failed', {
        discordId,
        rows: ids.length,
        message: err instanceof Error ? err.message : String(err),
      });
      report.sent += ids.length;
      report.dms += 1;
    }
  }

  logger.info('digest: finished', { ...report });
  return report;
}

async function safeFail(
  backend: DigestDeps['backend'],
  logger: Logger,
  failures: Array<{ id: string; reason: string }>,
  userId: string,
): Promise<void> {
  try {
    await backend.markFailed(failures);
  } catch (err) {
    logger.error('digest: could not record a delivery failure', {
      userId,
      rows: failures.length,
      message: err instanceof Error ? err.message : String(err),
    });
  }
}
