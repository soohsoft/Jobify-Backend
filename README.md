# jobify-backend

Core jobs-board API for Jobify: Rust + Axum 0.8, MongoDB (`mongodb` v3), JWT auth with
email one-time codes, the public jobs feed (jobs and tenders), saved jobs, profile
matching, alerts, notifications, and the scraper's internal ingest endpoints.

The product is **Discord-first**: a member registers inside the bot, identity is the
Discord snowflake, and the web portal exists only for payment/account management, entered
through a one-time link the bot mints. `POST /internal/discord/*` is the bot's API;
`POST /auth/magic` is the only way into the portal.

Deliberately **not** in this service: the CV builder, the LLM chat, credits, and a payment
gateway. They were removed with the old code; the collections they used are untouched.
`POST /subscription/start` records a pending $1.00 manual payment and says plainly that no
gateway is wired.

## Run

```bash
cp .env.example .env    # fill MONGODB_URI, JWT_SECRET, SCRAPER_INTERNAL_API_KEY
./run.sh --build        # listens on 0.0.0.0:3000
```

Leave `SMTP_HOST` empty to run in console mode: verification and reset codes are written to
the server log (`mail> ...`) instead of being sent, which makes signup testable with no
mailbox credentials.

## Routes

Every response is `{"status": "success", "data": ..., "meta": ...}`; every error is
`{"status": "error", "code": ..., "message": ...}`.

| Method | Path | Auth |
|---|---|---|
| GET | `/`, `/health` | public |
| POST | `/auth/register`, `/auth/login` | public |
| POST | `/auth/magic` (`{token}` → session + `subscription`) | public |
| POST | `/auth/forgot-password`, `/auth/reset-password` | public |
| GET, PATCH | `/auth/me` | Bearer |
| PATCH | `/me/company` (empty `name` clears it) | Bearer |
| PUT, DELETE | `/me/match-profile` | Bearer |
| POST | `/auth/verify-email`, `/auth/resend-otp` | Bearer |
| GET, POST | `/subscription`, `/subscription/start` | Bearer |
| GET | `/jobs` (`q`, `location`, `category`, `organization`, `kind`, `page`, `limit`) | public, optional Bearer |
| GET | `/jobs/{id}` | public |
| GET | `/jobs/match` (`limit`, `record`) | Bearer |
| GET | `/jobs/saved` | Bearer |
| POST, DELETE | `/jobs/{id}/save` | Bearer |
| GET, PUT | `/alerts` | Bearer |
| POST | `/jobs/{id}/not-a-fit` (`scope`: employer \| category) | Bearer |
| GET | `/notifications`, `/notifications/unread-count` | Bearer |
| PATCH | `/notifications/read-all`, `/notifications/{id}/read` | Bearer |
| GET | `/credits` | Bearer |
| POST | `/assistant/query` (`{message}`) | Bearer |
| POST | `/credits/topup` (`{discord_id \| user_id, queries}`) | `x-api-key` |
| POST | `/internal/assistant/query` (`{discord_id, message}`) | `x-api-key` |
| GET | `/internal/sources` | `x-api-key` |
| POST | `/internal/jobs` (JSON array of `JobInput`, optional `kind`) | `x-api-key` |
| POST | `/internal/matches/refresh` | `x-api-key` |
| GET | `/internal/deliveries` | `x-api-key` |
| POST | `/internal/deliveries/sent`, `/internal/deliveries/failed` | `x-api-key` |
| POST | `/internal/discord/users` (`discord_id`, `username`, `global_name?`) | `x-api-key` |
| POST | `/internal/discord/profile` (`discord_id` + onboarding answers) | `x-api-key` |
| POST | `/internal/discord/link-token` (`discord_id`, `ttl_seconds?`) | `x-api-key` |
| GET | `/internal/discord/user/{discord_id}` | `x-api-key` |
| GET | `/internal/discord/matches/{discord_id}?limit=5` (scored rows, same matcher as `/jobs/match`) | `x-api-key` |

`POST /subscription/start` answers 200 with `meta.gateway_wired: false` and a pending
`provider: "manual"` payment row: the $1/month charge is recorded but nothing is activated,
because the gateway is a later phase.

## Smart queries and credits

The assistant is purpose-bound, not a chatbot. `POST /assistant/query` (and the bot's
`POST /internal/assistant/query`) answers exactly three kinds of message:

- **search** — "warehouse jobs in Hargeisa", "tenders for construction". The rows come from
  `jobs` through the same `live_job_filter()` the feed uses; the model only phrases up to
  three lines about rows Rust passes it, and is told never to invent a job, employer,
  deadline or URL. An empty result set is answered with `Nothing live matches that right
  now.` and no model call at all.
- **help** — how to register, what the bot does, how alerts work, how to change preferences,
  what credits are and how to buy them. Answered from a constant.
- **status** — the caller's own credits, subscription and today's alerts, read from their own
  rows.

Anything else is `intent: "unsupported"` with one fixed redirect sentence and **no charge**.

Units are queries, not tokens. Every supported answer costs 1: the free allowance
(`SUBSCRIPTION_MONTHLY_SMART_QUERIES`, default 30 per 30-day period) is spent first, then the
purchased `balance`. At zero both, the answer is **402** with `code: "out_of_credits"` and
`"You have used your free queries this month. Buy more credits to keep going."` — the bot
turns that code into the Buy Credits action. Twenty queries per user per 10 minutes, then
**429**. A provider failure or timeout is **502** `llm_unavailable` and refunds the charge:
a user is never billed for an answer they did not get.

