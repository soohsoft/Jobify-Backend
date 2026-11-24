/**
 * Component, select and modal handling: the interview's step machine plus the small buttons
 * attached to `/alerts`, `/credits` and the summary.
 *
 * Every branch starts by checking the session, because an old message can outlive a restart
 * or a finished interview. A dead session gets a plain "run /start again" rather than a
 * half-applied profile write.
 */
import { MessageFlags, type ButtonInteraction, type Interaction, type ModalSubmitInteraction, type StringSelectMenuInteraction, type ActionRowBuilder, type ButtonBuilder } from 'discord.js';

import { alertsPanel, handleCommand } from './commands.js';
import type { BotContext } from './context.js';
import {
  CID,
  EDUCATION_OPTIONS,
  MAX_SECTORS,
  educationPrompt,
  experiencePrompt,
  locationModal,
  locationPrompt,
  sectorPage,
  sectorsPrompt,
  skillsModal,
  skillsPrompt,
  summaryPrompt,
  wantsPrompt,
} from './onboarding.js';
import { assistantErrorReply, clampContent } from './ui.js';
import { BackendError } from './http.js';

export async function handleInteraction(interaction: Interaction, ctx: BotContext): Promise<void> {
  if (interaction.isChatInputCommand()) {
    await handleCommand(interaction, ctx);
    return;
  }
  if (interaction.isButton()) {
    await handleButton(interaction, ctx);
    return;
  }
  if (interaction.isStringSelectMenu()) {
    await handleSelect(interaction, ctx);
    return;
  }
  if (interaction.isModalSubmit()) {
    await handleModal(interaction, ctx);
  }
}

// ---------------------------------------------------------------------------
// Shared failure answers
// ---------------------------------------------------------------------------

async function expired(interaction: ButtonInteraction | StringSelectMenuInteraction | ModalSubmitInteraction): Promise<void> {
  const payload = {
    content: 'That setup step has expired. Run `/start` to pick it up again — anything you already answered is saved.',
    flags: MessageFlags.Ephemeral,
  } as const;
  try {
    if (interaction.deferred || interaction.replied) await interaction.followUp(payload);
    else await interaction.reply(payload);
  } catch {
    // The interaction can be too old to answer at all; nothing else to do.
  }
}

