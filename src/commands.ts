/**
 * The slash commands, registered globally on boot.
 *
 * Each handler answers from live backend data only. There is no cached job list and no
 * placeholder row: if the backend has nothing, the member is told so in as many words.
 */
import {
  ActionRowBuilder,
  ButtonBuilder,
  ButtonStyle,
  MessageFlags,
  SlashCommandBuilder,
  type ChatInputCommandInteraction,
  type RESTPostAPIChatInputApplicationCommandsJSONBody,
} from 'discord.js';

import type { BotContext } from './context.js';
import {
  formatAssistantReply,
  formatRows,
  MAX_BROWSE_ROWS,
  noMatchesCopy,
  type RowInput,
} from './format.js';
import { BackendError } from './http.js';
import { educationSummary, experienceLabel } from './onboarding.js';
import { assistantErrorReply, buyCreditsRow, clampContent, respond } from './ui.js';
import type { FeedJob, UserEnvelope } from './types.js';

/**
 * The four commands the product keeps. `/balance` replaced `/credits` — same panel, new name.
 * Onboarding is not a command: a member's first free text starts the interview, so the list a
 * member can see is exactly what they search and manage with.
 */
export function buildCommands(): RESTPostAPIChatInputApplicationCommandsJSONBody[] {
  const withQuery = (name: string, description: string): RESTPostAPIChatInputApplicationCommandsJSONBody =>
    new SlashCommandBuilder()
      .setName(name)
      .setDescription(description)
      .addStringOption((option) =>
        option
          .setName('query')
          .setDescription('What to search for, e.g. "data analyst in Mogadishu"')
          .setMaxLength(300)
          .setRequired(false),
      )
      .toJSON();

  return [
    withQuery('jobs', 'Search jobs, or list the newest in your fields'),
    withQuery('tenders', 'Search tenders, or list the newest in your fields'),
    new SlashCommandBuilder().setName('balance').setDescription('How many searches you have left, and how to buy more').toJSON(),
    new SlashCommandBuilder().setName('account').setDescription('Get a private one-time link to your Jobify account').toJSON(),
  ];
}

// ---------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------

function globalNameOf(interaction: ChatInputCommandInteraction): string | null {
  const name = interaction.user.globalName;
  return name && name.length > 0 ? name : null;
}

/** Registers the member on first contact. Safe to call on every command. */
async function ensureUser(interaction: ChatInputCommandInteraction, ctx: BotContext): Promise<void> {
  await ctx.backend.upsertUser({
    discord_id: interaction.user.id,
    username: interaction.user.username,
    global_name: globalNameOf(interaction),
  });
}

function feedRow(job: FeedJob, matchScore?: number | null): RowInput {
  return {
    title: job.title,
    organization: job.organization,
    location: job.location ?? null,
    deadline: job.deadline ?? null,
    kind: job.kind ?? null,
    jobId: job._id,
    url: job.url ?? null,
    matchScore: matchScore ?? null,
  };
}

/**
 * The "plain list from the matcher": the newest live rows for the member, as the BACKEND
 * decided them. Returns the reply text so both the `/jobs` command and a bare "jobs" typed
 * in chat answer from the member's own areas — the same list, one decision.
 *
 * One call (`GET /internal/jobs/for-user/{discord_id}`), not one per category: the backend
 * owns the multi-category filter and the ranking, so `/jobs` and the portal's `/jobs/match`
 * show the same thing in the same order. Fetching the public feed once per category — what
 * this used to do — could not narrow on `kind` without losing rows to the page limit.
 */
