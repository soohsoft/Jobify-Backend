// Verify what actually landed: read the last DM in Dayib's channel and print its components.
const path = '/home/ubuntu/projects/jobify/bot';
const { loadDotEnv, loadConfig } = require(`${path}/dist/env.js`);
loadDotEnv(`${path}/.env`);
const config = loadConfig();
const TARGET_DISCORD = '1432771294968942724';

(async () => {
  const auth = { Authorization: `Bot ${config.discordToken}`, 'Content-Type': 'application/json' };
  const chRes = await fetch('https://discord.com/api/v10/users/@me/channels', {
    method: 'POST',
    headers: auth,
    body: JSON.stringify({ recipient_id: TARGET_DISCORD }),
  });
  const channel = await chRes.json();
  const msgs = await (await fetch(`https://discord.com/api/v10/channels/${channel.id}/messages?limit=3`, { headers: auth })).json();
  for (const m of msgs) {
    const labels = (m.components ?? []).flatMap((row) => (row.components ?? []).map((b) => b.label));
    console.log(`${m.id} | ${new Date(m.timestamp).toISOString()} | buttons: ${JSON.stringify(labels)}`);
    console.log('   ' + (m.content || '').split('\n')[0].slice(0, 90));
  }
})().catch((e) => {
  console.error('failed:', e.message);
  process.exit(1);
});
