/**
 * Boot: read config, connect the gateway, register the global commands, arm the digest.
 *
 * Intents are exactly what the product needs — Guilds (to know its servers), GuildMessages
 * (mentions in a server) and DirectMessages (the documented way to talk to the bot). No
 * MessageContent: DMs work without it, and a server mention needs it enabled in the developer
 * portal, which the README explains.
 */
import { Client, Events, GatewayIntentBits, Partials } from 'discord.js';

import { buildCommands } from './commands.js';
import { createCategoryCache, createDmSender, type BotContext } from './context.js';
import { runDigest } from './digest.js';
import { loadConfig, loadDotEnv } from './env.js';
import { Backend } from './http.js';
import { handleInteraction } from './interactions.js';
import { createLogger, errorFields } from './log.js';
import { handleMessage } from './messages.js';
import { OnboardingSessions } from './onboarding.js';
import { DailyDigestScheduler } from './scheduler.js';

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
    intents: [GatewayIntentBits.Guilds, GatewayIntentBits.GuildMessages, GatewayIntentBits.DirectMessages],
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
      scheduler.start();
    })();
  });

  client.on(Events.InteractionCreate, (interaction) => {
    void handleInteraction(interaction, ctx).catch((err) => {
      logger.error('interaction handler threw', {
        type: interaction.type,
        ...errorFields(err),
      });
    });
  });

  client.on(Events.MessageCreate, (message) => {
    const botId = client.user?.id ?? config.discordAppId;
    void handleMessage(message, ctx, botId).catch((err) => {
      logger.error('message handler threw', errorFields(err));
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
