/**
 * Component and modal handling: the four-step interview plus the small buttons attached to
 * the alerts panel, `/balance` and the completion message.
 *
 * Every branch starts by checking the session, because an old message can outlive a restart
 * or a finished interview. A dead session gets a plain "send me a message" and nothing is
 * written, rather than a half-applied profile write.
 */
import { MessageFlags, type ButtonInteraction, type Interaction, type ModalSubmitInteraction, type StringSelectMenuInteraction, type ActionRowBuilder, type ButtonBuilder } from 'discord.js';

import { alertsPanel, browseRows, handleCommand } from './commands.js';
import type { BotContext } from './context.js';
import {
  CID,
  DEGREE_LEVELS,
  EDUCATION_OPTIONS,
  FIELD_OPTIONS,
  MAX_EDUCATION,
  anotherPrompt,
  completionPrompt,
  educationPrompt,
  experiencePrompt,
  fieldModal,
  fieldPrompt,
  nameModal,
  namePrompt,
  positionModal,
  positionPrompt,
  type OnboardingSession,
} from './onboarding.js';
import type { EducationEntry } from './types.js';
import { assistantErrorReply, companyModal } from './ui.js';
import { BackendError } from './http.js';
import { MAX_DIGEST_ROWS } from './format.js';

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
    await handleSelect(interaction);
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
    content: 'That setup step has expired. Send me a message and we will pick it up again — anything you already answered is saved.',
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
      : err instanceof BackendError && err.status === 400
        ? `That answer was refused: ${err.message}`
        : 'I could not save that answer. Please try again — if it keeps failing, send me a message to pick the setup up again.';
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

  if (id === 'company:open') {
    // The company modal has to be the first response to the click: no defer, no edit.
    await interaction.showModal(companyModal());
    return;
  }
  if (id === 'credits:buy') {
    await buyCredits(interaction, ctx);
    return;
  }
  if (id.startsWith('alerts:')) {
    await alertsButton(interaction, ctx, id.slice('alerts:'.length));
    return;
  }
  // The Jobs / Tenders tabs on the completion message. Deliberately before the session check:
  // they are a read, they need no interview in progress, and the setup message can be pressed
  // months after it was sent.
  if (id === CID.browseJobs || id === CID.browseTenders) {
    await browseTab(interaction, ctx, id === CID.browseTenders ? 'tender' : 'job');
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

  // Step 1 — the name. A modal has to be the interaction's first response: no defer here.
  if (id === CID.nameOpen) {
    await interaction.showModal(nameModal(session.name));
    return;
  }

  // Step 2a — the level.
  if (id.startsWith('onb:edu:')) {
    const level = id.slice('onb:edu:'.length);
    if (!EDUCATION_OPTIONS.some((option) => option.value === level)) {
      await expired(interaction);
      return;
    }
    if (DEGREE_LEVELS.includes(level)) {
      // Needs a field first; nothing is saved until that answer exists.
      session.pendingLevel = level;
      session.step = 'education_field';
      await interaction.update(fieldPrompt(level));
      return;
    }
    await interaction.deferUpdate();
    await addDegree(interaction, ctx, session, { level, field: '' });
    return;
  }

  // Step 2b — the field, only for bachelor / master / PhD.
  if (id.startsWith('onb:field:')) {
    const key = id.slice('onb:field:'.length);
    const option = FIELD_OPTIONS.find((candidate) => candidate.key === key);
    const level = session.pendingLevel;
    if (!option || !level) {
      await expired(interaction);
      return;
    }
    if (option.key === 'other') {
      // "Other" is free text, and the modal keeps `pendingLevel` for the submit.
      await interaction.showModal(fieldModal());
      return;
    }
    await interaction.deferUpdate();
    await addDegree(interaction, ctx, session, { level, field: option.label });
    return;
  }

  // Step 2c — "another degree?"
  if (id.startsWith('onb:another:')) {
    const answer = id.slice('onb:another:'.length);
    if (answer !== 'yes' && answer !== 'no') {
      await expired(interaction);
      return;
    }
    await interaction.deferUpdate();
    if (answer === 'yes' && session.education.length < MAX_EDUCATION) {
      session.step = 'education_level';
      await interaction.editReply(educationPrompt(session));
      return;
    }
    // "no", or "yes" after the fourth degree: on to the job history.
    session.step = 'position';
    await interaction.editReply(positionPrompt());
    return;
  }

  // Step 3a — the last job title.
  if (id === CID.positionOpen) {
    await interaction.showModal(positionModal(session.lastPosition));
    return;
  }

  // Step 3b — the years band, and then step 4: done.
  if (id.startsWith('onb:exp:')) {
    const band = id.slice('onb:exp:'.length);
    const years = experienceYearsFromBand(band);
    if (years === null) {
      await expired(interaction);
      return;
    }
    await interaction.deferUpdate();
    session.experienceYears = years;
    await completeInterview(interaction, ctx, session, years);
  }
}