async function stepFailed(
  interaction: ButtonInteraction | StringSelectMenuInteraction | ModalSubmitInteraction,
  ctx: BotContext,
  err: unknown,
): Promise<void> {
  const content =
    err instanceof BackendError && err.status === 0
      ? 'I could not reach Jobify just now — your answer was not saved. Please try again.'
      : 'I could not save that answer. Please try again — if it keeps failing, run `/start` again.';
  ctx.logger.error('onboarding step failed', {
    discordId: interaction.user.id,
    step: interaction.customId,
    message: err instanceof Error ? err.message : String(err),
  });
  try {
    await interaction.followUp({ content, flags: MessageFlags.Ephemeral });
  } catch {
    /* ignored: the interaction is gone */
  }
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

async function handleButton(interaction: ButtonInteraction, ctx: BotContext): Promise<void> {
  const id = interaction.customId;
  const discordId = interaction.user.id;

  if (id === 'credits:buy') {
    await buyCredits(interaction, ctx);
    return;
  }
  if (id.startsWith('alerts:')) {
    await alertsButton(interaction, ctx, id.slice('alerts:'.length));
    return;
  }
  if (id === CID.restart) {
    await restartInterview(interaction, ctx);
    return;
  }
  if (!id.startsWith('onb:')) {
    ctx.logger.warn('unhandled button', { customId: id });
    return;
  }

  const session = ctx.sessions.get(discordId);
  if (!session) {
    await expired(interaction);
    return;
  }

  if (id.startsWith('onb:edu:')) {
    const value = id.slice('onb:edu:'.length);
    if (!EDUCATION_OPTIONS.some((option) => option.value === value)) {
      await expired(interaction);
      return;
    }
    await interaction.deferUpdate();
    try {
      await ctx.backend.saveProfile({ discord_id: discordId, education_level: value });
    } catch (err) {
      await stepFailed(interaction, ctx, err);
      return;
    }
    session.education = value;
    session.step = 'experience';
    await interaction.editReply(experiencePrompt());
    return;
  }

  if (id.startsWith('onb:exp:')) {
    const band = id.slice('onb:exp:'.length);
    const option = experienceYarsFromBand(band);
    if (option === null) {
      await expired(interaction);
      return;
    }
    await interaction.deferUpdate();
    try {
      await ctx.backend.saveProfile({ discord_id: discordId, experience_years: option });
    } catch (err) {
      await stepFailed(interaction, ctx, err);
      return;
    }
    session.experienceYears = option;
    session.step = 'sectors';
    await interaction.editReply(sectorsPrompt(session, await ctx.categories()));
    return;
  }

  if (id.startsWith('onb:sectors:page:')) {
    const page = Number.parseInt(id.slice('onb:sectors:page:'.length), 10);
    if (!Number.isInteger(page)) {
      await expired(interaction);
      return;
    }
    await interaction.deferUpdate();
    session.sectorPage = page;
    await interaction.editReply(sectorsPrompt(session, await ctx.categories()));
    return;
  }

  if (id === CID.sectorsDone) {
    await interaction.deferUpdate();
    const categories = await ctx.categories();
    if (session.sectors.length === 0) {
      const view = sectorsPrompt(session, categories);
      await interaction.editReply({
        ...view,
        content: `Pick at least one sector — I need something to match against.\n\n${view.content}`,
      });
      return;
    }
    try {
      await ctx.backend.saveProfile({ discord_id: discordId, sectors: session.sectors });
    } catch (err) {
      await stepFailed(interaction, ctx, err);
      return;
    }
    session.step = 'location';
    await interaction.editReply(locationPrompt());
    return;
  }

  if (id === CID.locationOpen) {
    // A modal has to be the interaction's first response: no defer here.
    await interaction.showModal(locationModal());
    return;
  }

  if (id === CID.skillsOpen) {
    await interaction.showModal(skillsModal());
    return;
  }

  if (id.startsWith('onb:want:')) {
    const raw = id.slice('onb:want:'.length);
    if (raw !== 'jobs' && raw !== 'tenders' && raw !== 'both') {
      await expired(interaction);
      return;
    }
    await interaction.deferUpdate();
    try {
      await ctx.backend.saveProfile({ discord_id: discordId, wants: raw });
      // A finished interview turns the daily digest on. `/stop` is the way out, and it says
      // so — switching alerts on silently would be the wrong default.
      await ctx.backend.setAlerts(discordId, true, 'daily');
      const envelope = await ctx.backend.getUser(discordId);
      const matchCount = await countLiveMatches(ctx, envelope.user.match_profile?.categories ?? [], raw);
      ctx.sessions.clear(discordId);
      await interaction.editReply(
        summaryPrompt({
          user: envelope.user,
          matchCount,
          digestTz: ctx.digestTz,
          digestHour: ctx.digestHour,
          appBaseUrl: ctx.appBaseUrl,
        }),
      );
      ctx.logger.info('onboarding complete', { discordId, matchCount, wants: raw });
    } catch (err) {
      await stepFailed(interaction, ctx, err);
    }
  }
}

function experienceYarsFromBand(band: string): number | null {
  switch (band) {
    case '0':
      return 0;
    case '1-2':
      return 1;
    case '3-5':
      return 3;
    case '6-10':
      return 6;
    case '10+':
      return 11;
    default:
      return null;
  }
}

/**
 * How many live openings match: the sectors the member picked, narrowed to the kind they
 * asked for. Read from the public feed rather than the assistant, because a summary must not
 * cost a query.
 */
async function countLiveMatches(ctx: BotContext, categories: string[], wants: string): Promise<number> {
  if (categories.length === 0) return 0;
  const wanted = new Set(categories);
  const page = await ctx.backend.listJobs({ limit: 200 });
  return page.items.filter((job) => {
    if (!job.category || !wanted.has(job.category)) return false;
    if (wants === 'jobs') return (job.kind ?? 'job') !== 'tender';
    if (wants === 'tenders') return job.kind === 'tender';
    return true;
  }).length;
}

async function restartInterview(interaction: ButtonInteraction, ctx: BotContext): Promise<void> {
  await interaction.deferUpdate();
  const existing = await ctx.backend.getUser(interaction.user.id).catch(() => null);
  ctx.sessions.start(interaction.user.id, { sectors: existing?.user.sectors ?? [] });
  await interaction.editReply(educationPrompt());
  ctx.logger.info('onboarding restarted', { discordId: interaction.user.id });
}

async function alertsButton(interaction: ButtonInteraction, ctx: BotContext, action: string): Promise<void> {
  if (action === 'redo') {
    await restartInterview(interaction, ctx);
    return;
  }

  await interaction.deferUpdate();
  try {
    if (action === 'daily') await ctx.backend.setAlerts(interaction.user.id, true, 'daily');
    else if (action === 'instant') await ctx.backend.setAlerts(interaction.user.id, true, 'instant');
    else if (action === 'off') await ctx.backend.setAlerts(interaction.user.id, false);
    else {
      await expired(interaction);
      return;
    }
    const envelope = await ctx.backend.getUser(interaction.user.id);
    await interaction.editReply(alertsPanel(envelope, ctx));
    ctx.logger.info('alerts toggled', { discordId: interaction.user.id, action });
  } catch (err) {
    await stepFailed(interaction, ctx, err);
  }
}

async function buyCredits(interaction: ButtonInteraction, ctx: BotContext): Promise<void> {
  await interaction.deferReply({ flags: MessageFlags.Ephemeral });
  try {
    await ctx.backend.upsertUser({
      discord_id: interaction.user.id,
      username: interaction.user.username,
      global_name: interaction.user.globalName ?? null,
    });
    const link = await ctx.backend.mintLink(interaction.user.id, 900);
    await interaction.editReply({
      content: [
        '**Buying credits happens on your Jobify account page.**',
        link.url,
        '',
        'Single use, expires in 15 minutes. Payment is handled there — there is no card entry in Discord.',
      ].join('\n'),
    });
  } catch (err) {
    const failure = assistantErrorReply(err);
    await interaction.editReply({ content: failure.content });
  }
}

// ---------------------------------------------------------------------------
// Select menu: the sector multi-select, paged
// ---------------------------------------------------------------------------

async function handleSelect(interaction: StringSelectMenuInteraction, ctx: BotContext): Promise<void> {
  if (interaction.customId !== CID.sectorSelect) return;

  const session = ctx.sessions.get(interaction.user.id);
  if (!session) {
    await expired(interaction);
    return;
  }

  const categories = await ctx.categories();
  const view = sectorPage(categories, session.sectorPage);
  const onThisPage = new Set(view.items.map((category) => category.slug));
  // Replace this page's contribution and keep the other page's choices: selecting on page 2
  // must not erase what was picked on page 1.
  const kept = session.sectors.filter((slug) => !onThisPage.has(slug));
  const chosen = [...new Set([...kept, ...interaction.values])].slice(0, MAX_SECTORS);
  session.sectors = chosen;

  const prompt = sectorsPrompt(session, categories);
  const overflow = kept.length + interaction.values.length > MAX_SECTORS;
  await interaction.update({
    ...prompt,
    content: overflow
      ? `Only the first ${MAX_SECTORS} sectors are kept.\n\n${prompt.content}`
      : prompt.content,
  });
}

// ---------------------------------------------------------------------------
// Modals: location, skills and the company profile
// ---------------------------------------------------------------------------

async function handleModal(interaction: ModalSubmitInteraction, ctx: BotContext): Promise<void> {
  if (interaction.customId === 'company:submit') {
    await companySubmit(interaction, ctx);
    return;
  }

  const session = ctx.sessions.get(interaction.user.id);
  if (!session) {
    await expired(interaction);
    return;
  }

  if (interaction.customId === CID.locationModal) {
    const location = interaction.fields.getTextInputValue('location').trim();
    if (location.length === 0) {
      await interaction.reply({ content: 'A city or region is needed — try `/start` again.', flags: MessageFlags.Ephemeral });
      return;
    }
    await advanceFromModal(interaction, ctx, async () => {
      await ctx.backend.saveProfile({ discord_id: interaction.user.id, location });
      session.step = 'skills';
      return skillsPrompt();
    });
    return;
  }

  if (interaction.customId === CID.skillsModal) {
    const raw = interaction.fields.getTextInputValue('skills');
    const skills = raw
      .split(',')
      .map((skill) => skill.trim())
      .filter((skill) => skill.length > 0)
      .slice(0, 20);
    await advanceFromModal(interaction, ctx, async () => {
      if (skills.length > 0) {
        await ctx.backend.saveProfile({ discord_id: interaction.user.id, skills });
      }
      session.step = 'wants';
      return wantsPrompt();
    });
    return;
  }
}

/**
 * A modal submit continues the SAME ephemeral message when it came from one, which keeps the
 * interview in a single window; otherwise it starts a new private message.
 */
async function advanceFromModal(
  interaction: ModalSubmitInteraction,
  ctx: BotContext,
  work: () => Promise<{ content: string; components: ActionRowBuilder<ButtonBuilder>[] }>,
): Promise<void> {
  const fromMessage = interaction.isFromMessage();
  try {
    if (fromMessage) await interaction.deferUpdate();
    else await interaction.deferReply({ flags: MessageFlags.Ephemeral });
    const next = await work();
    await interaction.editReply({
      content: clampContent(next.content),
      components: next.components,
    });
  } catch (err) {
    await stepFailed(interaction, ctx, err);
  }
}

async function companySubmit(interaction: ModalSubmitInteraction, ctx: BotContext): Promise<void> {
  const name = interaction.fields.getTextInputValue('name').trim();
  const services = interaction.fields
    .getTextInputValue('services')
    .split(',')
    .map((service) => service.trim())
    .filter((service) => service.length > 0);
  await interaction.deferReply({ flags: MessageFlags.Ephemeral });
  try {
    const result = await ctx.backend.setCompany({
      discord_id: interaction.user.id,
      name,
      services,
    });
    const stored = result.user.company;
    ctx.logger.info('company profile saved from Discord', {
      discordId: interaction.user.id,
      services: stored?.services.length ?? 0,
    });
    await interaction.editReply({
      content: stored
        ? `Saved: **${stored.name}**${stored.services.length ? ` — ${stored.services.join(', ')}` : ''}.\nThe portal shows the full company profile.`
        : 'Your company profile was cleared.',
    });
  } catch (err) {
    const failure =
      err instanceof BackendError && err.status === 400
        ? { content: `That could not be saved: ${err.message}` }
        : assistantErrorReply(err);
    await interaction.editReply({ content: failure.content });
  }
}
