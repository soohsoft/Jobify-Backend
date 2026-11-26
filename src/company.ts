/**
 * The company profile, asked in three steps: name → services → city.
 *
 * A Discord modal can only hold text inputs, not a dropdown, so the city has to be a separate
 * select-menu step. The draft therefore lives between interactions, keyed by Discord id, exactly
 * like the interview session — and is cleared once the profile is written.
 */
import {
  ActionRowBuilder,
  ButtonBuilder,
  ButtonStyle,
  ModalBuilder,
  StringSelectMenuBuilder,
  StringSelectMenuOptionBuilder,
  TextInputBuilder,
  TextInputStyle,
} from 'discord.js';

/**
 * The Somali cities an employer can pick from. Discord caps a select menu at 25 options, so this
 * is the 25 that matter for hiring; the portal accepts a free-text location for anything else.
 */
export const COMPANY_CITIES: readonly string[] = [
  'Mogadishu',
  'Hargeisa',
  'Bosaso',
  'Kismayo',
  'Baidoa',
  'Galkayo',
  'Garowe',
  'Beledweyne',
  'Burao',
  'Marka',
  'Jowhar',
  'Berbera',
  'Borama',
  'Las Anod',
  'Qardho',
  'Afgooye',
  'Balad',
  'Baraawe',
  'Dhusamareb',
  'Eyl',
  'Jilib',
  'Wajid',
  'El Buur',
  'Abudwak',
  'Hudur',
];

export interface CompanyDraft {
  name?: string;
  services?: string[];
}

/**
 * The one draft store for the process. Module-level like the session store: the lifetime that
 * matters is the process, and a company form spans several interactions.
 */
/** What a half-finished company profile is: in memory, per member, gone when the process restarts. */
export class CompanyDrafts {
  private readonly drafts = new Map<string, CompanyDraft>();

  start(discordId: string): CompanyDraft {
    const draft: CompanyDraft = {};
    this.drafts.set(discordId, draft);
    return draft;
  }

  patch(discordId: string, patch: CompanyDraft): CompanyDraft {
    const draft = this.drafts.get(discordId) ?? this.start(discordId);
    Object.assign(draft, patch);
    return draft;
  }

  get(discordId: string): CompanyDraft | undefined {
    return this.drafts.get(discordId);
  }

  clear(discordId: string): void {
    this.drafts.delete(discordId);
  }
}

/** The one draft store for the process — a company form spans several interactions. */
export const companyDrafts = new CompanyDrafts();

/** Opens step 2 from step 1's reply: a modal submit cannot answer with another modal. */
export function companyNextRow(): ActionRowBuilder<ButtonBuilder> {
  return new ActionRowBuilder<ButtonBuilder>().addComponents(
    new ButtonBuilder()
      .setCustomId('company:services')
      .setLabel('Enter company services')
      .setStyle(ButtonStyle.Primary),
  );
}

/** Step 1 — the name, prefilled with what is already stored when there is a company on file. */
export function companyNameModal(currentName?: string | null): ModalBuilder {
  const input = new TextInputBuilder()
    .setCustomId('name')
    .setLabel('Company name')
    .setPlaceholder('SOOHSOFT LTD')
    .setStyle(TextInputStyle.Short)
    .setMaxLength(120)
    .setRequired(true);
  if (currentName && currentName.trim().length > 0) input.setValue(currentName.trim().slice(0, 120));
  return new ModalBuilder()
    .setCustomId('company:submit:name')
    .setTitle('Company profile — 1 of 3')
    .addComponents(new ActionRowBuilder<TextInputBuilder>().addComponents(input));
}

/** Step 2 — what the company does. These services are what the tender matcher scores. */
export function companyServicesModal(currentServices?: readonly string[]): ModalBuilder {
  const input = new TextInputBuilder()
    .setCustomId('services')
    .setLabel('Company services (comma separated)')
    .setPlaceholder('Construction, Logistics')
    .setStyle(TextInputStyle.Short)
    .setMaxLength(400)
    .setRequired(true);
  if (currentServices && currentServices.length > 0) {
    input.setValue(currentServices.join(', ').slice(0, 400));
  }
  return new ModalBuilder()
    .setCustomId('company:submit:services')
    .setTitle('Company profile — 2 of 3')
    .addComponents(new ActionRowBuilder<TextInputBuilder>().addComponents(input));
}

/** Step 3 — the city. One option per Somali city, so the value is always spelled one way. */
export function companyCityRow(): ActionRowBuilder<StringSelectMenuBuilder> {
  const menu = new StringSelectMenuBuilder()
    .setCustomId('company:city')
    .setPlaceholder('Where is the company based?')
    .addOptions(
      COMPANY_CITIES.map((city) =>
        new StringSelectMenuOptionBuilder().setLabel(city).setValue(city),
      ),
    );
  return new ActionRowBuilder<StringSelectMenuBuilder>().addComponents(menu);
}

/** The row shown with the city step, so a member can drop out without half a company on file. */
export function companyCancelRow(): ActionRowBuilder<ButtonBuilder> {
  return new ActionRowBuilder<ButtonBuilder>().addComponents(
    new ButtonBuilder().setCustomId('company:cancel').setLabel('Cancel').setStyle(ButtonStyle.Secondary),
  );
}
