/**
 * One posting, rendered the way the product shows it in Telegram.
 *
 * The order is fixed: title (the link), employer and place, closing date, then the source it
 * came from. Members asked for the closing date and the source on their own lines instead of a
 * dot-separated tail, and both are what they scan before deciding to open anything.
 */
import type { FeedJob } from '../types.js';

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

export function escapeHtml(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

/** `2026-12-31` (or an ISO timestamp) → `31 Dec 2026`; anything unparseable is shown as-is. */
export function closingDate(deadline?: string | null): string {
  if (!deadline) return '';
  const date = deadline.slice(0, 10);
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  if (!match) return deadline;
  const [, year, month, day] = match;
  const name = MONTHS[Number(month) - 1];
  if (!name) return deadline;
  return `${Number(day)} ${name} ${year}`;
}

/** The four-line block for one posting. `index` is 1-based; 0 renders no number. */
export function jobBlock(
  job: Pick<FeedJob, 'title' | 'organization' | 'location' | 'deadline' | 'url' | 'source_name'>,
  index: number,
  fallbackUrl?: string,
): string {
  const url = job.url ?? fallbackUrl ?? '';
  const title = escapeHtml(job.title);
  const heading = url ? `<a href="${escapeHtml(url)}">${title}</a>` : title;
  const where = job.location ? `${escapeHtml(job.organization)} — ${escapeHtml(job.location)}` : escapeHtml(job.organization);
  const lines: string[] = [`${index > 0 ? `${index}. ` : ''}${heading}`];
  if (where) lines.push(`   ${where}`);
  const closes = closingDate(job.deadline);
  if (closes) lines.push(`   Closing Date: ${closes}`);
  return lines.join('\n');
}