export async function browseRows(
  ctx: BotContext,
  discordId: string,
  kind: 'job' | 'tender',
): Promise<string> {
  const { rows, meta } = await ctx.backend.jobsForUser(discordId, {
    limit: MAX_BROWSE_ROWS,
    kind,
    threshold: 0,
  });
  if (meta.categories.length === 0) {
    return 'I do not have a work area for you yet — send me a message and I will ask you four quick questions.';
  }

  if (rows.length === 0) {
    // Below the bar is an ANSWER, not an empty corpus: the backend scored rows and none
    // matched closely enough. The member gets the product sentence — the scoring behind it
    // stays internal — and weak rows are never listed to pad the reply.
    if (meta.reason === 'below_threshold') {
      return noMatchesCopy();
    }
    // `no_candidates` (browsing runs at threshold 0, so this is the only way to be empty)
    // means nothing was admitted at all: the corpus for that kind is empty, not the member's
    // fit. Tenders say so plainly, because a jobs board cannot stand in for them.
    if (kind === 'tender') {
      return (
        'No tenders are live right now.\n' +
        'I check every day and will DM you when one matches your services — or ask me for something specific.'
      );
    }
    return (
      `Nothing live in your fields right now (${meta.categories.join(', ')}).\n` +
      'I check again every day and will DM you when something matches — or ask me for something specific.'
    );
  }

  // ONLY the rows: the owner's line format IS the message. No header, no "I watch for…",
  // no hint at the bottom, and no match percentage — the list is the jobs.
  const content = formatRows(
    rows.map((row) => feedRow(row.job, row.matchScore)),
    { appBaseUrl: ctx.appBaseUrl, max: MAX_BROWSE_ROWS },
  );

  return clampContent(content.trimEnd());
}

async function browse(
  ctx: BotContext,
  interaction: ChatInputCommandInteraction,
  kind: 'job' | 'tender',
): Promise<void> {
  await interaction.editReply({ content: await browseRows(ctx, interaction.user.id, kind) });
}

