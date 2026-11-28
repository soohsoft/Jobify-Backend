/**
 * The Telegram front end: one bot, the same product as Discord.
 *
 * The backend already owns every decision — the categories a degree maps to, the score, the
 * digest queue — so this file only translates: Telegram updates in, backend calls, Telegram
 * messages out. Nothing here re-derives matching.
 *
 * Identity: Telegram ids are passed to the backend in its `discord_id` slot. That column is
 * "the chat identity of a member", not "a Discord snowflake" — the two id spaces cannot collide
 * (Discord ids are 18-19 digits, Telegram ids are up to 10).
 */
import { Backend } from '../http.js';
import type { Logger } from '../log.js';
import type { FeedJob, UserEnvelope } from '../types.js';
import { TelegramClient, type InlineKeyboard, type TgUpdate } from './client.js';
import { frame, jobBlock } from './format.js';

/**
 * The floor for the "other sectors" block: base (20) + picked sector (10) is 30, which is a row
 * admitted purely because a sector was ticked. 35 means at least one word of the member's work
 * history (or their education's category) is present, so the list stays related rather than
 * merely permitted.
 */
const RELATED_MIN_SCORE = 35;
import {
  CID,
  EMPLOYERS,
  FIELDS,
  FIELDS_ASKED,
  LEVELS,
  POSITIONS,
  YEARS,
  TgSessions,
  anotherPrompt,
  browseKeyboard,
  companyCityPrompt,
  companyNamePrompt,
  companyServicesPrompt,
  completionText,
  employerPrompt,
  fieldPrompt,
  levelPrompt,
  masterPrompt,
  namePrompt,
  positionPrompt,
  positionsPrompt,
  yearsPrompt,
  type TgSession,
} from './prompts.js';

export interface TgBotConfig {
  bot: TelegramClient;
  backend: Backend;
  sessions: TgSessions;
  logger: Logger;
  appBaseUrl: string;
  digestTz: string;
  digestHour: number;
}