`GET /credits` → `{balance, monthly_allowance, used_this_period, period_end,
remaining_this_period}`. `POST /credits/topup` is the **manual** path (`x-api-key` only,
no card processing, no prices) until WaafiPay is wired: `{discord_id | user_id, queries}`
adds purchased queries and says `meta.payment_provider: "manual"`. Usage rows land in
`assistant_usage` (`user_id`, `at`, `intent`, token counts, `charged_queries`); the legacy
`token_usage` collection is untouched.

The provider is one hand-rolled `reqwest` JSON call (`LLM_BASE_URL`, `LLM_API_KEY`,
`LLM_MODEL`; the historical `DEEPSEEK_*` names are still read) behind the `LlmClient` trait,
so the whole test suite runs offline with a stub. An empty key is a valid deployment state:
every assistant call answers 502 and charges nothing.

## Data rules that must not regress

- **Ingest upserts on `(external_id, source)` and writes only the fields the payload
  actually carries.** Identity fields (`external_id`, `source`, `title`, `url`,
  `updated_at`) are always written. A full `$set` once blanked `description` and `deadline`
  on a partial scrape run.
- **Unknown category slugs are stored as null with a warning, and the job is still
  persisted.** The taxonomy in `src/categories.rs` is a wire + database + scraper contract.
- **One visibility rule** (`live_job_filter`) is shared by `GET /jobs` and `GET /jobs/match`:
  deadline today or later (Africa/Nairobi), or no deadline and posted within 14 days
  (falling back to `created_at`). It is visibility only — a deadline is never synthesised.
  `GET /jobs/saved` is deliberately not live-filtered.
- **Matching** scores +2 for a profile location, +1 per keyword in the title, +0.5 in the
  description; `record=false` writes nothing, `record=true` queues a notification per match.
  A user with no match profile gets 200 with `meta.reason = "no_match_profile"`.
- **Passwords are salted SHA-256**, not argon2 — the stored hashes are in that format and
  changing it would lock every existing account out.
- **Verification codes** are 6 digits, only the SHA-256 hash is stored, 10-minute expiry
  (30 for reset), 5 attempts, 60-second resend cooldown. `verify-email` and `resend-otp`
  authenticate by Bearer token, never by an email in the body.

## Discord identity and portal links

- **A user's identity is the Discord snowflake**, never their handle. `discord_links` holds
  exactly one row per snowflake (unique index), and the row is created by an upsert on
  `discord_id`, so two simultaneous first messages cannot create two accounts.
- **A Discord account has no email**, so `email` is `""` on those users and the unique index
  on `email` is **partial** (`{email: {$gt: ""}}`, name `email_unique_nonempty`). A plain
  unique index treats every missing email as the same null key and would allow exactly one
  Discord user in the whole database.
- **Only the SHA-256 of a portal token is stored** (`link_tokens._id`), never the raw token.
  The mint response is the only place it exists; it cannot be read back.
- **Exchange is one atomic `find_one_and_update`** filtered on `used_at: null` and
  `expires_at > now`, so two parallel exchanges of the same token cannot both succeed.
  Unknown, used and expired tokens all answer the same 400 and the same message.
- **`link_tokens.expires_at` is a BSON date**, not an RFC3339 string: a TTL index only
  expires Date values. Correctness does not depend on the TTL monitor — the exchange
  filters on the date itself.
- **`kind` is `job` or `tender`**, written by ingest on every upsert (absent or unrecognized
  ⇒ `job`, never a rejected listing). The 55 pre-kind documents were not rewritten; they
  read back as `"kind": "job"` through the serde default, and `?kind=job` matches an absent
  field as well as an explicit one.
- **`match_profile` is derived in Rust from the onboarding answers** — there is no model in
  the match path:
  - `categories` = the union, in answer order and deduped, of the categories each recorded
    degree's **field of study** maps to (`src/onboarding.rs`). `Health → health`,
    `Business administration` and `Finance & accounting → business_and_finance`, `ICT → ict`,
    `Engineering → engineering_and_construction`, `Agriculture → agriculture`, `Law →
    legal_and_regulatory_services`, `Logistics & procurement → logistics_and_supply_chain`,
    `Social science & development → humanitarian_aid_and_emergency_relief`, `WASH → wash`,
    `Nutrition → food_security`, `Education → education`;
  - `keywords` = the words of `last_position` (at most four), the member's own words behind an
    `Other` field, and any skills a web client still sends (20 total);
  - `locations` is **always empty**: Jobify covers all of Somalia, so the interview does not ask
    for a city and the matcher ranks the whole country.
  Levels and fields are validated against fixed lists; anything else is a **400 naming the
  offender** and nothing is written. `education` is `[{level, field}]`, ordered, max 4; a
  non-degree level carries an empty field, and `Other` is stored as `Other: <their words>`
  (which maps to no category — the words only become keywords). `education_level` mirrors the
  highest degree and `sectors` mirrors the derived categories, so neither can drift.
- **`onboarding_complete` = a name AND at least one education entry**, not "the matcher has a
  category": an all-`Other` history is a real answer and still onboarding-complete (it just
  matches by keyword). Completing it switches the daily digest on
  (`alerts.enabled: true`, `mode: daily`) — unless `alerts.user_disabled` is set, which is what
  `/stop` and `/alerts off` write, so a re-run of the interview can never silently re-enable a
  member who said no.

## Dev gates

```bash
cargo fmt --check
cargo clippy --all-targets
cargo build
cargo test
```
