/**
 * Boot: read config, connect the gateway, register the global commands, arm the digest.
 *
 * Intents are exactly what the product needs — Guilds (to know its servers), GuildMessages
 * (mentions in a server and the optional free-text channel) and DirectMessages (the
 * documented way to talk to the bot), plus MessageContent because a guild message arrives
 * with an empty body without it. Slack-like "type in the channel" only works when the
 * privileged intent is both requested here and enabled for the application in the developer
 * portal; Discord refuses the login if it is requested but not approved.
 */
import { Client, Events, GatewayIntentBits, Partials } from 'discord.js';

import { buildCommands } from './commands.js';
import {
  RequestGuard,
  createCategoryCache,
  createChannelSender,
  createDmSender,
  type BotContext,
} from './context.js';
import { runDigest } from './digest.js';
import { loadConfig, loadDotEnv } from './env.js';
import { Backend } from './http.js';
import { ensureIntro, introContent, type IntroChannel } from './intro.js';
import { handleInteraction } from './interactions.js';
import { createLogger, errorFields } from './log.js';
import { handleMessage } from './messages.js';
import { OnboardingSessions } from './onboarding.js';
import { DailyDigestScheduler } from './scheduler.js';
import { handleMemberJoin } from './welcome.js';

async function main(): Promise<void> {
  loadDotEnv();
  const config = loadConfig();
  const logger = createLogger((process.env.LOG_LEVEL as 'debug' | 'info' | 'warn' | 'error') ?? 'info');

  const backend = new Backend({
    baseUrl: config.backendBaseUrl,
    apiKey: config.internalApiKey,
    logger,
  });

  const client = new Client({
    intents: [
      GatewayIntentBits.Guilds,
      GatewayIntentBits.GuildMessages,
      // `GuildMemberAdd` is only dispatched with this privileged intent. Without it a new
      // member is silent until they write first — the exact behaviour this feature removes.
      GatewayIntentBits.GuildMembers,
      GatewayIntentBits.DirectMessages,
      // Guild message bodies (mentions and the free-text channel) are empty without this.
      GatewayIntentBits.MessageContent,
    ],
    // A DM channel created before this process started arrives partially hydrated; without
    // this, answering a member who wrote while the bot was restarting throws.
    partials: [Partials.Channel],
  });

  const ctx: BotContext = {
    backend,
    appBaseUrl: config.appBaseUrl,
    digestTz: config.digestTz,
    digestHour: config.digestHour,
    logger,
    sessions: new OnboardingSessions(),
    ...(config.freeChannelId ? { freeChannelId: config.freeChannelId } : {}),
    categories: createCategoryCache(backend),
    sendDm: createDmSender(client),
    sendChannel: createChannelSender(client),
    guard: new RequestGuard(),
  };

  const scheduler = new DailyDigestScheduler({
    timeZone: config.digestTz,
    hour: config.digestHour,
    logger,
    run: () =>
      runDigest({
        backend,
        sendDm: ctx.sendDm,
        appBaseUrl: config.appBaseUrl,
        logger,
      }),
  });

  client.once(Events.ClientReady, (ready) => {
    logger.info('gateway ready', { username: ready.user.username, id: ready.user.id });
    void (async () => {
      const commands = buildCommands();
      try {
        await ready.application.commands.set(commands);
        logger.info('global commands registered', {
          count: commands.length,
          names: commands.map((command) => command.name),
        });
      } catch (err) {
        // Commands may already exist from a previous deploy; the bot is still useful, so this
        // is loud but not fatal.
        logger.error('command registration failed', errorFields(err));
      }
      // Guild-scoped copies SHADOW the global ones, and this app has registered both (instant
      // visibility while testing). A copy left over from an older build keeps a removed
      // command (`/start`, `/credits`) in the picker no matter what the global set says, so
      // every guild is brought to the same list on boot. One guild in practice; a failure
      // here is logged and never fatal — the global set is already correct.
      for (const guild of ready.guilds.cache.values()) {
        try {
          await guild.commands.set(commands);
          logger.info('guild commands registered', { guildId: guild.id, count: commands.length });
        } catch (err) {
          logger.error('guild command registration failed', { guildId: guild.id, ...errorFields(err) });
        }
      }
      scheduler.start();

      // The pinned intro is the channel's front door; a failure here must not stop the bot.
      if (config.freeChannelId) {
        try {
          const channel = await ready.channels.fetch(config.freeChannelId);
          if (!channel || !('messages' in channel)) {
            logger.warn('intro skipped: free channel is not a text channel', {
              channelId: config.freeChannelId,
            });
          } else {
            const outcome = await ensureIntro(
              channel as unknown as IntroChannel,
              logger,
              introContent(config.digestHour, config.digestTz),
            );
            logger.info('intro ensured', { channelId: config.freeChannelId, outcome });
          }
        } catch (err) {
          logger.error('intro ensure failed', errorFields(err));
        }
      }
    })();
  });

  client.on(Events.InteractionCreate, (interaction) => {
    // One answer per interaction. A redelivered event would otherwise run the command again
    // and send the same list twice.
    if (!ctx.guard.claim(interaction.id)) {
      logger.warn('duplicate interaction ignored', { id: interaction.id, type: interaction.type });
      return;
    }
    void handleInteraction(interaction, ctx).catch((err) => {
      logger.error('interaction handler threw', {
        type: interaction.type,
        ...errorFields(err),
      });
    });
  });

  client.on(Events.MessageCreate, (message) => {
    if (!ctx.guard.claim(`msg:${message.id}`)) {
      logger.warn('duplicate message ignored', { id: message.id });
      return;
    }
    const botId = client.user?.id ?? config.discordAppId;
    void handleMessage(message, ctx, botId).catch((err) => {
      logger.error('message handler threw', errorFields(err));
    });
  });

  client.on(Events.GuildMemberAdd, (member) => {
    // The handler already catches its own delivery failures; this is the last-resort guard so
    // one bad join can never take the process down.
    void handleMemberJoin(member, ctx).catch((err) => {
      logger.error('member join handler threw', { discordId: member.user?.id, ...errorFields(err) });
    });
  });

  client.on(Events.Error, (err) => {
    logger.error('gateway error', errorFields(err));
  });

  client.rest.on('rateLimited', (info) => {
    logger.warn('discord rate limit', { route: info.route, timeout: info.timeToReset });
  });

  const shutdown = (signal: string): void => {
    logger.info('shutting down', { signal });
    scheduler.stop();
    void client.destroy().finally(() => process.exit(0));
    // A hung destroy must not leave the process on the gateway forever.
    setTimeout(() => process.exit(1), 5_000).unref();
  };
  process.on('SIGTERM', () => shutdown('SIGTERM'));
  process.on('SIGINT', () => shutdown('SIGINT'));

  process.on('unhandledRejection', (reason) => {
    logger.error('unhandled rejection', errorFields(reason));
  });

  logger.info('starting', {
    app_id: config.discordAppId,
    backend: config.backendBaseUrl,
    app_base_url: config.appBaseUrl,
    digest: `${config.digestHour}:00 ${config.digestTz}`,
    // Presence of the token is reported, never the value.
    token_configured: config.discordToken.length > 0,
  });

  await client.login(config.discordToken);
}

main().catch((err) => {
  // Config errors land here before any logger exists; stderr plus a non-zero exit is what a
  // systemd unit needs to see.
  console.error(JSON.stringify({ level: 'error', msg: 'fatal startup error', ...errorFields(err) }));
  process.exit(1);
});