/** Telegram HTML: only these three characters must be escaped outside tags. */
function html(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function levelLabel(key: string): string {
  return LEVELS.find((level) => level.key === key)?.label ?? key;
}

function fieldLabel(key: string): string {
  return FIELDS.find((field) => field.key === key)?.label ?? key;
}

export class TgBot {
  private readonly bot: TelegramClient;
  private readonly backend: Backend;
  private readonly sessions: TgSessions;
  private readonly logger: Logger;

  constructor(private readonly config: TgBotConfig) {
    this.bot = config.bot;
    this.backend = config.backend;
    this.sessions = config.sessions;
    this.logger = config.logger;
  }

  registerCommands(): Promise<void> {
    return this.bot.setCommands([
      { command: 'start', description: 'Set up your job profile' },
      { command: 'jobs', description: 'Jobs matched to you' },
      { command: 'tenders', description: 'Tenders for your company' },
      { command: 'balance', description: 'How many searches you have left' },
      { command: 'account', description: 'Link to your Jobify account' },
      { command: 'stop', description: 'Stop job notifications' },
    ]);
  }

  async handle(update: TgUpdate): Promise<void> {
    if (update.callback_query) {
      await this.onCallback(update.callback_query.id, update.callback_query.from.id, update.callback_query.data ?? '', update.callback_query.message?.message_id);
      return;
    }
    if (update.message?.text) {
      await this.onMessage(update.message.chat.id, update.message.from?.id ?? update.message.chat.id, update.message.text);
    }
  }

  // -------------------------------------------------------------------------
  // Messages
  // -------------------------------------------------------------------------

  private async onMessage(chatId: number, userId: number, raw: string): Promise<void> {
    const text = raw.trim();
    if (text.startsWith('/')) {
      await this.onCommand(chatId, userId, text.split(/\s+/)[0].slice(1).toLowerCase());
      return;
    }

    const session = this.sessions.get(userId);
    if (!session) {
      // Any plain message from someone with no profile starts the interview, exactly like
      // the Discord flow: there is nothing to search with yet.
      await this.startInterview(chatId, userId);
      return;
    }

    switch (session.step) {
      case 'name': {
        if (text.length < 2 || text.length > 80) {
          await this.bot.sendMessage(chatId, 'Send your name — 2 to 80 characters.');
          return;
        }
        session.name = text;
        session.step = 'education_level';
        session.updatedAt = Date.now();
        await this.sendStep(chatId, levelPrompt());
        return;
      }
      case 'position': {
        if (text.length < 2 || text.length > 160) {
          await this.bot.sendMessage(
            chatId,
            'Send the positions — 2 to 160 characters, separated by commas.',
          );
          return;
        }
        session.lastPosition = text;
        try {
          await this.backend.saveProfile({ discord_id: String(userId), last_position: text });
        } catch (err) {
          await this.fail(chatId, err);
          return;
        }
        session.step = 'years';
        session.updatedAt = Date.now();
        await this.sendStep(chatId, yearsPrompt());
        return;
      }
      case 'company_name': {
        session.companyName = text;
        session.step = 'company_services';
        session.updatedAt = Date.now();
        await this.sendStep(chatId, companyServicesPrompt());
        return;
      }
      case 'company_services': {
        session.companyServices = text;
        session.step = 'company_city';
        session.updatedAt = Date.now();
        await this.sendStep(chatId, companyCityPrompt());
        return;
      }
      default: {
        // Mid-interview, a stray message re-sends the step instead of starting over — the
        // answers already given are kept (and the ones already saved are on the server).
        await this.bot.sendMessage(chatId, 'Let us finish the questions first — here is where you are.');
        await this.replayStep(chatId, session);
      }
    }
  }

  private async onCommand(chatId: number, userId: number, command: string): Promise<void> {
    switch (command) {
      case 'start':
        await this.startInterview(chatId, userId);
        return;
      case 'jobs':
        await this.sendJobs(chatId, userId, 'job');
        return;
      case 'tenders':
        await this.sendTenders(chatId, userId);
        return;
      case 'balance':
        await this.sendBalance(chatId, userId);
        return;
      case 'account':
        await this.sendAccount(chatId, userId);
        return;
      case 'stop':
        await this.stop(chatId, userId);
        return;
      case 'sectors':
        await this.editSectors(chatId, userId);
        return;
      case 'help':
        await this.bot.sendMessage(
          chatId,
          '<b>Jobify</b>\n/start — set up your profile\n/jobs — jobs matched to you\n/tenders — tenders for your company\n/sectors — change the other sectors you follow\n/balance — searches left\n/account — link to your account\n/stop — stop notifications',
        );
        return;
      default:
        await this.bot.sendMessage(chatId, 'Commands: /start, /jobs, /tenders, /balance, /account, /stop');
    }
  }

  // -------------------------------------------------------------------------
  // The interview
  // -------------------------------------------------------------------------

  private async startInterview(chatId: number, userId: number): Promise<void> {
    const session = this.sessions.start(userId);
    try {
      await this.backend.upsertUser({ discord_id: String(userId), username: `tg_${userId}` });
    } catch (err) {
      await this.fail(chatId, err);
      return;
    }
    void session;
    await this.sendStep(chatId, namePrompt());
  }

  private async replayStep(chatId: number, session: TgSession): Promise<void> {
    switch (session.step) {
      case 'name':
        await this.sendStep(chatId, namePrompt());
        return;
      case 'education_level':
        await this.sendStep(chatId, levelPrompt());
        return;
      case 'education_field':
        await this.sendStep(chatId, fieldPrompt(session.pendingLevel ?? 'bachelor'));
        return;
      case 'another_degree':
        await this.sendStep(chatId, anotherPrompt(session.education));
        return;
      case 'position':
        await this.sendStep(chatId, positionPrompt());
        return;
      case 'years':
        await this.sendStep(chatId, yearsPrompt());
        return;
      case 'employer_types':
        await this.sendStep(chatId, employerPrompt(session));
        return;
      case 'positions':
        await this.sendStep(chatId, positionsPrompt(session));
        return;
      case 'company_name':
        await this.sendStep(chatId, companyNamePrompt());
        return;
      case 'company_services':
        await this.sendStep(chatId, companyServicesPrompt());
        return;
      case 'company_city':
        await this.sendStep(chatId, companyCityPrompt());
        return;
      default:
        await this.bot.sendMessage(chatId, 'You are all set — /jobs shows your matches.');
    }
  }

  private async sendStep(chatId: number, step: { text: string; keyboard: InlineKeyboard }): Promise<void> {
    await this.bot.sendMessage(chatId, step.text, step.keyboard.length > 0 ? step.keyboard : undefined);
  }

  // -------------------------------------------------------------------------
  // Buttons
  // -------------------------------------------------------------------------

  private async onCallback(queryId: string, userId: number, data: string, messageId?: number): Promise<void> {
    const chatId = userId; // private chat: the chat id IS the user id
    const session = this.sessions.get(userId);

    const redraw = async (step: { text: string; keyboard: InlineKeyboard }): Promise<void> => {
      await this.bot.answerCallback(queryId);
      if (messageId) {
        await this.bot.editMessage(chatId, messageId, step.text, step.keyboard.length > 0 ? step.keyboard : undefined);
      } else {
        await this.sendStep(chatId, step);
      }
    };

    if (data.startsWith('onb:lvl:')) {
      if (!session) return this.expired(queryId, chatId);
      const level = data.slice('onb:lvl:'.length);
      session.pendingLevel = level;
      session.updatedAt = Date.now();
      if (FIELDS_ASKED.includes(level)) {
        session.step = 'education_field';
        await redraw(fieldPrompt(level));
      } else {
        session.education.push({ level, field: '' });
        try {
          await this.backend.saveProfile({ discord_id: String(userId), education: session.education });
        } catch (err) {
          return this.fail(chatId, err);
        }
        session.step = 'another_degree';
        await redraw(anotherPrompt(session.education));
      }
      return;
    }

    if (data.startsWith('onb:fld:')) {
      if (!session?.pendingLevel) return this.expired(queryId, chatId);
      const key = data.slice('onb:fld:'.length);
      const level = session.pendingLevel;
      const label = key === 'other' ? 'Other' : fieldLabel(key);
      session.education.push({ level, field: label });
      try {
        await this.backend.saveProfile({ discord_id: String(userId), education: session.education });
      } catch (err) {
        return this.fail(chatId, err);
      }
      session.pendingLevel = undefined;
      session.updatedAt = Date.now();
      if (level === 'bachelor') {
        session.step = 'another_degree';
        await redraw(masterPrompt());
      } else {
        session.step = 'another_degree';
        await redraw(anotherPrompt(session.education));
      }
      return;
    }

    if (data.startsWith('onb:master:')) {
      if (!session) return this.expired(queryId, chatId);
      if (data.endsWith('yes')) {
        session.pendingLevel = 'master';
        session.step = 'education_field';
        await redraw(fieldPrompt('master'));
      } else {
        session.step = 'position';
        await redraw(positionPrompt());
      }
      return;
    }

    if (data.startsWith('onb:another:')) {
      if (!session) return this.expired(queryId, chatId);
      if (data.endsWith('yes')) {
        session.step = 'education_level';
        await redraw(levelPrompt());
      } else {
        session.step = 'position';
        await redraw(positionPrompt());
      }
      return;
    }

    if (data.startsWith('onb:yrs:')) {
      if (!session) return this.expired(queryId, chatId);
      const key = data.slice('onb:yrs:'.length);
      const band = YEARS.find((entry) => entry.key === key);
      session.experienceYears = Number(band?.key ?? 0);
      session.step = 'employer_types';
      session.updatedAt = Date.now();
      await redraw(employerPrompt(session));
      return;
    }

    if (data.startsWith('onb:emp:')) {
      if (!session) return this.expired(queryId, chatId);
      const value = data.slice('onb:emp:'.length);
      if (value === 'done') {
        session.step = 'positions';
        await redraw(positionsPrompt(session));
        return;
      }
      const slug = EMPLOYERS.find((employer) => employer.slug === value)?.slug;
      if (!slug) return this.expired(queryId, chatId);
      session.employerTypes = session.employerTypes.includes(slug)
        ? session.employerTypes.filter((item) => item !== slug)
        : [...session.employerTypes, slug];
      session.updatedAt = Date.now();
      await redraw(employerPrompt(session));
      return;
    }

    if (data.startsWith('onb:posp:')) {
      if (!session) return this.expired(queryId, chatId);
      session.positionsPage = Number(data.slice('onb:posp:'.length)) || 0;
      await redraw(positionsPrompt(session));
      return;
    }

    if (data === CID.positionsAll) {
      if (!session) return this.expired(queryId, chatId);
      // "All" means every sector, not "no filter": an empty list would leave the member with
      // no picked sector at all, and the matcher would then rank them below the cut-off.
      session.positions = POSITIONS.map((position) => position.slug);
      await this.finishInterview(queryId, chatId, messageId, session);
      return;
    }

    if (data === CID.positionsDone) {
      if (!session) return this.expired(queryId, chatId);
      // Done with nothing picked is not an answer: the sector list is what the matcher ranks
      // on, and an empty one produces "no jobs" for a member who finished the interview.
      if (session.positions.length === 0) {
        await this.bot.answerCallback(queryId, 'Pick at least one sector — or tap All.');
        return;
      }
      await this.finishInterview(queryId, chatId, messageId, session);
      return;
    }

    if (data.startsWith('onb:pos:')) {
      if (!session) return this.expired(queryId, chatId);
      const slug = data.slice('onb:pos:'.length);
      if (!POSITIONS.some((position) => position.slug === slug)) return this.expired(queryId, chatId);
      session.positions = session.positions.includes(slug)
        ? session.positions.filter((item) => item !== slug)
        : [...session.positions, slug];
      session.updatedAt = Date.now();
      await redraw(positionsPrompt(session));
      return;
    }

    // ---------------------------------------------------------------------
    // Out of the interview: the tabs, the account link, the company steps
    // ---------------------------------------------------------------------

    if (data === CID.browseJobs) {
      await this.bot.answerCallback(queryId);
      await this.sendJobs(chatId, userId, 'job');
      return;
    }
    if (data === CID.browseTenders || data === CID.companyOpen) {
      await this.bot.answerCallback(queryId);
      await this.sendTenders(chatId, userId);
      return;
    }
    if (data === CID.account) {
      await this.bot.answerCallback(queryId);
      await this.sendAccount(chatId, userId);
      return;
    }
    if (data.startsWith('company:city:')) {
      const city = data.slice('company:city:'.length);
      const live = this.sessions.get(userId);
      if (!live?.companyName) return this.expired(queryId, chatId);
      try {
        await this.backend.setCompany({
          discord_id: String(userId),
          name: live.companyName,
          services: (live.companyServices ?? '').split(',').map((part) => part.trim()).filter(Boolean),
          location: city,
        });
      } catch (err) {
        return this.fail(chatId, err);
      }
      this.sessions.clear(userId);
      await this.bot.answerCallback(queryId, 'Company saved');
      await this.bot.sendMessage(chatId, 'Company saved ✓');
      await this.sendJobs(chatId, userId, 'tender');
      return;
    }

    await this.expired(queryId, chatId);
  }

  /**
   * `/sectors` — reopen the sector picker with the member's current picks.
   *
   * The preference lives on the profile, so the screen has to start from what is stored: an
   * empty picker would silently wipe the sectors they chose during onboarding.
   */
  private async editSectors(chatId: number, userId: number): Promise<void> {
    let envelope: UserEnvelope;
    try {
      envelope = await this.backend.getUser(String(userId));
    } catch (err) {
      await this.fail(chatId, err);
      return;
    }
    const session = this.sessions.start(userId);
    session.step = 'positions';
    session.editSectors = true;
    session.positions = [...(envelope.user.match_profile?.positions ?? [])];
    session.employerTypes = [...(envelope.user.match_profile?.employer_types ?? [])];
    const prompt = positionsPrompt(session);
    await this.bot.sendMessage(chatId, prompt.text, prompt.keyboard);
  }

  private async finishInterview(
    queryId: string,
    chatId: number,
    messageId: number | undefined,
    session: TgSession,
  ): Promise<void> {
    const userId = chatId;

    // /sectors is an edit of one answer: it writes the sector list and stops there, because
    // re-sending the interview's completion card would claim the profile was just created.
    if (session.editSectors) {
      try {
        await this.backend.saveProfile({
          discord_id: String(userId),
          positions: session.positions,
        });
      } catch (err) {
        await this.bot.answerCallback(queryId);
        return this.fail(chatId, err);
      }
      this.sessions.clear(userId);
      await this.bot.answerCallback(queryId, 'Sectors updated');
      let matches = 0;
      try {
        const list = await this.backend.jobsForUser(String(userId), { limit: 10, kind: 'job' });
        matches = list.rows.length;
      } catch {
        // A read failure must not make a successful save look broken.
      }
      const text = [
        '<b>Sectors updated ✓</b>',
        '',
        `${session.positions.length} sector(s) followed — ${matches} job(s) match now.`,
        '',
        'Tap My Jobs to see them, or /sectors again to change them.',
      ].join('\n');
      if (messageId) {
        await this.bot.editMessage(chatId, messageId, text, browseKeyboard());
      } else {
        await this.bot.sendMessage(chatId, text, browseKeyboard());
      }
      return;
    }

    try {
      await this.backend.saveProfile({
        discord_id: String(userId),
        name: session.name,
        education: session.education,
        last_position: session.lastPosition,
        experience_years: session.experienceYears,
        employer_types: session.employerTypes,
        positions: session.positions,
      });
    } catch (err) {
      await this.bot.answerCallback(queryId);
      return this.fail(chatId, err);
    }
    session.step = 'done';
    session.completed = true;
    this.sessions.clear(userId);
    await this.bot.answerCallback(queryId);

    let matches = 0;
    try {
      const list = await this.backend.jobsForUser(String(userId), { limit: 5, kind: 'job' });
      matches = list.rows.length;
    } catch {
      // The interview succeeded; a match read failure must not turn it into an error.
    }

    const text = completionText({
      name: html(session.name ?? 'there'),
      matches,
      alertsHour: this.config.digestHour,
      timeZone: this.config.digestTz,
    });
    if (messageId) {
      await this.bot.editMessage(chatId, messageId, text, browseKeyboard());
    } else {
      await this.bot.sendMessage(chatId, text, browseKeyboard());
    }
    this.logger.info('telegram onboarding complete', { telegramId: userId, matches });
  }

  // -------------------------------------------------------------------------
  // Reads
  // -------------------------------------------------------------------------

  private async sendJobs(chatId: number, userId: number, kind: 'job' | 'tender'): Promise<void> {
    // Nothing can match before the interview is answered, so a tap on My Jobs from an empty
    // profile goes to the interview instead of an empty list that looks like our fault.
    try {
      const envelope = await this.backend.getUser(String(userId));
      if (!envelope.onboarding_complete) {
        await this.startInterview(chatId, userId);
        return;
      }
    } catch (err) {
      await this.fail(chatId, err);
      return;
    }
    try {
      // Two reads of the same matcher: the cut-off decides what is a MATCH, and `threshold=0`
      // returns everything the member's areas admit — the difference is the RELATED list, the
      // rows that come from the sectors they picked without clearing the bar.
      const matched = await this.backend.jobsForUser(String(userId), { limit: 10, kind });
      const everything = await this.backend.jobsForUser(String(userId), {
        limit: 30,
        kind,
        threshold: 0,
      });
      const matchedIds = new Set(matched.rows.map((row) => row.job._id));
      // A sector the member ticked widens what they CAN see, but a row that is only in the
      // list because of that tick — not in their education's category, and sharing none of
      // their worked-position words — scores 30 and has no business in front of them. That is
      // how a Management/NGO member was shown surgery and nutrition posts.
      const related = everything.rows
        .filter((row) => !matchedIds.has(row.job._id) && row.matchScore >= RELATED_MIN_SCORE)
        .slice(0, 8);
      const noun = kind === 'job' ? 'Jobs' : 'Tenders';
      const block = (rows: typeof matched.rows) =>
        rows
          .map((row, index) =>
            jobBlock(row.job, index + 1, `${this.config.appBaseUrl}/jobs/${row.job._id}`),
          )
          .join('\n\n');

      if (matched.rows.length === 0 && related.length === 0) {
        const title = kind === 'job' ? 'My Jobs' : 'My Tenders';
        await this.bot.sendMessage(
          chatId,
          `Nothing live matches you this minute — I check again every day and will message you the moment something does.\n\n<b>${title}</b>`,
          browseKeyboard(),
        );
        return;
      }

      const sections: string[] = [];
      if (matched.rows.length > 0) sections.push(frame(`Matched ${noun}`, block(matched.rows)));
      if (related.length > 0) sections.push(frame('Other sectors you select', block(related)));
      await this.bot.sendMessage(chatId, sections.join('\n\n'), browseKeyboard());
    } catch (err) {
      await this.fail(chatId, err);
    }
  }

  private async sendTenders(chatId: number, userId: number): Promise<void> {
    let envelope: UserEnvelope;
    try {
      envelope = await this.backend.getUser(String(userId));
    } catch (err) {
      await this.fail(chatId, err);
      return;
    }
    if (!envelope.user.company) {
      // Straight into the first question: a "Start" button between the tap and the question
      // is one tap of nothing, and the question is the answer they tapped for.
      const session = this.sessions.start(userId);
      session.step = 'company_name';
      await this.bot.sendMessage(
        chatId,
        '<b>Please fill your company information to search relevant tenders for your company.</b>',
      );
      await this.sendStep(chatId, companyNamePrompt());
      return;
    }
    await this.sendJobs(chatId, userId, 'tender');
  }

  private async sendBalance(chatId: number, userId: number): Promise<void> {
    try {
      await this.backend.upsertUser({ discord_id: String(userId), username: `tg_${userId}` });
      const data = await this.backend.getUser(String(userId));
      const credits = data.credits;
      await this.bot.sendMessage(
        chatId,
        [
          '<b>Credits</b>',
          `Free searches left this period: <b>${credits.remaining_this_period} of ${credits.monthly_allowance}</b>`,
          `Purchased searches: <b>${credits.balance}</b>`,
          '',
          'One smart search costs one query. Browsing and your alerts are free.',
          '',
          'Never miss a job. Daily Jobs Notifications just for $1/month.',
        ].join('\n'),
        [[{ text: 'My Account', callback_data: CID.account }]],
      );
    } catch (err) {
      await this.fail(chatId, err);
    }
  }

  private async sendAccount(chatId: number, userId: number): Promise<void> {
    try {
      await this.backend.upsertUser({ discord_id: String(userId), username: `tg_${userId}` });
      const link = await this.backend.mintLink(String(userId), 600);
      await this.bot.sendMessage(
        chatId,
        `<b>Your Jobify account page</b> — subscription, searches and your company profile.\n\n${link.url}\n\nValid for 10 minutes.`,
      );
    } catch (err) {
      await this.fail(chatId, err);
    }
  }

  private async stop(chatId: number, userId: number): Promise<void> {
    try {
      await this.backend.setAlerts(String(userId), false);
      await this.bot.sendMessage(chatId, 'Notifications stopped. /start brings them back.');
    } catch (err) {
      await this.fail(chatId, err);
    }
  }

  private async expired(queryId: string, chatId: number): Promise<void> {
    await this.bot.answerCallback(queryId, 'That step expired — send /start');
    await this.bot.sendMessage(chatId, 'That step expired. Send /start to begin again — anything you already answered is saved.');
  }

  private async fail(chatId: number, err: unknown): Promise<void> {
    this.logger.warn('telegram handler failed', { message: err instanceof Error ? err.message : String(err) });
    await this.bot.sendMessage(chatId, 'Something went wrong on my side. Please try again in a moment.');
  }
}