/** Appends one education entry, writes the WHOLE list, and asks for the next one. */
async function addDegree(
  interaction: ButtonInteraction | ModalSubmitInteraction,
  ctx: BotContext,
  session: OnboardingSession,
  entry: EducationEntry,
): Promise<void> {
  const education = [...session.education, entry].slice(0, MAX_EDUCATION);
  try {
    await ctx.backend.saveProfile({ discord_id: interaction.user.id, education });
  } catch (err) {
    await stepFailed(interaction, ctx, err);
    return;
  }
  session.education = education;
  session.pendingLevel = undefined;
  session.step = 'another_degree';

  const prompt = anotherPrompt(session);
  if (interaction.deferred || interaction.replied) await interaction.editReply(prompt);
  else await interaction.reply({ ...prompt, flags: MessageFlags.Ephemeral });
}

function experienceYearsFromBand(band: string): number | null {
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
 * Step 4: the last answer, then "Setup complete" with the member's CURRENT matches.
 *
 * The bot does not enable alerts here — the backend does it when onboarding completes, and
 * skips it for anyone who switched alerts off. Re-enabling from the bot would quietly undo that.
 * The matches are the backend's own scored rows (max 5), rendered by the digest formatter.
 */
async function completeInterview(
  interaction: ButtonInteraction,
  ctx: BotContext,
  session: OnboardingSession,
  years: number,
): Promise<void> {
  const discordId = interaction.user.id;
  try {
    await ctx.backend.saveProfile({ discord_id: discordId, experience_years: years });
    const envelope = await ctx.backend.getUser(discordId);

    let matches: Awaited<ReturnType<BotContext['backend']['jobsForUser']>>['rows'] = [];
    let matchReason: string | null = null;
    try {
      // The backend's own for-user read: it decided the categories, so the first digest is
      // exactly the list `/jobs` will show.
      const list = await ctx.backend.jobsForUser(discordId, { limit: MAX_DIGEST_ROWS });
      matches = list.rows;
      matchReason = list.meta.reason;
    } catch (err) {
      // Setup itself succeeded; a failed match read must not turn a finished interview into
      // an error message. `/jobs` still works, and the digest will fill in later.
      ctx.logger.warn('first match digest could not be read', {
        discordId,
        message: err instanceof Error ? err.message : String(err),
      });
    }

    ctx.sessions.clear(discordId);
    await interaction.editReply(
      completionPrompt({
        user: envelope.user,
        matches,
        matchReason,
        digestTz: ctx.digestTz,
        digestHour: ctx.digestHour,
        appBaseUrl: ctx.appBaseUrl,
      }),
    );
    ctx.logger.info('onboarding complete', {
      discordId,
      education: envelope.user.education.length,
      matches: matches.length,
      alertsEnabled: envelope.alerts_enabled,
      experienceYears: years,
      // Kept for the log only: the session is gone after this.
      sessionName: session.name ?? null,
    });
  } catch (err) {
    await stepFailed(interaction, ctx, err);
  }
}

/**
 * The Jobs / Tenders tabs.
 *
 * Ephemeral on purpose: pressing a tab shows the list to the member who pressed it, and the
 * setup message with its two tabs stays where it is, so the same message can be used again.
 * `browseRows` is the same function `/jobs` and `/tenders` call, so the tabs cannot drift from
 * the commands — including the tender rule that asks for a company profile first.
 */
async function browseTab(
  interaction: ButtonInteraction,
  ctx: BotContext,
  kind: 'job' | 'tender',
): Promise<void> {
  await interaction.deferReply({ flags: MessageFlags.Ephemeral });
  const reply = await browseRows(ctx, interaction.user.id, kind);
  await interaction.editReply({ content: reply.content, components: reply.components ?? [] });
}

async function restartInterview(interaction: ButtonInteraction, ctx: BotContext): Promise<void> {
  await interaction.deferUpdate();
  const existing = await ctx.backend.getUser(interaction.user.id).catch(() => null);
  ctx.sessions.start(interaction.user.id, {
    previousEducation: existing?.user.education ?? [],
    name: existing?.user.name ?? undefined,
    lastPosition: existing?.user.last_position ?? undefined,
    experienceYears: existing?.user.experience_years ?? undefined,
  });
  await interaction.editReply(namePromptForRestart());
  ctx.logger.info('onboarding restarted', { discordId: interaction.user.id });
}

/** The name prompt, with a line saying a re-run replaces what is on file. */
function namePromptForRestart(): { content: string; components: ActionRowBuilder<ButtonBuilder>[] } {
  const prompt = namePrompt();
  return {
    content: `${prompt.content}\nYour current answers are filled in where they can be — answering again replaces them.`,
    components: prompt.components,
  };
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
// Select menus
//
// The interview has no select menus any more (sectors went with the old flow), but the
// interaction router still needs to answer one politely if an old message is clicked.
// ---------------------------------------------------------------------------

async function handleSelect(interaction: StringSelectMenuInteraction): Promise<void> {
  await expired(interaction);
}

// ---------------------------------------------------------------------------
// Modals: name, field of study, last position, and the company profile
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

  // Step 1 — the name.
  if (interaction.customId === CID.nameModal) {
    const name = interaction.fields.getTextInputValue('name').trim();
    if (name.length === 0) {
      await interaction.reply({ content: 'I need a name — send me a message and we will start again.', flags: MessageFlags.Ephemeral });
      return;
    }
    await advanceFromModal(interaction, ctx, async () => {
      await ctx.backend.saveProfile({ discord_id: interaction.user.id, name });
      session.name = name;
      session.step = 'education_level';
      return educationPrompt(session);
    });
    return;
  }

  // Step 2b — "Other" as free text. It becomes keywords, never a category.
  if (interaction.customId === CID.fieldModal) {
    const text = interaction.fields.getTextInputValue('field').trim();
    const level = session.pendingLevel;
    if (!level) {
      await expired(interaction);
      return;
    }
    if (text.length === 0) {
      await interaction.reply({ content: 'Tell me the field, or pick one of the buttons.', flags: MessageFlags.Ephemeral });
      return;
    }
    if (interaction.isFromMessage()) await interaction.deferUpdate();
    else await interaction.deferReply({ flags: MessageFlags.Ephemeral });
    await addDegree(interaction, ctx, session, { level, field: `Other: ${text}` });
    return;
  }

  // Step 3a — the last job title.
  if (interaction.customId === CID.positionModal) {
    const position = interaction.fields.getTextInputValue('position').trim();
    if (position.length === 0) {
      await interaction.reply({ content: 'A job title is needed — send me a message and we will start again.', flags: MessageFlags.Ephemeral });
      return;
    }
    await advanceFromModal(interaction, ctx, async () => {
      await ctx.backend.saveProfile({ discord_id: interaction.user.id, last_position: position });
      session.lastPosition = position;
      session.step = 'years';
      return experiencePrompt();
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
      content: next.content,
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
