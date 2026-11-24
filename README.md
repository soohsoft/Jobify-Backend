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
| GET | `/internal/sources` | `x-api-key` |
| POST | `/internal/jobs` (JSON array of `JobInput`, optional `kind`) | `x-api-key` |
| POST | `/internal/matches/refresh` | `x-api-key` |
| GET | `/internal/deliveries` | `x-api-key` |
| POST | `/internal/deliveries/sent`, `/internal/deliveries/failed` | `x-api-key` |
| POST | `/internal/discord/users` (`discord_id`, `username`, `global_name?`) | `x-api-key` |
| POST | `/internal/discord/profile` (`discord_id` + onboarding answers) | `x-api-key` |
| POST | `/internal/discord/link-token` (`discord_id`, `ttl_seconds?`) | `x-api-key` |
| GET | `/internal/discord/user/{discord_id}` | `x-api-key` |

`POST /subscription/start` answers 200 with `meta.gateway_wired: false` and a pending
`provider: "manual"` payment row: the $1/month charge is recorded but nothing is activated,
because the gateway is a later phase.

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
- **`match_profile` is derived from the bot's onboarding answers** (sectors → canonical
  category slugs, skills → keywords, location → locations) so the existing matcher works
  unchanged. Unknown sectors are dropped; the manual `PUT /me/match-profile` still refuses
  them by name.

## Dev gates

```bash
cargo fmt --check
cargo clippy --all-targets
cargo build
cargo test
```
