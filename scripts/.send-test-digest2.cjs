// One-off: run the real digest pipeline for Dayib's pending notifications.
// The refresh, the queue, the renderer and the receipt are production code (dist/digest.js);
// only the transport is guarded so a test run cannot DM anyone else.
const path = '/home/ubuntu/projects/jobify/bot';
const { loadDotEnv, loadConfig } = require(`${path}/dist/env.js`);
const { Backend } = require(`${path}/dist/http.js`);
const { runDigest } = require(`${path}/dist/digest.js`);

loadDotEnv(`${path}/.env`);
const config = loadConfig();
const backend = new Backend({ baseUrl: config.backendBaseUrl, apiKey: config.internalApiKey });

const TARGET_DISCORD = '1432771294968942724';

async function sendDm(discordId, payload) {
  if (discordId !== TARGET_DISCORD) {
    console.log('skipping DM to another member:', discordId);
    return;
  }
  const channelRes = await fetch('https://discord.com/api/v10/users/@me/channels', {
    method: 'POST',
    headers: { Authorization: `Bot ${config.discordToken}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ recipient_id: discordId }),
  });
  if (!channelRes.ok) throw new Error(`dm channel ${channelRes.status}: ${await channelRes.text()}`);
  const channel = await channelRes.json();
  const body = { content: payload.content };
  if (payload.components?.length) {
    body.components = payload.components.map((row) => row.toJSON());
    console.log('buttons attached:', JSON.stringify(body.components[0].components.map((b) => b.label)));
  }
  const sent = await fetch(`https://discord.com/api/v10/channels/${channel.id}/messages`, {
    method: 'POST',
    headers: { Authorization: `Bot ${config.discordToken}`, 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  const sentBody = await sent.json();
  if (!sent.ok) throw new Error(`dm send ${sent.status}: ${JSON.stringify(sentBody)}`);
  console.log('DM sent:', sentBody.id);
  console.log('---- content ----');
  console.log(payload.content);
}

(async () => {
  const report = await runDigest({
    backend,
    sendDm,
    appBaseUrl: config.appBaseUrl,
    logger: {
      info: (m, f) => console.log('info', m, f ?? ''),
      warn: (m, f) => console.log('warn', m, f ?? ''),
      error: (m, f) => console.log('error', m, f ?? ''),
      debug: () => {},
    },
  });
  console.log('report:', report);
})().catch((err) => {
  console.error('failed:', err.message);
  process.exit(1);
});
