# Jobify Bot

The Discord front door for Jobify: onboarding interview, job and tender search, alerts, and a
daily digest that comes to you as a DM. Every listing it shows comes from the Rust backend —
it never invents a job, an employer or a deadline, and it never invents an answer when the
assistant is down.

Node 20.6+ (uses `--import` for tests), discord.js v14, TypeScript strict.

## Commands

| Command | What it does |
|---|---|
| `/start` | Registers the member by Discord id, then runs the six-step interview: education → experience → sectors (multi-select, 10 max) → location → skills → jobs/tenders/both. Each answer is written to the backend immediately, so stopping halfway still leaves a usable profile. Ends with how many live openings match. |
| `/jobs [query]` | With a query: the purpose-bound assistant. Without: the newest live jobs in the member's sectors. |
| `/tenders [query]` | The same, `kind=tender`. |
| `/alerts` | On/off, instant vs daily, and a button to re-run the interview. |
| `/credits` | Free searches left this period, purchased balance, Buy credits. |
| `/account` | Mints a one-time portal link (15 min, single use) and DMs it. |
| `/company` | Company name + services via a modal. |
| `/help` | Ten lines. |
| `/stop` | Alerts off; the account and profile are kept. |

Free text: a DM to the bot (or an @mention in a server) goes to the assistant with the
sender's Discord id. The reply is the assistant's own sentence plus up to three real rows as
links. A brand-new member is not answered by the assistant at all: their first message starts
the same six-step interview as `/start` (pre-selecting any sectors already on file), and that
welcome costs no query. Writing again mid-interview re-sends the current step rather than
restarting it.

Set `DISCORD_FREE_CHANNEL_ID` to a channel id to make one guild channel conversational
without an @mention — plain messages there take the same path as a DM, onboarding-first
included. Every other guild channel keeps the mention-only rule, and leaving it unset keeps
today's behaviour.

## Invite URL

```
https://discord.com/oauth2/authorize?client_id=1574835560315363418&scope=bot%20applications.commands&permissions=83968
```

`permissions=83968` = Send Messages (2048) + Embed Links (16384) + Read Message History
(65536). `applications.commands` is what lets the global slash commands appear.

## Run

```bash
cp .env.example .env      # fill DISCORD_TOKEN, SCRAPER_INTERNAL_API_KEY
npm install
npm run build
./run.sh                  # sources .env, runs dist/index.js
```

| script | purpose |
|---|---|
| `npm run build` | `tsc` → `dist/` |
| `npm start` | `node dist/index.js` (no build) |
| `npm run dev` | `tsx watch src/index.ts` |
| `npm run typecheck` | `tsc` over `src/` **and** `test/`, no emit |
| `npm test` | `node --import tsx --test test/*.test.ts` |

Global commands are registered on every boot (`client.application.commands.set`), so a new
command appears after a restart without touching the portal. Discord caches per-client for up
to an hour; `GET /applications/{app_id}/commands` shows the truth immediately.

### systemd

```ini
[Unit]
Description=Jobify Discord bot
After=network-online.target

[Service]
Type=simple
User=ubuntu
WorkingDirectory=/home/ubuntu/projects/jobify/bot
EnvironmentFile=/home/ubuntu/projects/jobify/bot/.env
ExecStart=/home/ubuntu/projects/jobify/bot/run.sh
Restart=on-failure
RestartSec=5
KillSignal=SIGTERM
TimeoutStopSec=20

[Install]
WantedBy=multi-user.target
```

SIGTERM destroys the gateway connection and stops the digest scheduler; logs go to stdout as
one JSON object per line, so `journalctl -u jobify-bot -o cat` is already structured. The
token is never logged — the boot line reports only `token_configured: true`.

## MESSAGE CONTENT intent

The bot requests four intents: `Guilds`, `GuildMessages`, `DirectMessages` and
`MessageContent`.

- **DMs work without MESSAGE CONTENT.** A DM to the bot arrives with its content; this is the
  documented way to talk to it.
- **Guild messages need MESSAGE CONTENT.** Without it a message — mention or plain text in
  `DISCORD_FREE_CHANNEL_ID` — arrives with an empty body, so the bot stays silent. The intent
  must be enabled in the developer portal (Applications → Bot → Privileged Gateway Intents)
  *and* requested in `src/index.ts`; requesting one the application has not been approved for
  makes the gateway login fail, which is why the code and the portal setting move together.
  It requires verification above 100 servers. Slash commands are unaffected either way.

## The digest

