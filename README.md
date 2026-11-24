# jobify-backend

Core jobs-board API for Jobify: Rust + Axum 0.8, MongoDB (`mongodb` v3), JWT auth with
email one-time codes, the public jobs feed, saved jobs, profile matching, alerts,
notifications, and the scraper's internal ingest endpoints.

Deliberately **not** in this service: the CV builder, the LLM chat, credits and payments.
They were removed with the old code; the collections they used are untouched.

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
| POST | `/auth/forgot-password`, `/auth/reset-password` | public |
| GET, PATCH | `/auth/me` | Bearer |
| POST | `/auth/verify-email`, `/auth/resend-otp` | Bearer |
| GET | `/jobs` (`q`, `location`, `category`, `organization`, `page`, `limit`) | public, optional Bearer |
| GET | `/jobs/{id}` | public |
| GET | `/jobs/match` (`limit`, `record`) | Bearer |
| GET | `/jobs/saved` | Bearer |
| POST, DELETE | `/jobs/{id}/save` | Bearer |
| GET, PUT | `/alerts` | Bearer |
| POST | `/jobs/{id}/not-a-fit` (`scope`: employer \| category) | Bearer |
| GET | `/notifications`, `/notifications/unread-count` | Bearer |
| PATCH | `/notifications/read-all`, `/notifications/{id}/read` | Bearer |
| GET | `/internal/sources` | `x-api-key` |
| POST | `/internal/jobs` (JSON array of `JobInput`) | `x-api-key` |
| POST | `/internal/matches/refresh` | `x-api-key` |
| GET | `/internal/deliveries` | `x-api-key` |
| POST | `/internal/deliveries/sent`, `/internal/deliveries/failed` | `x-api-key` |

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

## Dev gates

```bash
cargo fmt --check
cargo clippy --all-targets
cargo build
cargo test
```