async function search(
  ctx: BotContext,
  interaction: ChatInputCommandInteraction,
  kind: 'job' | 'tender',
): Promise<void> {
  const query = interaction.options.getString('query')?.trim() ?? '';
  if (query.length === 0) {
    await browse(ctx, interaction, kind);
    return;
  }

  try {
    const answer = await ctx.backend.assistantQuery(interaction.user.id, query);
    const content = formatAssistantReply(answer, { appBaseUrl: ctx.appBaseUrl, maxRows: 3 });
    await interaction.editReply({ content: clampContent(content) });
    ctx.logger.info('search answered', {
      discordId: interaction.user.id,
      intent: answer.intent,
      rows: answer.jobs.length,
      kind,
    });
  } catch (err) {
    if (err instanceof BackendError) {
      ctx.logger.info('search refused by the backend', {
        discordId: interaction.user.id,
        status: err.status,
        code: err.code,
      });
    } else {
      ctx.logger.error('search failed', {
        discordId: interaction.user.id,
        message: err instanceof Error ? err.message : String(err),
      });
    }
    const failure = assistantErrorReply(err);
    await interaction.editReply({
      content: clampContent(failure.content),
      ...(failure.components ? { components: failure.components } : {}),
    });
  }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

export async function handleJobs(interaction: ChatInputCommandInteraction, ctx: BotContext): Promise<void> {
  await interaction.deferReply();
  await ensureUser(interaction, ctx);
  await search(ctx, interaction, 'job');
}

export async function handleTenders(interaction: ChatInputCommandInteraction, ctx: BotContext): Promise<void> {
  await interaction.deferReply();
  await ensureUser(interaction, ctx);
  await search(ctx, interaction, 'tender');
}

/**
 * The alerts panel, built as a pure value so the toggle handler can re-render the very same
 * message after a write without a second copy of the copy. `/alerts` is gone; the panel is
 * still what the `alerts:*` buttons answer with.
 */
export function alertsPanel(
  data: UserEnvelope,
  ctx: Pick<BotContext, 'digestHour' | 'digestTz'>,
): { content: string; components: ActionRowBuilder<ButtonBuilder>[] } {
  const alerts = data.alerts;
  const mode = alerts.mode === 'instant' ? 'instant' : alerts.mode === 'daily' ? 'daily' : 'not chosen';
  const hours = `${String(ctx.digestHour).padStart(2, '0')}:00`;
  const lines = [
    `**Alerts are ${alerts.enabled ? 'on' : 'off'}** · cadence: ${alerts.enabled ? mode : '—'}`,
    `Daily digest at ${hours} (${ctx.digestTz}); instant sends as soon as something new is queued.`,
    `Education: ${educationSummary(data.user.education)} · experience: ${experienceLabel(data.user.experience_years)}`,
    `Last job: ${data.user.last_position ?? 'not set'} · matching: ${data.user.match_profile?.categories.length ? data.user.match_profile.categories.join(', ') : 'keywords only'}`,
  ];
  if (!alerts.enabled) lines.push('', 'Alerts are off, so nothing is queued for you. Switch one back on below.');

  const row = new ActionRowBuilder<ButtonBuilder>().addComponents(
    new ButtonBuilder()
      .setCustomId('alerts:daily')
      .setLabel('Daily')
      .setStyle(alerts.enabled && mode === 'daily' ? ButtonStyle.Primary : ButtonStyle.Secondary),
    new ButtonBuilder()
      .setCustomId('alerts:instant')
      .setLabel('Instant')
      .setStyle(alerts.enabled && mode === 'instant' ? ButtonStyle.Primary : ButtonStyle.Secondary),
    new ButtonBuilder().setCustomId('alerts:off').setLabel('Off').setStyle(ButtonStyle.Danger),
    new ButtonBuilder().setCustomId('alerts:redo').setLabel('Re-run setup').setStyle(ButtonStyle.Secondary),
  );

  return { content: lines.join('\n'), components: [row] };
}

export async function handleBalance(interaction: ChatInputCommandInteraction, ctx: BotContext): Promise<void> {
  await interaction.deferReply({ flags: MessageFlags.Ephemeral });
  await ensureUser(interaction, ctx);
  const data = await ctx.backend.getUser(interaction.user.id);
  const credits = data.credits;
  const renews = credits.period_end ? credits.period_end.slice(0, 10) : 'unknown';
  const content = [
    '**Credits**',
    `Free searches left this period: **${credits.remaining_this_period} of ${credits.monthly_allowance}** (renews ${renews})`,
    `Purchased searches: **${credits.balance}**`,
    '',
    'One smart search costs one query. Browsing `/jobs` or `/tenders` without a query, your alerts and the daily digest are free.',
  ].join('\n');
  await interaction.editReply({ content, components: [buyCreditsRow()] });
}

export async function handleAccount(interaction: ChatInputCommandInteraction, ctx: BotContext): Promise<void> {
  await interaction.deferReply({ flags: MessageFlags.Ephemeral });
  await ensureUser(interaction, ctx);
  const link = await ctx.backend.mintLink(interaction.user.id, 900);
  const message = [
    '**Your Jobify account link**',
    link.url,
    '',
    'Valid for 15 minutes and single use. The page it opens manages payment and your company profile.',
  ].join('\n');
  try {
    await ctx.sendDm(interaction.user.id, message);
    await interaction.editReply({
      content: 'I sent the link to your DMs. It expires in 15 minutes and can only be used once.',
    });
  } catch {
    // The link is a credential, so the fallback is a private (ephemeral) reply, never the
    // channel. A closed DM door is a setting the member can change; say which one.
    ctx.logger.warn('portal link could not be DMd', { discordId: interaction.user.id });
    await interaction.editReply({
      content: [
        'I could not DM you — this server may block direct messages from bots.',
        `Here is the link privately instead (expires in 15 minutes, single use):\n${link.url}`,
        'To get these in a DM next time, allow direct messages for this server.',
      ].join('\n'),
    });
  }
}

/** Type used only by `handleCommand`'s error path; the real objects are interactions. */

export async function handleCommand(interaction: ChatInputCommandInteraction, ctx: BotContext): Promise<void> {
  try {
    switch (interaction.commandName) {
      case 'jobs':
        return await handleJobs(interaction, ctx);
      case 'tenders':
        return await handleTenders(interaction, ctx);
      case 'balance':
        return await handleBalance(interaction, ctx);
      case 'account':
        return await handleAccount(interaction, ctx);
      default:
        // A member can still hold a cached client entry for a command this build no longer
        // registers (`/start`, `/credits`, …). Answer with the four that exist.
        ctx.logger.warn('unknown command', { command: interaction.commandName });
        await respond(interaction, {
          content: 'That command is not one I use any more. Try `/jobs`, `/tenders`, `/balance` or `/account`.',
          flags: MessageFlags.Ephemeral,
        });
    }
  } catch (err) {
    ctx.logger.error('command failed', {
      command: interaction.commandName,
      discordId: interaction.user.id,
      message: err instanceof Error ? err.message : String(err),
    });
    const content =
      err instanceof BackendError && err.status === 0
        ? 'I could not reach Jobify just now. Please try again in a moment.'
        : 'Something went wrong on my side. Please try again in a moment.';
    try {
      await respond(interaction, { content, flags: MessageFlags.Ephemeral });
    } catch {
      // The interaction may already have expired; nothing useful is left to do.
    }
  }
}
