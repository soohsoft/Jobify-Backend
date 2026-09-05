# Jobify Backend

Rust/Axum + MongoDB API for Jobify. This service is the source of truth for jobs, users, resume chats, CV data, credits, payments, and notifications.

The Node/Express code in `jobify-backend-Node` is being replaced by this Rust service. Web scraping stays in a separate Node application for Playwright/stealth reasons.

## Architecture

- **Rust backend (this repo):** auth, job ingestion API, resume-building chat orchestration, CV data storage, credit/token accounting, WaafiPay payments, and notifications.
- **Node scraper (separate server):** runs Playwright/stealth, fetches its target sites from `GET /internal/sources`, scrapes/refines each job, then pushes clean JSON to `POST /internal/jobs`.
- **Frontend:** owns all visual CV templates, template selection UI, PDF export, and rendering. The backend only returns consistent structured profile data.

## Scraper contract

Protected by the `x-api-key` header matching `SCRAPER_INTERNAL_API_KEY`.

- `GET /internal/sources` — returns target job-board source configurations.
- `POST /internal/jobs` — accepts one job object or an array of job objects for upsert.

## Public API

- `GET /health`
- `POST /auth/register`
- `POST /auth/login`
- `GET /jobs`
- `GET /jobs/:id`
- `GET /resumes/templates`
- `POST /payments/waafipay/callback`

Authenticated endpoints require `Authorization: Bearer <jwt>`.

## Credits & pricing

- DeepSeek blended cost baseline is **5,500,000 tokens per $1** at cost.
- Jobify bills **1,100,000 tokens per $1** (80% profit margin on token cost).
- A $1 top-up adds **1,100,000 tokens**.
- Token usage is deducted from the user's token balance after every LLM-backed chat/edit.
- Usage is recorded daily per user for auditing.

## Setup

```sh
cp .env.example .env
# fill in MongoDB, DeepSeek, and WaafiPay values
cargo run
```

## Development

```sh
cargo fmt
cargo clippy --all-targets --all-features
cargo build
```