An in-process scheduler (no external cron, no queue library) ticks every 30 seconds and reads
the wall clock in `DIGEST_TZ` through `Intl`, so any IANA zone works. It fires in the first ten
minutes of `DIGEST_HOUR` — a restart at 08:07 during a deploy still sends that morning's digest
— and a per-day guard means one run per local day.

Each run:

1. `POST /internal/matches/refresh` (a failure here is logged and the drain continues with
   whatever is already queued);
2. `GET /internal/deliveries?limit=200`, grouped by user id;
3. per member: `GET /internal/discord/link/{user_id}` → Discord id, then one DM with at most
   **5 rows**;
4. outcomes: `POST /internal/deliveries/sent`, or `/failed` with a reason.

Failure mapping, which is the part that keeps a queue honest:

| DM error | reason | backend effect |
|---|---|---|
| 50007 / 403 (blocked or DMs closed) | `blocked` | terminal → that member's alerts are paused |
| 10013 / 404 (unknown user) | `deactivated` | terminal → paused |
| anything else | `transient` | recorded, alerts stay on |
| no `discord_links` row for that user id | `no_discord_link` | recorded, alerts stay on (web-only account) |

Rows beyond the fifth stay **pending** rather than failed, so tomorrow's digest picks them up
and nothing is lost. A terminal reason is recorded once, never retried.

## Backend contract

Internal routes, all `x-api-key`:

| Route | Used for |
|---|---|
| `POST /internal/discord/users` | register/refresh on first contact |
| `POST /internal/discord/profile` | one answer at a time (education, experience, sectors, location, skills, wants) |
| `GET /internal/discord/user/{discord_id}` | profile, alerts, credits in one read |
| `POST /internal/discord/link-token` | `/account`, Buy credits |
| `GET /internal/discord/link/{user_id}` | digest: user id → Discord id |
| `POST /internal/discord/alerts` | `/alerts`, `/stop` |
| `POST /internal/discord/company` | `/company` |
| `POST /internal/assistant/query` | `/jobs <query>`, `/tenders <query>`, free text |
| `GET /internal/deliveries`, `POST /internal/deliveries/{sent,failed}`, `POST /internal/matches/refresh` | digest |

Public: `GET /categories` (the 28-slug taxonomy the sector multi-select renders) and
`GET /jobs` (browsing and the "N live openings" count).

### Why `/company` writes through an internal route

`PATCH /me/company` is Bearer-only and the bot has no bearer token. Two options were possible:
make the bot fetch a magic link and act as a browser, or add a snowflake-addressed write. The
second was chosen: `POST /internal/discord/company` takes the same fields and calls the *same*
`apply_company` function as the portal route, so validation, normalization and clear-on-empty
cannot drift between the two surfaces. The bot never mints a token to write data it already has
a key for.

### Why `wants` exists

"What do you want: jobs, tenders or both" is stored as `users.wants` (`jobs` / `tenders` /
absent = both) and applied by the matcher's query, not filtered after the fact — otherwise a
tenders-only member would accumulate job notifications they are never sent.

## Tests

`npm test` runs 56 unit tests with a stubbed backend — no network, no gateway:

- request building and envelope unwrapping, the 402 `out_of_credits` path, 5xx retry and
  timeout classification (`test/http.test.ts`);
- assistant reply mapping (reply + exactly the returned rows as links, no invented rows),
  the delivery outcome mapping, the 5-per-DM cap (`test/format.test.ts`);
- the digest with a mocked backend: one DM per member capped at five, four left pending,
  blocked → `blocked`, unlinked → `no_discord_link` (`test/digest.test.ts`);
- onboarding components, sector paging 25+3, session lifecycle, summary copy
  (`test/onboarding.test.ts`);
- free text: first contact starts the interview with zero assistant calls, an onboarded
  member still gets one answer, mid-interview re-prompts reuse the session, and the optional
  free-text channel is mention-free while other guild channels stay ignored
  (`test/messages.test.ts`);
- the wall clock in `Africa/Nairobi`, the once-a-day guard (`test/scheduler.test.ts`).

## Project layout

```
src/
  index.ts        boot, gateway, command registration, SIGTERM
  env.ts          .env + config (never logs a value)
  log.ts          JSON lines, secret redaction
  http.ts         the only backend client: timeout, retry, typed errors
  types.ts        wire shapes copied from the backend models
  format.ts       every user-visible string, as pure functions
  onboarding.ts   interview state + component builders
  commands.ts     the nine slash commands
  interactions.ts buttons, select and modals
  messages.ts     free-text DMs and mentions
  digest.ts       refresh → drain → DM → report
  scheduler.ts    the in-process daily timer
test/             unit tests, stubbed backend
```
