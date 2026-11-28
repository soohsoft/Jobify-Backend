use futures_util::TryStreamExt;

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use mongodb::Collection;
use mongodb::bson::{Document, doc};
use mongodb::options::FindOptions;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::country::{self, ALL_COUNTRIES, SOMALIA};
use crate::error::AppError;
use crate::models::{AlertPrefs, JobDoc, JobInput, MatchProfile, NotificationDoc};
use crate::state::AppState;
use crate::util::{escape_regex, nairobi_days_ago, nairobi_today, now_iso, uuid_id};

use super::ApiResult;
use super::common::{paginate, total_pages};

pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list))
        .route("/jobs/{id}", get(get_by_id))
}

pub fn internal_router() -> Router<AppState> {
    Router::new()
        .route("/internal/sources", get(sources))
        .route("/internal/jobs", post(ingest_jobs))
        // The bot has no bearer token, so `GET /jobs/match` is unreachable from Discord.
        // This is the same payload shape addressed by the member's Discord identity, which
        // is what "/jobs for me" means in the product.
        .route("/internal/jobs/for-user/{discord_id}", get(jobs_for_user))
        // Deadline backfill: a second, slower pass whose only job is to visit postings that
        // publish no closing date on their card and read it off the page itself.
        .route("/internal/jobs/missing-deadline", get(missing_deadline))
        .route("/internal/jobs/{id}/deadline", post(set_deadline))
}

pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/jobs/match", get(match_jobs))
        .route("/jobs/saved", get(list_saved))
        .route("/jobs/{id}/save", post(save_job).delete(unsave_job))
}

/// How long a job with no closing date stays visible, counted from `posted_date` and
/// falling back to `created_at` when the board never showed a posting date.
pub const UNDATED_JOB_LIFETIME_DAYS: i64 = 14;

// The single definition of "live" for the feed AND the matcher, so the two can never
// disagree about what a user is allowed to see.
//
// A job is live when:
//   - it has a closing date that has not passed, or
//   - it has no closing date and was posted within 14 days — or, when no posting date is
//     known, was first recorded within 14 days.
//
// This is a VISIBILITY rule and nothing more. It deliberately does not write or derive a
// closing date: an undated job keeps `deadline: null` in every response, so no client can
// render a deadline the board never published. Do not turn `UNDATED_JOB_LIFETIME_DAYS`
// into a synthetic deadline.

/// How many rows the feed reads before collapsing and paging them. A page is at most 100
/// rows, so this is the headroom dedupe needs; `meta.scanned` and `meta.total` show when a
/// caller has run into it.
pub const FEED_SCAN_CAP: i64 = 500;

/// Which collection(s) one read should use.
///
/// This replaces the old `kind` field inside a document: the collection a row lives in IS
/// its kind, so a read chooses a destination instead of adding a query clause. Both
/// collections are read and merged when the caller asks for `both` (or says nothing).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FeedScope {
    Jobs,
    Tenders,
    Both,
}

impl FeedScope {
    /// The `?kind=` vocabulary. Absent, blank, `both` and `all` mean both collections;
    /// anything that is not a job or a tender spelling is a 400, because a typo must not
    /// silently widen a list from one kind to two.
    pub(crate) fn parse(raw: Option<&str>) -> Result<Self, AppError> {
        match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            None | Some("") | Some("both") | Some("all") => Ok(Self::Both),
            Some("job") | Some("jobs") => Ok(Self::Jobs),
            Some("tender") | Some("tenders") => Ok(Self::Tenders),
            Some(other) => Err(AppError::BadRequest(format!(
                "kind must be \"job\" or \"tender\" — got '{other}'"
            ))),
        }
    }

    /// The stored preference (`users.wants`). Applied leniently: it is data written by an
    /// earlier version, so an unrecognised value means both rather than a 400.
    pub(crate) fn from_preference(raw: Option<&str>) -> Self {
        Self::parse(raw).unwrap_or(Self::Both)
    }

    /// The collections this scope reads, each with the label its rows should report.
    pub(crate) fn collections(self, state: &AppState) -> Vec<(&'static str, Collection<JobDoc>)> {
        match self {
            Self::Jobs => vec![("job", state.jobs())],
            Self::Tenders => vec![("tender", state.tenders())],
            Self::Both => vec![("job", state.jobs()), ("tender", state.tenders())],
        }
    }
}

/// Read one document by id from whichever collection holds it. Used by the paths that only
/// have an id in hand — a saved row, a queued notification, "not a fit" — because a client
/// passes the id it was given and must not have to know the kind.
pub(crate) async fn find_any_job(state: &AppState, id: &str) -> Result<Option<JobDoc>, AppError> {
    if let Some(job) = state.jobs().find_one(doc! { "_id": id }).await? {
        return Ok(Some(job));
    }
    Ok(state.tenders().find_one(doc! { "_id": id }).await?)
}

/// The country a request is asking for: `?country=` when given, otherwise the member's own
/// country, otherwise Somalia. `all` is passed through as its own sentinel and turns the
/// filter off.
pub fn resolve_country(
    requested: Option<&str>,
    member_location: Option<&str>,
) -> Result<String, AppError> {
    let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(country::default_for_member(member_location).to_string());
    };
    let requested = requested.to_ascii_lowercase();
    if requested == ALL_COUNTRIES {
        return Ok(requested);
    }
    // A two- or three-letter code, nothing else. A typo must not silently answer "all".
    if requested.len() > 3 || !requested.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(AppError::BadRequest(format!(
            "country must be a short country code or \"{ALL_COUNTRIES}\" — got '{requested}'"
        )));
    }
    Ok(requested)
}

/// The Mongo clause for a country filter. `None` means every country (the `all` escape).
///
/// The Somali default also admits rows with NO stored country: whether such a row belongs
/// is decided from its location text in Rust (`country_accepts`), because "unknown and
/// foreign-looking" cannot be expressed as a clause without re-implementing the mapping as
/// a regex.
pub fn country_query_clause(wanted: &str) -> Option<Document> {
    if wanted == ALL_COUNTRIES {
        return None;
    }
    if wanted == SOMALIA {
        Some(doc! {
            "$or": [
                { "country": SOMALIA },
                { "country": null },
            ]
        })
    } else {
        Some(doc! { "country": wanted })
    }
}

/// The country of one job: the stored field when it has one, otherwise the location text.
/// The fallback makes a document written before `country` existed behave exactly like one
/// ingested today.
fn job_country(job: &JobDoc) -> Option<&str> {
    match job.country.as_deref().map(str::trim) {
        Some(code) if !code.is_empty() => Some(code),
        _ => country::from_location(job.location.as_deref()),
    }
}

/// Whether one job belongs in a list for `wanted`.
///
/// A job with no derivable country is kept only in the Somali default and only when its
/// location says nothing foreign: the conservative side of the error is "do not show a
/// Djibouti job to a Somali member", so an unknown-but-unforeign row ("Multiple Locations")
/// stays, and an unknown row that names another country does not.
pub fn country_accepts(job: &JobDoc, wanted: &str) -> bool {
    if wanted == ALL_COUNTRIES {
        return true;
    }
    match job_country(job) {
        Some(code) => code.eq_ignore_ascii_case(wanted),
        None => wanted == SOMALIA && !country::is_foreign_looking(job.location.as_deref()),
    }
}

// ---------------------------------------------------------------------------
// De-duplication: the same vacancy carried by two boards
// ---------------------------------------------------------------------------
//
// Two sources routinely carry one posting — somalijobs and unjobs both list "Chief of
// Section, Human Resources Management" — and a member sees the same job twice. There is no
// shared id to join on, so the match is on the only thing both boards agree about: the
// title, the employer, and the place. Anything less certain is kept, because hiding a real
// job is worse than showing one twice.

/// Lowercased alphanumeric words joined by single spaces. Punctuation is dropped, so
/// "Chief of Section, Human Resources Management, P5" and "Chief Of Section Human Resources
/// Management P5" are the same key. Apostrophes are DELETED rather than turned into a space,
/// because the two boards spell the same employer "Children's" and "CHILDRENS" — splitting
/// there would make one employer look like two ("children s" versus "childrens").
pub(crate) fn normalised(value: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    for character in value.chars() {
        if character.is_alphanumeric() {
            for lower in character.to_lowercase() {
                current.push(lower);
            }
        } else if character == '\'' || character == '\u{2019}' {
            // Delete the apostrophe, keep the word joined.
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words.join(" ")
}

/// `shorter` is the opening words of `longer` — "UNICEF" against "UNICEF United Nations
/// Childrens Fund". Four characters minimum and a word boundary, so a two-letter stub cannot
/// prefix-match a different employer.
fn is_word_prefix(shorter: &str, longer: &str) -> bool {
    shorter.chars().count() >= 4
        && longer.starts_with(shorter)
        && longer[shorter.len()..].starts_with(' ')
}

fn at_most_one_edit(left: &str, right: &str) -> bool {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len().abs_diff(right.len()) > 1 {
        return false;
    }
    let (longer, shorter) = if left.len() >= right.len() {
        (&left, &right)
    } else {
        (&right, &left)
    };
    let mut i = 0;
    let mut j = 0;
    let mut edits = 0;
    while i < longer.len() && j < shorter.len() {
        if longer[i] == shorter[j] {
            i += 1;
            j += 1;
            continue;
        }
        edits += 1;
        if edits > 1 {
            return false;
        }
        if longer.len() == shorter.len() {
            i += 1;
            j += 1;
        } else {
            i += 1;
        }
    }
    edits + (longer.len() - i) + (shorter.len() - j) <= 1
}

/// Two employers are the same when their words are the same (in any order) or one is the
/// opening words of the other. The boards carry "UNICEF" and "UNICEF - United Nations
/// Children's Fund" for one vacancy, and "ICRC (International Committee of the Red Cross)"
/// and "International Committee of the Red Cross (ICRC)" for another.
fn organisations_related(left: &str, right: &str) -> bool {
    let left_key = organisation_key(left);
    let right_key = organisation_key(right);
    if left_key.is_empty() || right_key.is_empty() {
        return left_key == right_key;
    }
    if left_key == right_key {
        return true;
    }
    let left_text = normalised(left);
    let right_text = normalised(right);
    is_word_prefix(&left_text, &right_text) || is_word_prefix(&right_text, &left_text)
}

/// The employer as an order-insensitive key. The boards disagree about where the acronym
/// goes — "ICRC (International Committee of the Red Cross)" and "International Committee of
/// the Red Cross (ICRC)" are one employer — so the words are sorted before comparing.
fn organisation_key(value: &str) -> String {
    let normalised = normalised(value);
    let mut words: Vec<&str> = normalised.split(' ').collect();
    words.sort_unstable();
    words.join(" ")
}

/// Two locations name the same place when one is a prefix of the other: "Hargeisa" and
/// "Hargeisa, Somaliland" are one place, while "Garowe" and "Baidoa" are not. A missing
/// location only matches another missing one, because a shared title and employer in two
/// different cities can genuinely be two postings.
fn locations_related(left: Option<&str>, right: Option<&str>) -> bool {
    let left = left.map(normalised).unwrap_or_default();
    let right = right.map(normalised).unwrap_or_default();
    match (left.is_empty(), right.is_empty()) {
        (true, true) => true,
        (true, false) | (false, true) => false,
        (false, false) => {
            if left == right {
                return true;
            }
            // "Mogadishu" opens "Mogadishu, Somalia", and is also contained in "Heliwa,
            // Mogadishu, Somalia" — both are the same place.
            let shorter_opens = is_word_prefix(&left, &right) || is_word_prefix(&right, &left);
            let shorter_inside = format!(" {right} ").contains(&format!(" {left} "))
                || format!(" {left} ").contains(&format!(" {right} "));
            if shorter_opens || shorter_inside {
                return true;
            }
            // A one-letter spelling difference in the TOWN is the same place:
            // Garowe/Garoowe, Dhobley/Dhoobley, Hargeisa/Hargeysa — including when one
            // board appends the country ("Garoowe, Somalia"). Two different towns
            // ("Garowe"/"Baidoa") are three edits apart, so this cannot merge them.
            let left_town = left.split(' ').next().unwrap_or("");
            let right_town = right.split(' ').next().unwrap_or("");
            left_town.chars().count() >= 5
                && right_town.chars().count() >= 5
                && at_most_one_edit(left_town, right_town)
        }
    }
}

/// Which source wins when two rows are the same vacancy and everything else is equal.
/// Somalijobs is the primary Somalia board, so it holds the canonical URL and description.
fn source_rank(source: &str) -> u8 {
    match source.trim().to_ascii_lowercase().as_str() {
        "somalijobs" => 0,
        "qaranjobs" => 1,
        "reliefweb" => 2,
        "unjobs" => 3,
        _ => 4,
    }
}

pub(crate) fn same_vacancy(left: &JobDoc, right: &JobDoc) -> bool {
    let title = normalised(&left.title);
    if title.is_empty() || title != normalised(&right.title) {
        return false;
    }
    if !organisations_related(&left.organization, &right.organization) {
        return false;
    }
    locations_related(left.location.as_deref(), right.location.as_deref())
}

/// Collapse duplicate vacancies, keeping one row per vacancy.
///
/// The winner is the best row for the member, not the first one read: highest `matchScore`,
/// then the newest `created_at`, then the source priority (somalijobs > qaranjobs >
/// reliefweb > unjobs), then the id so the choice is deterministic. The returned count is
/// how many rows were collapsed, which every list reports as `meta.duplicatesRemoved`.
pub(crate) fn dedupe_rows(rows: Vec<(i64, JobDoc)>) -> (Vec<(i64, JobDoc)>, usize) {
    let mut ordered = rows;
    ordered.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.created_at.cmp(&left.1.created_at))
            .then_with(|| source_rank(&left.1.source).cmp(&source_rank(&right.1.source)))
            .then_with(|| right.1.id.cmp(&left.1.id))
    });

    let mut kept: Vec<(i64, JobDoc)> = Vec::with_capacity(ordered.len());
    let mut collapsed = 0usize;
    for candidate in ordered {
        if kept
            .iter()
            .any(|(_, winner)| same_vacancy(winner, &candidate.1))
        {
            collapsed += 1;
            continue;
        }
        kept.push(candidate);
    }
    (kept, collapsed)
}

/// The visibility rule: a job is visible while its closing date has not passed, or while it
/// is recent enough to be plausibly open (see the note above).
pub fn live_job_filter() -> Document {
    let today = nairobi_today();
    let cutoff = nairobi_days_ago(UNDATED_JOB_LIFETIME_DAYS);
    doc! {
        "$or": [
            doc! { "deadline": { "$gte": &today } },
            // `null` matches both an explicit null and a missing field.
            doc! { "deadline": null, "$or": [ doc! { "posted_date": { "$gte": &cutoff } }, doc! { "posted_date": null, "created_at": { "$gte": &cutoff } } ] },
            doc! { "deadline": "", "$or": [ doc! { "posted_date": { "$gte": &cutoff } }, doc! { "posted_date": null, "created_at": { "$gte": &cutoff } } ] },
        ]
    }
}

#[derive(Deserialize)]
pub struct JobsQuery {
    /// `q` is the documented search parameter; `search` is accepted as the same thing so
    /// an older client keeps working.
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    organization: Option<String>,
    #[serde(default)]
    category: Option<String>,
    /// `so` (the default), another short country code, or `all`. Absent means the member's
    /// own country: their stored location when it maps to one, Somalia otherwise.
    #[serde(default)]
    country: Option<String>,
    /// `job` or `tender`. Absent means both — which is what the 55 listings stored before
    /// `kind` existed need, since they carry no field at all.
    #[serde(default)]
    kind: Option<String>,
    #[serde(default = "default_page")]
    page: u64,
    #[serde(default = "default_limit")]
    limit: u64,
}

fn default_page() -> u64 {
    1
}

fn default_limit() -> u64 {
    20
}

async fn list(
    State(state): State<AppState>,
    extension: Option<Extension<AuthUser>>,
    Query(query): Query<JobsQuery>,
) -> ApiResult {
    // One read for the match profile the feed scores against. A public caller gets none.
    //
    // The stored `users.location` is deliberately NOT used as a fallback filter. Onboarding
    // no longer asks for one (the product is Somalia-wide), but legacy values survive a
    // read — "Kismayo, Mogadishu" is one substring and matches no single stored location,
    // so falling back to it emptied a signed-in member's whole feed and looked like "no
    // jobs". Only an explicit `?location=` narrows the feed, and existing user data is
    // never rewritten.
    let viewer = match extension {
        Some(Extension(user)) => state.users().find_one(doc! { "_id": &user.id }).await?,
        None => None,
    };

    // The country every row must be in. An absent `?country=` means the member's own
    // country — their stored location when it maps to one — and Somalia when there is no
    // account at all. Browsing is never "the whole world": the product is Somalia-wide.
    let country_wanted = resolve_country(
        query.country.as_deref(),
        viewer.as_ref().and_then(|doc| doc.location.as_deref()),
    )?;

    let location = query
        .location
        .clone()
        .filter(|value| !value.trim().is_empty());

    let mut filter = doc! {};
    if let Some(location) = location {
        filter.insert(
            "location",
            doc! { "$regex": escape_regex(&location), "$options": "i" },
        );
    }
    if let Some(organization) = query.organization.filter(|s| !s.trim().is_empty()) {
        filter.insert(
            "organization",
            doc! { "$regex": escape_regex(&organization), "$options": "i" },
        );
    }
    if let Some(category) = query.category.filter(|s| !s.trim().is_empty()) {
        filter.insert(
            "category",
            doc! { "$regex": escape_regex(&category), "$options": "i" },
        );
    }
    if let Some(search) = query.q.or(query.search).filter(|s| !s.trim().is_empty()) {
        let pattern = doc! { "$regex": escape_regex(&search), "$options": "i" };
        filter.insert(
            "$or",
            vec![
                doc! { "title": pattern.clone() },
                doc! { "organization": pattern.clone() },
                doc! { "description": pattern },
            ],
        );
    }

    // `kind` now selects a COLLECTION rather than adding a clause: a document no longer
    // carries the field, so there is nothing to filter on. Absent/`both` reads both.
    let scope = FeedScope::parse(query.kind.as_deref())?;

    let mut clauses = Vec::new();
    if !filter.is_empty() {
        clauses.push(filter);
    }
    clauses.push(live_job_filter());
    if let Some(country) = country_query_clause(&country_wanted) {
        clauses.push(country);
    }
    let filter = if clauses.len() == 1 {
        clauses.remove(0)
    } else {
        doc! { "$and": clauses }
    };

    let page = query.page.max(1);
    let limit = query.limit.clamp(1, 100);
    let profile = viewer
        .and_then(|doc| doc.match_profile)
        .filter(|profile| !profile.is_blank());

    // De-duplication has to happen BEFORE paging, or page 1 and page 2 can each carry half
    // of the same vacancy and the member still sees it twice. The feed therefore reads one
    // bounded window, collapses it, and pages the collapsed rows in memory. The window is
    // far larger than any page and than anything the boards currently hold; `meta.scanned`
    // reports how much of it was read so a truncated window is visible rather than silent.
    let options = FindOptions::builder()
        // Secondary sort on _id keeps pagination stable when many jobs share the same
        // created_at second (batch ingest), instead of repeating or skipping rows.
        .sort(doc! { "created_at": -1, "_id": -1 })
        .limit(FEED_SCAN_CAP)
        .build();

    // Read a bounded window from EACH selected collection, then merge and sort in memory.
    // Reading `limit + offset` per collection and concatenating would let one collection's
    // slice decide the merged order (and would drop rows the other collection should have
    // contributed); the scan window is larger than any page and `meta.scanned` says so.
    let mut scanned = 0usize;
    let mut rows: Vec<(i64, JobDoc)> = Vec::new();
    for (_label, collection) in scope.collections(&state) {
        let mut cursor = collection
            .find(filter.clone())
            .with_options(options.clone())
            .await?;
        while let Some(job) = cursor.try_next().await? {
            // The country gate is re-applied here because a row whose `country` was never
            // derived is decided from its location text, which a Mongo clause cannot do.
            if !country_accepts(&job, &country_wanted) {
                continue;
            }
            scanned += 1;
            let score = profile
                .as_ref()
                .map(|profile| score_job(&job, profile))
                .unwrap_or(0);
            rows.push((score, job));
        }
    }

    let (mut rows, duplicates_removed) = dedupe_rows(rows);
    rows.sort_by(|a, b| {
        b.1.created_at
            .cmp(&a.1.created_at)
            .then_with(|| b.1.id.cmp(&a.1.id))
    });
    let total = rows.len() as u64;
    let skip = ((page - 1) * limit) as usize;
    let window: Vec<(i64, JobDoc)> = rows.into_iter().skip(skip).take(limit as usize).collect();

    // The feed keeps its shape — the same job objects and the same `meta` — and adds ONE
    // field per row for a signed-in member with a profile: `matchScore`, the same 0–100 the
    // dedicated list returns, from the same `score_job`. Browsing is not the matcher, so the
    // feed is deliberately NOT cut off at the threshold: a member searching for a category
    // still sees every live listing in it, but can no longer mistake a 30% row for a 75% one.
    let scoring = profile.is_some();
    let data: Vec<Value> = window
        .into_iter()
        .map(|(score, job)| {
            let mut value = json!(&job);
            if scoring && let Some(map) = value.as_object_mut() {
                map.insert("matchScore".to_string(), json!(score));
            }
            value
        })
        .collect();

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": data,
            "meta": {
                "page": page,
                "limit": limit,
                "total": total,
                "totalPages": total_pages(total, limit),
                "threshold": state.config.match_threshold,
                "country": country_wanted,
                // How many rows this list collapsed as the same vacancy, and how much of
                // the corpus was read to do it.
                "duplicatesRemoved": duplicates_removed,
                "scanned": scanned
            }
        })),
    ))
}

#[derive(Deserialize, Default)]
pub struct JobQuery {
    /// `job`/`tender` (or the plurals) narrows the lookup to one collection. Absent means
    /// "either": the id is looked up in `jobs` first, then `tenders`, because a client
    /// holding an id should not have to remember which list it came from.
    #[serde(default)]
    kind: Option<String>,
}

async fn get_by_id(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<JobQuery>,
) -> ApiResult {
    let scope = FeedScope::parse(query.kind.as_deref())?;

    let mut found = None;
    for (_label, collection) in scope.collections(&state) {
        if let Some(job) = collection.find_one(doc! { "_id": &id }).await? {
            found = Some(job);
            break;
        }
    }
    let job = found.ok_or_else(|| AppError::NotFound("Job not found".to_string()))?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": job })),
    ))
}

#[derive(Deserialize)]
pub struct SavedQuery {
    #[serde(default = "default_page")]
    page: u64,
    #[serde(default = "default_saved_limit")]
    limit: u64,
}

fn default_saved_limit() -> u64 {
    30
}

/// The user's saved jobs, newest save first.
///
/// Deliberately NOT filtered by the live-job rule. That rule exists so nobody is shown a
/// job that closed weeks ago while browsing; a job the user chose to keep is theirs, and
/// dropping it from their own list is silent data loss. Each job carries its own `deadline`
/// untouched, so a client can mark one that has closed.
async fn list_saved(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<SavedQuery>,
) -> ApiResult {
    let page = query.page.max(1);
    let limit = query.limit.clamp(1, 100);
    let (rows, total) = paginate(
        &state.saved_jobs(),
        doc! { "user_id": &user.id },
        page,
        limit,
        doc! { "created_at": -1, "_id": -1 },
    )
    .await?;

    let ids: Vec<String> = rows.iter().map(|row| row.job_id.clone()).collect();
    let mut by_id: HashMap<String, JobDoc> = HashMap::new();
    if !ids.is_empty() {
        // A saved row can point at either collection: the save route accepted any id it
        // could find, so the read must look in both or a saved tender silently disappears.
        for collection in [state.jobs(), state.tenders()] {
            let mut cursor = collection.find(doc! { "_id": { "$in": &ids } }).await?;
            while let Some(job) = cursor.try_next().await? {
                by_id.insert(job.id.clone(), job);
            }
        }
    }

    // Walk the save order, not the job order: `$in` returns jobs in index order, which
    // would quietly undo the newest-first sort the page just made.
    let data: Vec<Value> = rows
        .iter()
        .filter_map(|row| {
            by_id
                .remove(&row.job_id)
                .map(|job| json!({ "job": job, "savedAt": row.created_at }))
        })
        .collect();

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": data,
            "meta": {
                "page": page,
                "limit": limit,
                "total": total,
                "totalPages": total_pages(total, limit)
            }
        })),
    ))
}

/// Save a job. Idempotent: saving twice is one save, and the response says which of the
/// two happened so a client can report it honestly.
async fn save_job(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> ApiResult {
    // A save of something that does not exist is a client bug, not an empty list to be
    // discovered later from a saved row pointing at nothing. Either collection counts.
    if find_any_job(&state, &id).await?.is_none() {
        return Err(AppError::NotFound("Job not found".to_string()));
    }

    let result = state
        .saved_jobs()
        .update_one(
            doc! { "user_id": &user.id, "job_id": &id },
            doc! { "$setOnInsert": {
                "_id": uuid_id(),
                "user_id": &user.id,
                "job_id": &id,
                "created_at": now_iso(),
            } },
        )
        .upsert(true)
        .await?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": { "saved": true, "alreadySaved": result.upserted_id.is_none(), "job_id": id }
        })),
    ))
}

/// Remove a saved job. Also idempotent: removing something already gone succeeds, because
/// the caller's intent ("this should not be saved") is satisfied either way.
async fn unsave_job(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> ApiResult {
    let result = state
        .saved_jobs()
        .delete_one(doc! { "user_id": &user.id, "job_id": &id })
        .await?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": { "saved": false, "removed": result.deleted_count > 0, "job_id": id }
        })),
    ))
}

async fn sources(State(state): State<AppState>) -> ApiResult {
    let mut cursor = state.sources().find(doc! {}).await?;
    let mut data = Vec::new();
    while let Some(source) = cursor.try_next().await? {
        data.push(source);
    }

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": data })),
    ))
}

#[derive(Deserialize)]
struct MissingDeadlineQuery {
    limit: Option<i64>,
    /// Rows checked more recently than this are skipped, so a backfill makes progress
    /// through the backlog instead of re-visiting the same undated postings every run.
    recheck_after_days: Option<i64>,
}

/// `GET /internal/jobs/missing-deadline` — the postings a deadline backfill should revisit:
/// no closing date stored, and either never checked or last checked longer ago than
/// `recheck_after_days`.
async fn missing_deadline(
    State(state): State<AppState>,
    Query(query): Query<MissingDeadlineQuery>,
) -> ApiResult {
    let limit = query.limit.unwrap_or(40).clamp(1, 200);
    let recheck = query.recheck_after_days.unwrap_or(7).max(0);
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(recheck))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let filter = doc! {
        "deadline": null,
        "$or": [
            { "deadline_checked_at": { "$exists": false } },
            { "deadline_checked_at": null },
            { "deadline_checked_at": { "$lt": &cutoff } },
        ],
    };

    let mut rows: Vec<Value> = Vec::new();
    for (label, collection) in FeedScope::Both.collections(&state) {
        let mut cursor = collection
            .find(filter.clone())
            .sort(doc! { "created_at": -1 })
            .limit(limit)
            .await?;
        while let Some(job) = cursor.try_next().await? {
            rows.push(json!({
                "id": job.id,
                "url": job.url,
                "source": job.source,
                "source_name": job.source_name,
                "title": job.title,
                "kind": label,
            }));
        }
        if rows.len() as i64 >= limit {
            break;
        }
    }
    rows.truncate(limit as usize);

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": rows })),
    ))
}

#[derive(Deserialize)]
struct DeadlineUpdate {
    /// `YYYY-MM-DD`, or null to record "visited, the page publishes no closing date".
    deadline: Option<String>,
}

fn valid_deadline_date(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let parsed = chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").ok()?;
    let year = chrono::Datelike::year(&parsed);
    if !(2020..=2100).contains(&year) {
        return None;
    }
    Some(trimmed.to_string())
}

/// `POST /internal/jobs/{id}/deadline` — the backfill's write. Sets the closing date when it
/// found one, and always stamps `deadline_checked_at`, so a posting whose page genuinely
/// publishes no date is not visited again on every run.
async fn set_deadline(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<DeadlineUpdate>,
) -> ApiResult {
    let deadline = match body.deadline.as_deref().map(str::trim) {
        Some(raw) if !raw.is_empty() => Some(
            valid_deadline_date(raw)
                .ok_or_else(|| AppError::BadRequest("deadline must be YYYY-MM-DD".into()))?,
        ),
        _ => None,
    };

    let now = now_iso();
    let mut set = doc! { "deadline_checked_at": &now, "updated_at": &now };
    if let Some(value) = &deadline {
        set.insert("deadline", value);
    }
    let update = doc! { "$set": set };

    for (_label, collection) in FeedScope::Both.collections(&state) {
        let result = collection
            .update_one(doc! { "_id": &id }, update.clone())
            .await?;
        if result.matched_count > 0 {
            return Ok((
                StatusCode::OK,
                Json(json!({
                    "status": "success",
                    "data": { "id": id, "deadline": deadline, "checked_at": now },
                })),
            ));
        }
    }

    Err(AppError::NotFound(format!("job '{id}' not found")))
}

#[derive(Deserialize)]
#[serde(untagged)]
enum IngestPayload {
    Single(Box<JobInput>),
    Many(Vec<JobInput>),
}

async fn ingest_jobs(
    State(state): State<AppState>,
    Json(payload): Json<IngestPayload>,
) -> ApiResult {
    let jobs = match payload {
        IngestPayload::Single(job) => vec![*job],
        IngestPayload::Many(jobs) => jobs,
    };

    if jobs.is_empty() {
        return Err(AppError::BadRequest("No jobs provided".to_string()));
    }

    let received = jobs.len() as u64;
    let mut persisted = 0u64;
    let mut created = 0u64;
    let mut updated = 0u64;
    let mut failed = 0u64;

    for input in jobs {
        if input.external_id.trim().is_empty() || input.source.trim().is_empty() {
            failed += 1;
            continue;
        }

        // A category the taxonomy doesn't know is dropped, not guessed at. The job itself
        // is still good, so this is a warning rather than a failure.
        if let Some(raw) = input
            .category
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && canonical_category(&input.category).is_none()
        {
            tracing::warn!(
                source = %input.source,
                external_id = %input.external_id,
                category = %raw,
                "ingest: unrecognized category slug, storing null"
            );
        }

        let now = now_iso();
        let set_on_insert = doc! { "_id": uuid_id(), "created_at": &now };

        // Only overwrite fields the payload actually carries. The scraper is not guaranteed
        // to send everything every cycle — enrichment can be disabled, a detail page can
        // fail, a board can stop exposing a field — and a full `$set` would blank whatever
        // an earlier, richer run had stored. Keys the payload always provides (identity,
        // title, url) stay unconditional; the tradeoff is that an empty value can no longer
        // clear a stored one.
        let mut set = Document::new();
        set.insert("external_id", &input.external_id);
        set.insert("source", &input.source);
        set.insert("title", &input.title);
        set.insert("url", &input.url);
        set.insert("updated_at", &now);

        if !input.source_name.trim().is_empty() {
            set.insert("source_name", &input.source_name);
        }
        if !input.organization.trim().is_empty() {
            set.insert("organization", &input.organization);
        }
        if !input.description.trim().is_empty() {
            set.insert("description", &input.description);
        }
        if let Some(value) = input.location.as_deref().filter(|s| !s.trim().is_empty()) {
            set.insert("location", value);
            // The country is derived from the SAME text, in the same write, so a row can
            // never carry a location and a contradicting country. `null` when the text
            // names no known place: an unrecognised location is not guessed at.
            set.insert("country", country::from_location(Some(value)));
        }
        if let Some(value) = input
            .posted_date
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            set.insert("posted_date", value);
        }
        if let Some(value) = input.deadline.as_deref().filter(|s| !s.trim().is_empty()) {
            set.insert("deadline", value);
        }
        if let Some(value) = input.image_url.as_deref().filter(|s| !s.trim().is_empty()) {
            set.insert("image_url", value);
        }
        if let Some(value) = input
            .organization_image_url
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            set.insert("organization_image_url", value);
        }
        // An absent, blank, or unrecognized slug is skipped rather than written, so a junk
        // value cannot blank a category a previous run got right.
        if let Some(canonical) = canonical_category(&input.category) {
            set.insert("category", canonical);
        }
        if let Some(value) = input
            .employment_type
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            set.insert("employment_type", value);
        }
        if let Some(value) = input.salary.as_deref().filter(|s| !s.trim().is_empty()) {
            set.insert("salary", value);
        }
        if !input.requirements.is_empty() {
            set.insert("requirements", &input.requirements);
        }
        if !input.qualifications.is_empty() {
            set.insert("qualifications", &input.qualifications);
        }

        // The kind is a DESTINATION, not a stored field. A tender is upserted into its own
        // collection; anything else is a job. The upsert key (external_id, source) is the
        // same in both, so re-ingesting one listing stays idempotent wherever it lives.
        let scope = canonical_kind(&input.kind);
        let collection = match scope {
            FeedScope::Tenders => state.tenders(),
            _ => state.jobs(),
        };
        // A listing has exactly ONE home. Re-ingesting the same (external_id, source) with a
        // different kind would otherwise leave a copy behind in the other collection — which is
        // how tenders ended up in the job feed — so the row is removed from the collection it
        // no longer belongs to, in the same pass that stores it here.
        let other = match scope {
            FeedScope::Tenders => state.jobs(),
            _ => state.tenders(),
        };

        // `kind` is removed rather than merely not written: a row ingested before the split
        // still carries it, and re-ingesting is the cheapest moment to drop the stale field.
        let update = doc! {
            "$set": set,
            "$setOnInsert": set_on_insert,
            "$unset": { "kind": "" },
        };

        match collection
            .update_one(
                doc! { "external_id": &input.external_id, "source": &input.source },
                update,
            )
            .upsert(true)
            .await
        {
            Ok(result) => {
                let _ = other
                    .delete_many(doc! {
                        "external_id": &input.external_id,
                        "source": &input.source,
                    })
                    .await;
                // `upserted_id` distinguishes a listing the feed has never carried from one
                // that was already stored and merely had its fields refreshed — the two are
                // different facts and the scraper's log should not blur them.
                if result.upserted_id.is_some() {
                    created += 1;
                } else {
                    updated += 1;
                }
                persisted += 1;
            }
            Err(err) => {
                tracing::warn!(
                    source = %input.source,
                    external_id = %input.external_id,
                    error = %err,
                    "ingest: job could not be persisted"
                );
                failed += 1;
            }
        }
    }

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "received": received,
                "persisted": persisted,
                "created": created,
                "updated": updated,
                "failed": failed
            }
        })),
    ))
}

/// Only canonical taxonomy slugs are stored (`src/categories.rs`). Anything else — a
/// board's free text, a mis-cased or hallucinated slug — becomes null instead of polluting
/// `GET /jobs?category=`.
fn canonical_category(raw: &Option<String>) -> Option<&'static str> {
    let slug = raw.as_deref()?.trim();
    crate::categories::Category::from_slug(slug).map(|category| category.slug())
}

/// The collection an ingest payload is routed to. "tender" is the only alternative to a
/// job; anything else — absent, blank, mis-cased or unknown — is a job, so the label is
/// never a reason to drop a listing, and a tender that stops claiming to be one becomes a
/// job rather than keeping a stale type.
fn canonical_kind(raw: &Option<String>) -> FeedScope {
    match FeedScope::parse(raw.as_deref()) {
        Ok(FeedScope::Tenders) => FeedScope::Tenders,
        _ => FeedScope::Jobs,
    }
}

#[derive(Deserialize)]
pub struct MatchQuery {
    #[serde(default = "default_match_limit")]
    limit: u64,
    /// When true, also queue notifications for what is returned (the automated path). The
    /// manual path leaves it false, so asking twice for "my jobs" never consumes alert state.
    #[serde(default)]
    record: bool,
    /// `so` (the default), another short code, or `all`. Absent means the member's own
    /// country; the same rule as `GET /jobs`.
    #[serde(default)]
    country: Option<String>,
}

fn default_match_limit() -> u64 {
    5
}

// ---------------------------------------------------------------------------
// Match scoring: one normalised 0–100 percentage
// ---------------------------------------------------------------------------
//
// The old score was an additive count (category 1, keyword 1 or 0.5) with no
// ceiling and no meaning: it ordered rows but could not answer "is this good enough to show
// someone?", so an ICT member saw "Prequalified Enumerator" next to a real IT role.
//
// The percentage is built outward from the category gate:
//
//   gate           `job.category` must be one of the member's categories. A foreign category
//                  scores 0 and is never returned — the query already filters on it, and
//                  this keeps a hand-built document from arriving with a non-zero score.
//   base      20   Any admitted row starts at 20. Being in a decided field is real evidence
//                  but it is not a job offer, so 20 alone is far under the cut-off.
//   category  10   The stored slug is one of the member's slugs verbatim.
//   keyword   15   per keyword in the title, capped at 30 (two hits): a loose word is the
//                  only signal inside an admitted category, so it is worth what it is worth.
//   keyword    5   per keyword in the DESCRIPTION only, capped at 10, so a description can
//                  never carry a row over the line by itself.
//
//   20 + 10            = 30   a bare category match     → below the 70 cut-off
//   20 + 10 + 30       = 60   two title keywords
//   20 + 10 + 30 + 10  = 70   the top of the scale: title and description keywords saturated
//   and the sum is capped at 100, so nothing can exceed the scale.
//
// ONE function for every route: `GET /jobs`, `GET /jobs/match`, the bot's for-user list and
// the alert refresh all score through `score_job`, so no two surfaces can disagree.

/// Default cut-off, overridden by `MATCH_THRESHOLD`.
///
/// 60 with the weights below means: right field (20 + 25) plus ONE piece of real evidence —
/// a title keyword (15), or a picked sector (10) with a description keyword (5). Two title
/// keywords clear it comfortably. What does not clear it: a posting that merely sits in the
/// degree's category with nothing about the job itself, which is the row that used to be
/// listed next to a genuine match.
pub const MATCH_THRESHOLD_DEFAULT: i64 = 60;

const SCORE_BASE: i64 = 20;
/// The member's education, expressed as the category the degree maps to. This is the base of
/// a match: it is knowledge, not a preference.
const SCORE_CATEGORY_EXACT: i64 = 25;
/// A nudge, not a filter: the member's chosen employer type when the organisation name looks
/// like it. See `employer_type_matches`.
const SCORE_EMPLOYER_TYPE: i64 = 5;
/// The member picked this sector in "which sectors do you want to be notified about?".
///
/// That answer is a NOTIFICATION preference, not evidence about the job, so it is a nudge
/// worth 10 — never a gate and never enough on its own (a bare row in a picked sector scores
/// 20 + 25 + 10 = 55, below the cut-off). What decides a match is the member's education
/// (the category) and the positions they have actually worked (the keywords).
const SCORE_POSITION_EXACT: i64 = 10;
const SCORE_KEYWORD_TITLE: i64 = 15;
const SCORE_KEYWORD_TITLE_MAX: i64 = 30;
const SCORE_KEYWORD_DESCRIPTION: i64 = 5;
const SCORE_KEYWORD_DESCRIPTION_MAX: i64 = 10;
const SCORE_MAX: i64 = 100;

/// A job's fit for one profile, as a 0–100 percentage.
///
/// Returns 0 — never a partial score — when the category is not one the member was decided
/// into, so "outside the gate" is distinguishable from "inside it but weak".
pub fn score_job(job: &JobDoc, profile: &MatchProfile) -> i64 {
    let Some(category) = job.category.as_deref() else {
        return 0;
    };
    let gate = profile.effective_categories();
    if !gate
        .iter()
        .any(|wanted| wanted.eq_ignore_ascii_case(category))
    {
        return 0;
    }

    let mut score = SCORE_BASE + SCORE_CATEGORY_EXACT;

    let title = job.title.to_lowercase();
    let description = job.description.to_lowercase();

    let mut in_title = 0;
    let mut in_description = 0;
    for keyword in &profile.keywords {
        let keyword = keyword.trim().to_lowercase();
        if keyword.is_empty() {
            continue;
        }
        if title.contains(&keyword) {
            in_title = (in_title + SCORE_KEYWORD_TITLE).min(SCORE_KEYWORD_TITLE_MAX);
        } else if description.contains(&keyword) {
            in_description =
                (in_description + SCORE_KEYWORD_DESCRIPTION).min(SCORE_KEYWORD_DESCRIPTION_MAX);
        }
    }
    score += in_title + in_description;
    if profile
        .positions
        .iter()
        .any(|position| position.eq_ignore_ascii_case(category))
    {
        score += SCORE_POSITION_EXACT;
    }
    if employer_type_matches(&job.organization, &profile.employer_types) {
        score += SCORE_EMPLOYER_TYPE;
    }

    score.clamp(0, SCORE_MAX)
}

/// True when the row's organisation looks like one of the employer types the member chose.
///
/// A job document carries no employer-type field, so this reads the organisation name. It is a
/// NUDGE worth 5 points, never a filter: an unrecognised name costs nothing, and a wrong guess
/// can only lift a row that already cleared the category gate.
fn employer_type_matches(organization: &str, wanted: &[String]) -> bool {
    if wanted.is_empty() {
        return false;
    }
    let kind = employer_type_of(organization);
    wanted
        .iter()
        .any(|choice| choice.eq_ignore_ascii_case(kind))
}

/// "government" / "ngo" / "private" from a name, or "" when nothing matches.
fn employer_type_of(organization: &str) -> &'static str {
    const GOVERNMENT: &[&str] = &[
        "ministry",
        "federal",
        "government",
        "authority",
        "municipal",
        "city council",
        "district",
        "state house",
        "parliament",
        "bureau",
        "administration of",
    ];
    const NGO: &[&str] = &[
        "united nations",
        " un ",
        "unicef",
        "unhcr",
        "wfp",
        "who ",
        "fao",
        "undp",
        "unfpa",
        "unops",
        "iom",
        "ocha",
        "unido",
        "un women",
        "world bank",
        "african union",
        "eu ",
        "european union",
        "nrc",
        "drc",
        "irc",
        "oxfam",
        "save the children",
        "care ",
        "mercy corps",
        "msf",
        "medecins",
        "concern",
        "action against hunger",
        "goal",
        "world vision",
        "caritas",
        "red cross",
        "red crescent",
        "foundation",
        "relief",
        "humanitarian",
        "ngo",
        "consortium",
    ];
    const PRIVATE: &[&str] = &[
        "ltd",
        "limited",
        " inc",
        " llc",
        " plc",
        "company",
        "co.",
        "group",
        "holdings",
        "bank",
        "telecom",
        "hotel",
        "university",
        "college",
        "school",
        "hospital",
        "airlines",
        "logistics",
        "construction",
        "trading",
        "enterprise",
        "industries",
    ];
    let name = format!(" {} ", organization.to_lowercase());
    if GOVERNMENT.iter().any(|needle| name.contains(needle)) {
        return "government";
    }
    if NGO.iter().any(|needle| name.contains(needle)) {
        return "ngo";
    }
    if PRIVATE.iter().any(|needle| name.contains(needle)) {
        return "private";
    }
    ""
}

async fn match_jobs(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<MatchQuery>,
) -> ApiResult {
    let limit = query.limit.clamp(1, 20);

    let user_doc = state.users().find_one(doc! { "_id": &user.id }).await?;
    let profile = user_doc.as_ref().and_then(|doc| doc.match_profile.clone());
    let exclusions = user_doc
        .as_ref()
        .and_then(|doc| doc.alerts.clone())
        .unwrap_or_default();

    // No profile (or no categories yet) is a normal state on the way in, not an error: the
    // caller branches on `reason` instead of parsing a failure.
    let Some(profile) = profile.filter(|p| !p.is_blank()) else {
        return Ok((
            StatusCode::OK,
            Json(json!({
                "status": "success",
                "data": [],
                "meta": {
                    "reason": "no_match_profile",
                    "threshold": state.config.match_threshold,
                    "total": 0,
                    "considered": 0,
                    "qualified": 0,
                    "queued": 0
                }
            })),
        ));
    };

    // Already-queued jobs are excluded only on the automated path. The manual list is a
    // live view and keeps showing what the user has already seen.
    let exclude = if query.record {
        queued_job_ids(&state, &user.id).await?
    } else {
        Vec::new()
    };

    let threshold = state.config.match_threshold;
    let country_wanted = resolve_country(
        query.country.as_deref(),
        user_doc.as_ref().and_then(|doc| doc.location.as_deref()),
    )?;
    let ranked = ranked_candidates(
        &state,
        &profile,
        &exclusions,
        &exclude,
        &Ranking {
            limit: limit as i64,
            threshold,
            scope: FeedScope::Both,
            country: &country_wanted,
        },
    )
    .await?;
    let queued_count = if query.record {
        queue_notifications(&state, &user.id, &ranked.rows, threshold).await?
    } else {
        0
    };

    let data: Vec<Value> = ranked
        .rows
        .iter()
        .map(|(score, job)| json!({ "job": job, "matchScore": score }))
        .collect();

    // An empty list after the cut-off is a normal answer, not an error: `reason` says so and
    // `considered` shows the member the decision was made on real rows rather than none.
    // Same vocabulary as `MatchOutcome::reason`: nothing admitted is `no_candidates`, not a
    // low fit. The portal prints its own sentence per reason.
    let reason = if !data.is_empty() {
        "ok"
    } else if ranked.considered == 0 {
        "no_candidates"
    } else {
        "below_threshold"
    };
    let total = data.len();
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": data,
            "meta": {
                "reason": reason,
                "threshold": threshold,
                "total": total,
                "considered": ranked.considered,
                "qualified": ranked.qualified,
                "queued": queued_count,
                "country": country_wanted,
                "duplicatesRemoved": ranked.duplicates_removed,
                // The categories the search actually used: what the interview derived plus
                // whatever the member picked as notification positions.
                "categories": profile.effective_categories(),
                "categorySource": profile.category_source,
            }
        })),
    ))
}

#[derive(Deserialize)]
pub struct ForUserQuery {
    #[serde(default = "default_match_limit")]
    limit: u64,
    /// `job` or `tender`; absent (or `both`) means both. `jobs`/`tenders` are accepted too
    /// because that is the vocabulary `wants` uses.
    #[serde(default)]
    kind: Option<String>,
    /// `so` (the default), another short code, or `all`; same rule as `GET /jobs`.
    #[serde(default)]
    country: Option<String>,
    /// Overrides the fit cut-off for this read only. `/jobs` browsing passes `0` because the
    /// member asked to see what is live in their areas, not only the rows that would justify
    /// an alert; the digest keeps the configured cut-off. Clamped to 0..=100.
    #[serde(default)]
    threshold: Option<i64>,
}

/// `GET /internal/jobs/for-user/{discord_id}` — the member's own list, addressed by the
/// identity the bot actually holds.
///
/// The payload is deliberately IDENTICAL to `GET /jobs/match` (`data: [{job, matchScore}]`,
/// the same `meta` keys), so the bot has one shape to render and the portal keeps its own
/// authoritative route. Nothing is queued: this is a read, and `/jobs` must not consume the
/// alert state the daily digest owns.
///
/// An unknown snowflake is a 404. "No profile yet" is NOT: it is
/// `meta.reason: "no_match_profile"` with an empty list, exactly like the portal.
async fn jobs_for_user(
    State(state): State<AppState>,
    Path(discord_id): Path<String>,
    Query(query): Query<ForUserQuery>,
) -> ApiResult {
    let discord_id = discord_id.trim();
    let limit = query.limit.clamp(1, 20);
    // `both` is spelled out by a caller that has a stored preference, so accept it. An
    // unknown value is a 400: this is a caller instruction, not stored data.
    let scope = FeedScope::parse(query.kind.as_deref())?;

    // The snowflake is the account's identity, so an unknown one means no account. A
    // malformed id can only be a caller bug and is treated the same way rather than
    // explained back to a client that has no business sending one.
    let Some(link) = state
        .discord_links()
        .find_one(doc! { "discord_id": discord_id })
        .await?
    else {
        return Err(AppError::NotFound(
            "This Discord account is not registered with Jobify yet.".to_string(),
        ));
    };

    let outcome = match_rows(
        &state,
        &link.user_id,
        limit as i64,
        scope,
        query.country.as_deref(),
        query.threshold,
    )
    .await?;
    let data: Vec<Value> = outcome
        .rows
        .iter()
        .map(|(score, job)| json!({ "job": job, "matchScore": score }))
        .collect();

    tracing::debug!(
        user_id = %link.user_id,
        rows = data.len(),
        considered = outcome.considered,
        threshold = outcome.threshold,
        reason = outcome.reason(),
        categories = outcome.areas.categories.len(),
        source = %outcome.areas.source,
        "for-user match read"
    );
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": data,
            "meta": {
                "reason": outcome.reason(),
                "threshold": outcome.threshold,
                "total": data.len(),
                "considered": outcome.considered,
                "qualified": outcome.qualified,
                "queued": 0,
                "country": outcome.country,
                "duplicatesRemoved": outcome.duplicates_removed,
                "categories": outcome.areas.categories,
                "categorySource": outcome.areas.source,
            }
        })),
    ))
}

/// Job ids already queued for a user, so the automated path never repeats one.
async fn queued_job_ids(state: &AppState, user_id: &str) -> Result<Vec<String>, AppError> {
    let mut cursor = state
        .notifications()
        .find(doc! { "user_id": user_id, "job_id": { "$exists": true } })
        .await?;
    let mut ids: Vec<String> = Vec::new();
    while let Some(notification) = cursor.try_next().await? {
        if let Some(job_id) = notification.job_id {
            ids.push(job_id);
        }
    }
    Ok(ids)
}

/// Candidates for a profile, scored, cut off and sorted.
///
/// The single place that decides what is eligible: the live-job rule, the profile's
/// categories, the user's own rejections, the kind scope, what has already been queued
/// and — the last line — the fit cut-off. The portal list and the automated alert path both
/// come through here, so a row cannot be good enough for one and not the other.
///
/// `considered` counts every admitted row that was scored; `qualified` counts those at or
/// above `threshold` BEFORE `limit` is applied, so a caller can report "2 of 11" instead of
/// pretending the corpus was two rows.
pub(crate) struct Ranked {
    pub rows: Vec<(i64, JobDoc)>,
    pub considered: usize,
    pub qualified: usize,
    /// Rows this read collapsed as the same vacancy carried by two boards.
    pub duplicates_removed: usize,
}

/// The narrowing one matcher read applies, as one value so the call sites cannot pass the
/// country and the kind scope in the wrong order.
pub(crate) struct Ranking<'a> {
    pub limit: i64,
    pub threshold: i64,
    pub scope: FeedScope,
    pub country: &'a str,
}

async fn ranked_candidates(
    state: &AppState,
    profile: &MatchProfile,
    exclusions: &AlertPrefs,
    exclude_job_ids: &[String],
    ranking: &Ranking<'_>,
) -> Result<Ranked, AppError> {
    let limit = ranking.limit;
    let threshold = ranking.threshold;
    let country = ranking.country;
    let mut filter = live_job_filter();

    // What the member asked for, and where they are. Applied as its own `$and` element
    // because `live_job_filter` already owns the top-level `$or`, and a Document cannot hold
    // one key twice; `?country=all` adds nothing.
    let mut extra: Vec<Document> = Vec::new();
    if let Some(clause) = country_query_clause(country) {
        extra.push(clause);
    }
    if !extra.is_empty() {
        filter.insert("$and", extra);
    }

    let gate = profile.effective_categories();
    let mut categories = doc! { "$in": &gate };
    // A rejected category is applied to the query rather than filtered after scoring, so
    // it cannot occupy one of the returned slots.
    if !exclusions.excluded_categories.is_empty() {
        categories.insert("$nin", &exclusions.excluded_categories);
    }
    filter.insert("category", categories);

    if !exclude_job_ids.is_empty() {
        filter.insert("_id", doc! { "$nin": exclude_job_ids });
    }

    // "Not a fit" on an employer, case-insensitive, built from escaped literals so a
    // company name containing regex metacharacters cannot corrupt the pattern.
    if !exclusions.excluded_employers.is_empty() {
        let names: Vec<String> = exclusions
            .excluded_employers
            .iter()
            .map(|name| escape_regex(name))
            .collect();
        filter.insert(
            "organization",
            doc! { "$not": { "$regex": format!("^({})$", names.join("|")), "$options": "i" } },
        );
    }

    // Both collections are read into ONE pool and scored together before the cut-off, the
    // dedupe, the sort and the limit. Scoring each collection and merging afterwards would
    // truncate one kind before the other had a chance to compete for the same slots.
    let options = FindOptions::builder().limit(300).build();
    let mut scored: Vec<(i64, JobDoc)> = Vec::new();
    let mut considered = 0usize;
    for (_label, collection) in ranking.scope.collections(state) {
        let mut cursor = collection
            .find(filter.clone())
            .with_options(options.clone())
            .await?;
        while let Some(job) = cursor.try_next().await? {
            // Country first: a member in Somalia is never shown a Djibouti row, and a row
            // whose country was never derived is decided from its location text.
            if !country_accepts(&job, country) {
                continue;
            }
            considered += 1;
            let score = score_job(&job, profile);
            // A 0 means the category gate rejected the row (the query already excludes
            // those, so this is the hand-built-document case). Either way it can never be
            // shown.
            if score < threshold {
                continue;
            }
            scored.push((score, job));
        }
    }

    // Two boards carrying the same vacancy are one row here too, and BEFORE the limit slices
    // the list: otherwise a duplicate can occupy a slot a real job would have taken.
    let (mut scored, duplicates_removed) = dedupe_rows(scored);
    let qualified = scored.len();

    // Recency, then id, as tie-breakers so paging is stable.
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.created_at.cmp(&a.1.created_at))
            .then_with(|| b.1.id.cmp(&a.1.id))
    });
    scored.truncate(limit as usize);
    Ok(Ranked {
        rows: scored,
        considered,
        qualified,
        duplicates_removed,
    })
}

/// Queue one job_match notification per job. A duplicate-key error means the partial
/// unique index caught a repeat, which is the intended outcome rather than a failure.
///
/// The cut-off is re-checked here even though `ranked_candidates` already applied it: this is
/// the function that writes the digest, and a row under the threshold reaching a member's
/// phone is the one failure this feature exists to prevent.
async fn queue_notifications(
    state: &AppState,
    user_id: &str,
    scored: &[(i64, JobDoc)],
    threshold: i64,
) -> Result<u64, AppError> {
    let now = now_iso();
    let mut queued = 0u64;
    for (score, job) in scored {
        if *score < threshold {
            tracing::debug!(job_id = %job.id, score = *score, threshold, "match: skipping sub-threshold row");
            continue;
        }
        let notification = NotificationDoc {
            id: uuid_id(),
            user_id: user_id.to_string(),
            notification_type: "job_match".to_string(),
            job_id: Some(job.id.clone()),
            title: job.title.clone(),
            body: job.description.clone(),
            match_score: Some(*score),
            read: false,
            sent_at: None,
            failed_reason: None,
            created_at: now.clone(),
        };
        if state
            .notifications()
            .insert_one(&notification)
            .await
            .is_ok()
        {
            queued += 1;
        }
    }
    Ok(queued)
}

/// The automated path for one user, with no HTTP caller — the alert refresh runs this per
/// subscriber. Returns how many notifications were newly queued.
///
/// Never queues a row under `MATCH_THRESHOLD`: a member with nothing above the cut-off gets
/// zero rows, which is a correct answer, not a failure. The caller logs the fact once per run.
pub async fn queue_matches_for_user(
    state: &AppState,
    user_id: &str,
    limit: i64,
) -> Result<u64, AppError> {
    let Some(user) = state.users().find_one(doc! { "_id": user_id }).await? else {
        return Ok(0);
    };
    // No profile means nothing to match on yet: a normal state, not an error.
    let Some(profile) = user.match_profile.clone().filter(|p| !p.is_blank()) else {
        return Ok(0);
    };
    let threshold = state.config.match_threshold;
    let exclusions = user.alerts.clone().unwrap_or_default();
    let exclude = queued_job_ids(state, user_id).await?;
    // A member who asked for tenders only must not be queued jobs they will never be sent.
    let scope = FeedScope::from_preference(user.wants.as_deref());
    // The digest is a Somalia-only list unless the member's own location says otherwise:
    // the same country default the portal and `/jobs` use.
    let country_wanted = country::default_for_member(user.location.as_deref());
    let ranked = ranked_candidates(
        state,
        &profile,
        &exclusions,
        &exclude,
        &Ranking {
            limit,
            threshold,
            scope,
            country: country_wanted,
        },
    )
    .await?;
    queue_notifications(state, user_id, &ranked.rows, threshold).await
}

/// What one matcher read produced, for a caller that must report the decision as well as
/// the rows. `areas` is the stored decision (possibly empty = no usable profile).
pub(crate) struct MatchOutcome {
    pub rows: Vec<(i64, JobDoc)>,
    pub areas: crate::models::MatchAreas,
    /// Rows the gate admitted and the matcher scored, before the cut-off.
    pub considered: usize,
    /// Rows at or above the cut-off, before `limit`.
    pub qualified: usize,
    pub threshold: i64,
    /// The country this read was filtered to, so a caller can report it.
    pub country: String,
    /// Rows collapsed as the same vacancy carried by two boards.
    pub duplicates_removed: usize,
}

impl MatchOutcome {
    /// `no_match_profile` when nothing was decided to match on, `below_threshold` when there
    /// is a profile but nothing cleared the cut-off (an honest empty list, not an error), and
    /// `ok` otherwise.
    pub fn reason(&self) -> &'static str {
        if self.areas.categories.is_empty() {
            "no_match_profile"
        } else if !self.rows.is_empty() {
            "ok"
        } else if self.considered == 0 {
            // Nothing was even admitted for the profile: an empty tender corpus, or a kind
            // filter with no stock. Reporting `below_threshold` here would claim rows scored
            // low when none were scored — the empty list is the corpus, not the member's fit.
            "no_candidates"
        } else {
            "below_threshold"
        }
    }
}

/// The matcher for a stored account, with no HTTP caller.
///
/// `GET /jobs/match` (bearer), `GET /internal/jobs/for-user/{discord_id}` and the bot's
/// `GET /internal/discord/matches/{discord_id}` all come through here, so the portal and
/// Discord cannot disagree about what matches — the same reason `live_job_filter` exists.
/// It always reads, never queues: the caller decides whether a read should consume alert
/// state, and every current caller says no.
///
/// `scope` selects which collection(s) are read: `Jobs`, `Tenders`, or `Both` merged into
/// one candidate pool. It is applied as a destination rather than a scoring filter, so a
/// member who asked for tenders is not shown jobs that occupied the returned slots — and an
/// empty tenders collection reports `no_candidates` instead of a phantom low fit.
///
/// An empty category list is the "no usable profile" state, which is normal on the way into
/// onboarding.
pub(crate) async fn match_rows(
    state: &AppState,
    user_id: &str,
    limit: i64,
    scope: FeedScope,
    country_requested: Option<&str>,
    threshold_override: Option<i64>,
) -> Result<MatchOutcome, AppError> {
    // Browsing passes 0: the member asked what is live in their areas, so every admitted row
    // is listed and ranked rather than hidden under the alert cut-off.
    let threshold = threshold_override
        .map(|value| value.clamp(0, 100))
        .unwrap_or(state.config.match_threshold);
    let empty = |country: &str| MatchOutcome {
        rows: Vec::new(),
        areas: crate::models::MatchAreas::default(),
        considered: 0,
        qualified: 0,
        threshold,
        country: country.to_string(),
        duplicates_removed: 0,
    };
    let Some(user) = state.users().find_one(doc! { "_id": user_id }).await? else {
        return Ok(empty(SOMALIA));
    };
    // An explicit `?country=` wins, otherwise the member's own country, otherwise Somalia.
    let country_wanted = resolve_country(country_requested, user.location.as_deref())?;
    let exclusions = user.alerts.clone().unwrap_or_default();
    let Some(profile) = user
        .match_profile
        .clone()
        .filter(|profile| !profile.categories.is_empty())
    else {
        return Ok(empty(&country_wanted));
    };

    let ranked = ranked_candidates(
        state,
        &profile,
        &exclusions,
        &[],
        &Ranking {
            limit,
            threshold,
            scope,
            country: &country_wanted,
        },
    )
    .await?;
    let areas = crate::models::MatchAreas {
        categories: profile.categories.clone(),
        source: profile.category_source.clone(),
    };
    Ok(MatchOutcome {
        rows: ranked.rows,
        areas,
        considered: ranked.considered,
        qualified: ranked.qualified,
        country: country_wanted,
        duplicates_removed: ranked.duplicates_removed,
        threshold,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        AppError, Document, FeedScope, JobDoc, Json, MatchProfile, StatusCode, canonical_category,
        canonical_kind, doc,
    };

    fn raw(value: &str) -> Option<String> {
        Some(value.to_string())
    }

    #[test]
    fn a_kind_is_tender_only_when_it_says_so() {
        // The kind is now a DESTINATION: "tender" routes to the tenders collection, and
        // everything else — including a label nobody knows — is a job.
        assert_eq!(canonical_kind(&raw("tender")), FeedScope::Tenders);
        assert_eq!(canonical_kind(&raw("  TENDER  ")), FeedScope::Tenders);
        assert_eq!(canonical_kind(&raw("job")), FeedScope::Jobs);
        assert_eq!(canonical_kind(&raw("consultancy")), FeedScope::Jobs);
        assert_eq!(canonical_kind(&raw("")), FeedScope::Jobs);
        assert_eq!(canonical_kind(&None), FeedScope::Jobs);
    }

    // The read parameter keeps both spellings and both `both` aliases, and an unknown value
    // is refused rather than quietly widened to the whole corpus.
    #[test]
    fn a_kind_parameter_selects_a_collection_and_refuses_an_unknown_value() {
        assert_eq!(FeedScope::parse(None).unwrap(), FeedScope::Both);
        assert_eq!(FeedScope::parse(Some("")).unwrap(), FeedScope::Both);
        assert_eq!(FeedScope::parse(Some("both")).unwrap(), FeedScope::Both);
        assert_eq!(FeedScope::parse(Some("ALL")).unwrap(), FeedScope::Both);
        assert_eq!(
            FeedScope::parse(Some("TENDERS")).unwrap(),
            FeedScope::Tenders
        );
        assert_eq!(
            FeedScope::parse(Some("tender")).unwrap(),
            FeedScope::Tenders
        );
        assert_eq!(FeedScope::parse(Some("jobs")).unwrap(), FeedScope::Jobs);
        assert_eq!(FeedScope::parse(Some(" job ")).unwrap(), FeedScope::Jobs);
        assert!(FeedScope::parse(Some("nonsense")).is_err());

        // The stored preference never errors: unknown data means "both".
        assert_eq!(
            FeedScope::from_preference(Some("nonsense")),
            FeedScope::Both
        );
        assert_eq!(
            FeedScope::from_preference(Some("tenders")),
            FeedScope::Tenders
        );
    }

    #[test]
    fn accepts_a_known_slug_after_trimming() {
        assert_eq!(canonical_category(&raw("ict")), Some("ict"));
        assert_eq!(
            canonical_category(&raw("  environment_and_climate  ")),
            Some("environment_and_climate")
        );
        assert_eq!(canonical_category(&raw("wash")), Some("wash"));
    }

    #[test]
    fn rejects_unknown_free_text_and_blank() {
        assert_eq!(canonical_category(&raw("totally_made_up_slug")), None);
        assert_eq!(canonical_category(&raw("Computer Technology")), None);
        assert_eq!(
            canonical_category(&raw("Water, Sanitation & Hygiene")),
            None
        );
        assert_eq!(canonical_category(&None), None);
        assert_eq!(canonical_category(&raw("")), None);
        assert_eq!(canonical_category(&raw("   ")), None);
    }

    // The visibility rule is the one thing standing between a user and a job that closed
    // weeks ago, so it is tested against a real collection rather than by re-stating the
    // filter in Rust. Uses its own throwaway database; the live one is never touched.
    #[tokio::test]
    async fn live_filter_keeps_open_jobs_and_expires_undated_ones_after_fourteen_days() {
        use super::{UNDATED_JOB_LIFETIME_DAYS, live_job_filter};
        use crate::util::{nairobi_days_ago, nairobi_today};
        use futures_util::TryStreamExt;
        use mongodb::bson::doc;

        let uri = std::env::var("MONGODB_URI")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017/jobify".to_string());
        let Ok(client) = mongodb::Client::with_uri_str(&uri).await else {
            eprintln!("skipping: no MongoDB reachable at {uri}");
            return;
        };
        let db = client.database("jobify_test_live_filter");
        let _ = db.drop().await;
        let jobs = db.collection::<mongodb::bson::Document>("jobs");

        let day = |n: i64| nairobi_days_ago(n);
        let stamp = |n: i64| format!("{}T09:00:00Z", nairobi_days_ago(n));
        let cases: Vec<(&str, mongodb::bson::Document)> = vec![
            ("open", doc! { "deadline": nairobi_days_ago(-5) }),
            ("closed", doc! { "deadline": day(1) }),
            ("closes-today", doc! { "deadline": nairobi_today() }),
            (
                "undated-posted-recently",
                doc! { "deadline": null, "posted_date": day(3) },
            ),
            (
                "undated-posted-too-long-ago",
                doc! { "deadline": null, "posted_date": day(UNDATED_JOB_LIFETIME_DAYS + 1) },
            ),
            (
                "undated-blank-deadline-recent",
                doc! { "deadline": "", "posted_date": day(3) },
            ),
            (
                "undated-no-posting-date-recorded-recently",
                doc! { "deadline": null, "created_at": stamp(3) },
            ),
            (
                "undated-no-posting-date-recorded-long-ago",
                doc! { "deadline": null, "created_at": stamp(UNDATED_JOB_LIFETIME_DAYS + 1) },
            ),
        ];
        for (title, extra) in &cases {
            let mut document = doc! { "_id": *title, "title": *title };
            document.extend(extra.clone());
            jobs.insert_one(&document).await.unwrap();
        }

        let mut cursor = jobs.find(live_job_filter()).await.unwrap();
        let mut live: Vec<String> = Vec::new();
        while let Some(document) = cursor.try_next().await.unwrap() {
            live.push(document.get_str("title").unwrap().to_string());
        }
        live.sort();

        let mut expected = vec![
            "closes-today",
            "open",
            "undated-blank-deadline-recent",
            "undated-no-posting-date-recorded-recently",
            "undated-posted-recently",
        ];
        expected.sort();
        assert_eq!(live, expected);

        let _ = db.drop().await;
    }

    fn job_from(value: serde_json::Value) -> JobDoc {
        serde_json::from_value(value).expect("a minimal job document must deserialize")
    }

    /// A row with the given category and words, to score against a profile.
    fn job_of(category: &str, title: &str, organization: &str, description: &str) -> JobDoc {
        job_from(serde_json::json!({
            "_id": format!("{category}-{}", title.replace(' ', "-")),
            "title": title,
            "organization": organization,
            "description": description,
            "category": category,
        }))
    }

    /// A sector the member picked for notifications is a subscription, not a hint: a plain row
    /// in that sector must clear the cut-off even when no keyword appears in its title. Without
    /// this, an ICT member heard nothing because ICT postings rarely repeat the keyword list.
    #[test]
    fn a_picked_sector_is_a_nudge_not_a_pass() {
        use super::{MATCH_THRESHOLD_DEFAULT, score_job};

        // A picked sector is a notification preference, so it must not carry a row over the
        // cut-off by itself: 20 base + 25 category + 10 sector = 55.
        let profile = MatchProfile {
            categories: vec!["ict".to_string()],
            keywords: vec!["ict officer".to_string()],
            positions: vec!["ict".to_string()],
            ..Default::default()
        };
        let plain = job_of(
            "ict",
            "Cybersecurity Expert",
            "FEDERAL GOVERNMENT OF SOMALIA",
            "Protect the payment system.",
        );
        assert!(
            score_job(&plain, &profile) < MATCH_THRESHOLD_DEFAULT,
            "a picked sector alone is not evidence about the job"
        );

        // Education plus ONE keyword from a position actually worked does clear it.
        let titled = job_of(
            "ict",
            "ICT Officer",
            "FEDERAL GOVERNMENT OF SOMALIA",
            "Protect the payment system.",
        );
        assert!(score_job(&titled, &profile) >= MATCH_THRESHOLD_DEFAULT);
    }

    /// The two answers taken after the last job must reach the matcher: the positions WIDEN the
    /// gate, and the employer type nudges a row up only when the organisation looks like it.
    #[test]
    fn positions_widen_the_gate_and_the_employer_type_only_nudges() {
        use super::score_job;

        let mut profile = MatchProfile {
            categories: vec!["ict".to_string()],
            keywords: vec!["data".to_string()],
            ..Default::default()
        };

        let wash = job_of("wash", "Water Officer", "UNICEF", "data collection");
        let ict = job_of("ict", "Data Officer", "Somali Telecom Ltd", "data");

        // Without positions, the wash row is outside the gate.
        assert_eq!(score_job(&wash, &profile), 0);

        // Picking it as a position widens the gate; picking the private employer type lifts the
        // ICT row by the nudge, and costs the wash row nothing.
        profile.positions = vec!["wash".to_string()];
        assert!(
            score_job(&wash, &profile) > 0,
            "a chosen position is inside the gate"
        );

        let tuned = MatchProfile {
            employer_types: vec!["private".to_string()],
            ..profile.clone()
        };
        let untuned = MatchProfile {
            employer_types: Vec::new(),
            ..profile.clone()
        };
        assert_eq!(
            score_job(&ict, &tuned) - score_job(&ict, &untuned),
            super::SCORE_EMPLOYER_TYPE,
            "the private employer type adds exactly the nudge to the telecom row"
        );

        // The nudge never becomes a filter: an unrecognised organisation still scores.
        let unknown = job_of("ict", "Data Officer", "Xamar Analytics", "data");
        assert!(score_job(&unknown, &profile) > 0);
    }

    /// The employer-type reader is a heuristic over a name, so pin its three answers.
    #[test]
    fn employer_types_are_read_from_the_organisation_name() {
        use super::employer_type_of;
        assert_eq!(employer_type_of("Ministry of Health"), "government");
        assert_eq!(employer_type_of("UNICEF Somalia"), "ngo");
        assert_eq!(employer_type_of("Hormuud Telecom"), "private");
        assert_eq!(employer_type_of("Xamar Analytics"), "");
    }

    /// The arithmetic, checked at the boundaries the product turns on: inside a decided
    /// category a row is only as good as the advert's own words, and the top of the scale —
    /// both keyword caps saturated — is exactly the 70 cut-off.
    #[test]
    fn a_category_alone_stays_below_the_cut_off_and_saturated_keywords_reach_it() {
        use super::{MATCH_THRESHOLD_DEFAULT, score_job};

        let profile = MatchProfile {
            categories: vec!["ict".to_string()],
            keywords: vec![
                "data".to_string(),
                "analyst".to_string(),
                "sql".to_string(),
                "reporting".to_string(),
            ],
            locations: Vec::new(),
            category_source: "llm".to_string(),
            facts_key: String::new(),
            updated_at: String::new(),
            ..Default::default()
        };

        // 20 base + 25 exact category = 45: in the right field, nothing about the job itself.
        // This is the row that used to be listed beside a real IT role.
        let bare = job_from(serde_json::json!({
            "_id": "job-1",
            "title": "Prequalified Enumerator",
            "description": "",
            "category": "ict",
        }));
        assert_eq!(score_job(&bare, &profile), 45);
        assert!(score_job(&bare, &profile) < MATCH_THRESHOLD_DEFAULT);

        // 20 + 25 + 30: two title keywords saturate the title cap.
        let titled = job_from(serde_json::json!({
            "_id": "job-2",
            "title": "Data Analyst",
            "description": "",
            "category": "ict",
        }));
        assert_eq!(score_job(&titled, &profile), 75);
        assert!(score_job(&titled, &profile) >= MATCH_THRESHOLD_DEFAULT);

        // 20 + 25 + 30 + 10 = 85: both caps saturated.
        let saturated = job_from(serde_json::json!({
            "_id": "job-3",
            "title": "Data Analyst",
            "description": "SQL reporting dashboards",
            "category": "ict",
        }));
        assert_eq!(score_job(&saturated, &profile), 85);
        assert!(score_job(&saturated, &profile) >= MATCH_THRESHOLD_DEFAULT);
    }

    /// A description-only keyword adds at most 5 per hit, capped at 10 — it can never lift a
    /// row over the cut-off by itself.
    #[test]
    fn a_description_keyword_adds_at_most_ten() {
        use super::{SCORE_KEYWORD_DESCRIPTION_MAX, score_job};

        let profile = MatchProfile {
            categories: vec!["ict".to_string()],
            keywords: vec![
                "cctv".to_string(),
                "troubleshooting".to_string(),
                "hardware maintenance".to_string(),
                "microsoft office".to_string(),
            ],
            ..MatchProfile::default()
        };
        let job = job_from(serde_json::json!({
            "_id": "job-1",
            "title": "Prequalified Enumerator",
            "description": "CCTV, troubleshooting, hardware maintenance and Microsoft Office",
            "category": "ict",
        }));

        let baseline = job_from(serde_json::json!({
            "_id": "job-2",
            "title": "Prequalified Enumerator",
            "description": "",
            "category": "ict",
        }));
        let added = score_job(&job, &profile) - score_job(&baseline, &profile);
        assert_eq!(added, SCORE_KEYWORD_DESCRIPTION_MAX);
        assert!(added <= 10, "four description hits add {added}");
        assert!(
            score_job(&job, &profile) < super::MATCH_THRESHOLD_DEFAULT,
            "a description can never carry a row over the cut-off"
        );
    }

    /// The category is a hard gate: a job outside the member's categories scores 0, not a
    /// small number, so it can never be returned by any of the three routes.
    #[test]
    fn a_foreign_category_scores_zero() {
        use super::score_job;

        let profile = MatchProfile {
            categories: vec!["ict".to_string()],
            keywords: vec!["data".to_string()],
            ..MatchProfile::default()
        };
        let wash = job_from(serde_json::json!({
            "_id": "job-1",
            "title": "Data Analyst",
            "description": "data",
            "category": "wash",
        }));
        assert_eq!(score_job(&wash, &profile), 0);

        let uncategorised = job_from(serde_json::json!({
            "_id": "job-2",
            "title": "Data Analyst",
            "description": "data",
        }));
        assert_eq!(score_job(&uncategorised, &profile), 0);
    }

    /// The scale has a hard ceiling and the maximum attainable score is now 70: base,
    /// category, and both keyword caps. Nothing a profile can say pushes a row above it.
    #[test]
    fn the_score_stops_at_the_top_of_the_scale() {
        use super::{MATCH_THRESHOLD_DEFAULT, score_job};

        let profile = MatchProfile {
            categories: vec!["ict".to_string()],
            keywords: vec![
                "data".to_string(),
                "analyst".to_string(),
                "sql".to_string(),
                "reporting".to_string(),
                "dashboards".to_string(),
                "mongodb".to_string(),
                "excel".to_string(),
            ],
            ..MatchProfile::default()
        };
        let job = job_from(serde_json::json!({
            "_id": "job-1",
            "title": "Data Analyst, SQL Reporting Dashboards",
            "description": "MongoDB and Excel",
            "category": "ict",
        }));
        assert_eq!(
            score_job(&job, &profile),
            85,
            "20 base + 25 category + 30 title + 10 description is the whole scale without a sector pick"
        );
        assert!(score_job(&job, &profile) >= MATCH_THRESHOLD_DEFAULT);
    }

    /// The bot's one call: same rows and shape as `/jobs/match`, addressed by snowflake, and
    /// a 404 — not an empty list — for an account that does not exist.
    #[tokio::test]
    async fn the_for_user_route_404s_for_an_unknown_snowflake_and_lists_a_known_members_rows() {
        use super::{ForUserQuery, jobs_for_user};
        use crate::config::Config;
        use crate::state::AppState;
        use axum::extract::{Path, Query, State};

        let uri = std::env::var("MONGODB_URI")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017/jobify".to_string());
        let Ok(client) = mongodb::Client::with_uri_str(&uri).await else {
            eprintln!("skipping: no MongoDB reachable at {uri}");
            return;
        };
        let db = client.database("jobify_test_jobs_for_user");
        let _ = db.drop().await;
        let state = AppState::new(db.clone(), Config::from_env());

        let missing = jobs_for_user(
            State(state.clone()),
            Path("900000000000000000".to_string()),
            Query(ForUserQuery {
                limit: 5,
                kind: None,
                country: None,
                threshold: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(missing, AppError::NotFound(_)),
            "an unknown snowflake is an unknown account"
        );

        db.collection::<Document>("discord_links")
            .insert_one(doc! {
                "_id": "link-901",
                "discord_id": "900000000000000901",
                "user_id": "user-901",
                "username": "sahra",
            })
            .await
            .unwrap();
        db.collection::<Document>("users")
            .insert_one(doc! {
                "_id": "user-901",
                "name": "Sahra",
                "education": [],
                "match_profile": {
                    "categories": ["ict"],
                    "keywords": ["data", "analyst", "sql", "reporting"],
                    "locations": [],
                    "category_source": "llm",
                },
            })
            .await
            .unwrap();
        db.collection::<Document>("jobs")
            .insert_one(doc! {
                "_id": "job-901",
                "title": "Data Analyst",
                "organization": "Hormuud",
                "description": "SQL reporting",
                "category": "ict",
                "deadline": crate::util::nairobi_days_ago(-5),
                "created_at": crate::util::now_iso(),
            })
            .await
            .unwrap();

        let (status, Json(body)) = jobs_for_user(
            State(state),
            Path("900000000000000901".to_string()),
            Query(ForUserQuery {
                limit: 5,
                kind: Some("job".to_string()),
                country: None,
                threshold: None,
            }),
        )
        .await
        .expect("a registered member must get a list");

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["meta"]["reason"], "ok");
        assert_eq!(body["meta"]["categories"], serde_json::json!(["ict"]));
        assert_eq!(body["meta"]["categorySource"], "llm");
        let rows = body["data"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "rows: {body}");
        assert_eq!(rows[0]["job"]["_id"], "job-901");
        assert_eq!(
            rows[0]["matchScore"], 85,
            "base 20 + exact category 25 + two title keywords 30 + two description keywords 10"
        );
        assert_eq!(body["meta"]["threshold"], 60);
        assert_eq!(body["meta"]["queued"], 0, "a read never queues");

        let _ = db.drop().await;
    }

    /// The cut-off is one rule on three routes and the queue: `/jobs/match`, the bot's
    /// for-user list and `queue_matches_for_user` all drop the same weak rows, a member with
    /// nothing above it gets `below_threshold` rather than an error, and the portal feed
    /// keeps its rows but reports the same 0–100.
    #[tokio::test]
    async fn the_threshold_filters_every_route_and_the_queue() {
        use super::{ForUserQuery, JobsQuery, MatchQuery, jobs_for_user, list, match_jobs};
        use super::{queue_matches_for_user, queued_job_ids};
        use crate::auth::AuthUser;
        use crate::config::Config;
        use crate::state::AppState;
        use axum::Extension;
        use axum::extract::{Path, Query, State};

        let uri = std::env::var("MONGODB_URI")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017/jobify".to_string());
        let Ok(client) = mongodb::Client::with_uri_str(&uri).await else {
            eprintln!("skipping: no MongoDB reachable at {uri}");
            return;
        };
        let db = client.database("jobify_test_match_threshold");
        let _ = db.drop().await;
        let state = AppState::new(db.clone(), Config::from_env());
        let live = crate::util::nairobi_days_ago(-5);
        let now = crate::util::now_iso();

        db.collection::<Document>("users")
            .insert_many([
                doc! {
                    "_id": "user-strong",
                    "name": "Sahra",
                    "education": [],
                    "match_profile": {
                        "categories": ["ict"],
                        "keywords": ["data", "analyst", "analysis", "sql"],
                        "locations": [],
                        "category_source": "llm",
                    },
                },
                doc! {
                    "_id": "user-weak",
                    "name": "Dayib",
                    "education": [],
                    "match_profile": {
                        "categories": ["ict"],
                        "keywords": ["cctv"],
                        "locations": [],
                        "category_source": "llm",
                    },
                },
            ])
            .await
            .unwrap();
        db.collection::<Document>("discord_links")
            .insert_many([
                doc! { "_id": "link-a", "discord_id": "111111111111111111", "user_id": "user-strong" },
                doc! { "_id": "link-b", "discord_id": "222222222222222222", "user_id": "user-weak" },
            ])
            .await
            .unwrap();
        db.collection::<Document>("jobs")
            .insert_many([
                doc! {
                    "_id": "job-strong",
                    "title": "REACH Data Analyst",
                    "organization": "ACTED",
                    "description": "Analysis products and SQL",
                    "category": "ict",
                    "deadline": &live,
                    "created_at": &now,
                },
                doc! {
                    "_id": "job-weak",
                    "title": "Prequalified Enumerator",
                    "organization": "NRC",
                    "description": "Collect data for the MEAL unit",
                    "category": "ict",
                    "deadline": &live,
                    "created_at": &now,
                },
                doc! {
                    "_id": "job-wash",
                    "title": "Data Analyst",
                    "organization": "UNICEF",
                    "description": "SQL",
                    "category": "wash",
                    "deadline": &live,
                    "created_at": &now,
                },
            ])
            .await
            .unwrap();

        // 1. The bot's route: one row, and the weak row is gone rather than ranked last.
        let (status, Json(bot)) = jobs_for_user(
            State(state.clone()),
            Path("111111111111111111".to_string()),
            Query(ForUserQuery {
                limit: 5,
                kind: None,
                country: None,
                threshold: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bot["meta"]["reason"], "ok");
        assert_eq!(bot["meta"]["threshold"], 60);
        assert_eq!(
            bot["meta"]["considered"], 2,
            "the wash row never enters: the category is the gate"
        );
        assert_eq!(bot["meta"]["qualified"], 1);
        let rows = bot["data"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "rows: {bot}");
        assert_eq!(rows[0]["job"]["_id"], "job-strong");
        assert_eq!(
            rows[0]["matchScore"], 85,
            "20 base + 25 category + 30 title keywords + 10 description keywords"
        );

        // 2. A member with nothing above the cut-off: an honest empty list, not a 404.
        let (status, Json(weak)) = jobs_for_user(
            State(state.clone()),
            Path("222222222222222222".to_string()),
            Query(ForUserQuery {
                limit: 5,
                kind: None,
                country: None,
                threshold: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(weak["meta"]["reason"], "below_threshold");
        assert_eq!(weak["meta"]["threshold"], 60);
        assert_eq!(weak["meta"]["considered"], 2);
        assert_eq!(weak["meta"]["total"], 0);
        assert_eq!(weak["data"].as_array().unwrap().len(), 0);

        // 3. The portal route (`GET /jobs/match`): the same rows, the same reason.
        let (_, Json(portal)) = match_jobs(
            State(state.clone()),
            Extension(AuthUser {
                id: "user-weak".to_string(),
                role: "user".to_string(),
            }),
            Query(MatchQuery {
                limit: 5,
                record: false,
                country: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(portal["meta"]["reason"], "below_threshold");
        assert_eq!(portal["meta"]["qualified"], 0);
        assert_eq!(portal["meta"]["threshold"], 60);
        assert_eq!(portal["data"].as_array().unwrap().len(), 0);

        // 4. Queueing: a sub-70 row is never written, so the digest cannot send one.
        let queued = queue_matches_for_user(&state, "user-weak", 5)
            .await
            .unwrap();
        assert_eq!(queued, 0, "nothing above the cut-off means nothing queued");
        assert_eq!(queued_job_ids(&state, "user-weak").await.unwrap().len(), 0);

        let queued = queue_matches_for_user(&state, "user-strong", 5)
            .await
            .unwrap();
        assert_eq!(queued, 1);
        assert_eq!(
            queued_job_ids(&state, "user-strong").await.unwrap(),
            vec!["job-strong".to_string()]
        );
        // The score travels WITH the queued row, so the digest can show the same percentage
        // the list did instead of re-deriving it or dropping it.
        let stored = state
            .notifications()
            .find_one(doc! { "user_id": "user-strong" })
            .await
            .unwrap()
            .expect("the strong match must be queued");
        assert_eq!(stored.match_score, Some(85));

        // 5. The feed keeps every live row — browsing is not matching — and reports the same
        //    0–100, so a 30% row cannot look like a 70% one.
        let (_, Json(feed)) = list(
            State(state.clone()),
            Some(Extension(AuthUser {
                id: "user-strong".to_string(),
                role: "user".to_string(),
            })),
            Query(JobsQuery {
                q: None,
                search: None,
                location: None,
                organization: None,
                category: None,
                country: None,
                kind: None,
                page: 1,
                limit: 20,
            }),
        )
        .await
        .unwrap();
        let scores: std::collections::HashMap<&str, i64> = feed["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row["_id"].as_str().unwrap(),
                    row["matchScore"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(scores.len(), 3, "the feed is not cut off: {feed}");
        assert_eq!(scores["job-strong"], 85);
        assert_eq!(
            scores["job-weak"], 50,
            "in the category (20 + 25), one keyword buried in the description (5)"
        );
        assert_eq!(
            scores["job-wash"], 0,
            "a foreign category scores 0 in the feed too"
        );

        let _ = db.drop().await;
    }

    /// The feed must NOT inherit a stored profile location as an implicit filter. Onboarding
    /// no longer asks for one (the product is Somalia-wide), but legacy values survive a
    /// read — and "Kismayo, Mogadishu" is a single substring that matches no stored location,
    /// so a signed-in member's whole feed came back empty. An explicit `?location=` must
    /// still narrow, exactly as before.
    #[tokio::test]
    async fn a_stale_profile_location_does_not_filter_the_feed_but_an_explicit_one_does() {
        use super::{JobsQuery, list};
        use crate::auth::AuthUser;
        use crate::config::Config;
        use crate::state::AppState;
        use axum::Extension;
        use axum::extract::{Query, State};

        let uri = std::env::var("MONGODB_URI")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017/jobify".to_string());
        let Ok(client) = mongodb::Client::with_uri_str(&uri).await else {
            eprintln!("skipping: no MongoDB reachable at {uri}");
            return;
        };
        let db = client.database("jobify_test_feed_location");
        let _ = db.drop().await;
        let state = AppState::new(db.clone(), Config::from_env());
        let live = crate::util::nairobi_days_ago(-5);
        let now = crate::util::now_iso();

        // The real member's legacy value, verbatim: two cities in one string.
        db.collection::<Document>("users")
            .insert_one(doc! {
                "_id": "user-dayib",
                "name": "Dayib",
                "education": [],
                "location": "Kismayo, Mogadishu",
            })
            .await
            .unwrap();
        db.collection::<Document>("jobs")
            .insert_many([
                doc! { "_id": "job-mog", "title": "Data Analyst", "location": "Mogadishu", "deadline": &live, "created_at": &now },
                doc! { "_id": "job-har", "title": "Driver", "location": "Hargeisa", "deadline": &live, "created_at": &now },
            ])
            .await
            .unwrap();

        let viewer = Some(Extension(AuthUser {
            id: "user-dayib".to_string(),
            role: "user".to_string(),
        }));
        let query = |location: Option<&str>| JobsQuery {
            q: None,
            search: None,
            location: location.map(str::to_string),
            organization: None,
            category: None,
            country: None,
            kind: None,
            page: 1,
            limit: 20,
        };

        // No `location` param: every live row, not zero. The stored value is not a filter.
        let (_, Json(feed)) = list(State(state.clone()), viewer.clone(), Query(query(None)))
            .await
            .unwrap();
        assert_eq!(
            feed["meta"]["total"], 2,
            "a junk stored location must not empty the feed: {feed}"
        );
        assert_eq!(feed["data"].as_array().unwrap().len(), 2);

        // ... and it matches what a caller with no account at all sees.
        let (_, Json(public)) = list(State(state.clone()), None, Query(query(None)))
            .await
            .unwrap();
        assert_eq!(public["meta"]["total"], feed["meta"]["total"]);

        // An explicit filter still narrows the feed to the matching location.
        let (_, Json(filtered)) = list(
            State(state.clone()),
            viewer,
            Query(query(Some("Mogadishu"))),
        )
        .await
        .unwrap();
        assert_eq!(filtered["meta"]["total"], 1, "{filtered}");
        assert_eq!(filtered["data"][0]["_id"], "job-mog");

        let _ = db.drop().await;
    }

    /// The Somalia default is a real filter, not a sorting preference: a Djibouti row that
    /// lives in the same collection must be absent from it, and `?country=all` must bring it
    /// back. Rows ingested before `country` existed are decided from their location text.
    #[tokio::test]
    async fn the_default_feed_is_somalia_and_all_brings_the_foreign_rows_back() {
        use super::{JobsQuery, list};
        use crate::config::Config;
        use crate::state::AppState;
        use axum::extract::{Query, State};

        let uri = std::env::var("MONGODB_URI")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017/jobify".to_string());
        let Ok(client) = mongodb::Client::with_uri_str(&uri).await else {
            eprintln!("skipping: no MongoDB reachable at {uri}");
            return;
        };
        let db = client.database("jobify_test_country_filter");
        let _ = db.drop().await;
        let state = AppState::new(db.clone(), Config::from_env());
        let live = crate::util::nairobi_days_ago(-5);
        let now = crate::util::now_iso();

        db.collection::<Document>("jobs")
            .insert_many([
                // Somalia, with the derived country stored.
                doc! { "_id": "job-mog", "title": "Data Analyst", "organization": "ACTED", "location": "Mogadishu", "country": "so", "deadline": &live, "created_at": &now },
                // The owner's complaint: a Djibouti vacancy on a Somali board.
                doc! { "_id": "job-dji", "title": "Focal Person For Fertilizer And Soil Health Hub", "organization": "IGAD", "location": "Djibouti", "country": "dj", "deadline": &live, "created_at": &now },
                doc! { "_id": "job-addis", "title": "Programme Officer", "organization": "AU", "location": "Addis Ababa", "country": "et", "deadline": &live, "created_at": &now },
                // Written before `country` existed: the location text still decides.
                doc! { "_id": "job-nairobi-legacy", "title": "Regional Advisor", "organization": "UN", "location": "Nairobi", "deadline": &live, "created_at": &now },
                // Unknown, but nothing foreign about it: kept in the Somali default.
                doc! { "_id": "job-unknown", "title": "Multiple Locations Roster", "organization": "NRC", "location": "Multiple Locations", "deadline": &live, "created_at": &now },
            ])
            .await
            .unwrap();

        let query = |country: Option<&str>| JobsQuery {
            q: None,
            search: None,
            location: None,
            organization: None,
            category: None,
            country: country.map(str::to_string),
            kind: None,
            page: 1,
            limit: 20,
        };
        let ids = |body: &serde_json::Value| -> Vec<String> {
            let mut ids: Vec<String> = body["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["_id"].as_str().unwrap().to_string())
                .collect();
            ids.sort();
            ids
        };

        // 1. No `?country=`: Somalia only. The Djibouti row is in the DB and not in the list.
        let (_, Json(default_feed)) = list(State(state.clone()), None, Query(query(None)))
            .await
            .unwrap();
        assert_eq!(default_feed["meta"]["country"], "so");
        assert_eq!(
            ids(&default_feed),
            vec!["job-mog".to_string(), "job-unknown".to_string()],
            "the Somali default must exclude Djibouti, Ethiopia and Nairobi: {default_feed}"
        );
        assert!(
            !ids(&default_feed).contains(&"job-dji".to_string()),
            "a Djibouti vacancy must not reach a Somali member"
        );

        // 2. `?country=all` turns the filter off — the row is there, so 1 proves it exists.
        let (_, Json(everything)) = list(State(state.clone()), None, Query(query(Some("all"))))
            .await
            .unwrap();
        assert_eq!(everything["meta"]["country"], "all");
        assert!(
            ids(&everything).contains(&"job-dji".to_string()),
            "?country=all must include the Djibouti row: {everything}"
        );
        assert_eq!(everything["meta"]["total"], 5);

        // 3. An explicit country is honoured on its own.
        let (_, Json(djibouti)) = list(State(state.clone()), None, Query(query(Some("dj"))))
            .await
            .unwrap();
        assert_eq!(ids(&djibouti), vec!["job-dji".to_string()]);

        // 4. A member whose own location is Djibouti defaults to Djibouti, not Somalia.
        db.collection::<Document>("users")
            .insert_one(doc! { "_id": "user-dji", "name": "Hodan", "location": "Djibouti" })
            .await
            .unwrap();
        let (_, Json(member)) = list(
            State(state.clone()),
            Some(axum::Extension(crate::auth::AuthUser {
                id: "user-dji".to_string(),
                role: "user".to_string(),
            })),
            Query(query(None)),
        )
        .await
        .unwrap();
        assert_eq!(member["meta"]["country"], "dj");
        assert_eq!(ids(&member), vec!["job-dji".to_string()]);

        let _ = db.drop().await;
    }

    /// Two boards carrying one vacancy are ONE row, and the row kept is the best one for the
    /// member. A same-title row from a different employer is a genuinely different vacancy
    /// and must survive, which is the failure mode a title-only rule would introduce.
    #[test]
    fn duplicates_collapse_keeping_the_higher_score_and_different_employers_survive() {
        use super::dedupe_rows;

        let unjobs = job_from(serde_json::json!({
            "_id": "job-un",
            "title": "Chief of Section, Human Resources Management",
            "organization": "UNSOS",
            "location": "Mogadishu",
            "source": "unjobs",
            "created_at": "2026-11-20T09:00:00Z",
        }));
        let somalijobs = job_from(serde_json::json!({
            "_id": "job-so",
            "title": "Chief Of Section  Human Resources Management",
            "organization": "UNSOS",
            "location": "Mogadishu, Somalia",
            "source": "somalijobs",
            "created_at": "2026-11-19T09:00:00Z",
        }));
        let other_employer = job_from(serde_json::json!({
            "_id": "job-other",
            "title": "Chief of Section, Human Resources Management",
            "organization": "UNICEF",
            "location": "Mogadishu",
            "source": "somalijobs",
            "created_at": "2026-11-20T09:00:00Z",
        }));

        let (kept, collapsed) = dedupe_rows(vec![
            (75, unjobs.clone()),
            (80, somalijobs.clone()),
            (75, other_employer),
        ]);
        assert_eq!(collapsed, 1, "one pair collapsed");
        assert_eq!(kept.len(), 2, "the other employer stays");
        // The 80 wins even though the 75 was read first.
        assert!(
            kept.iter()
                .any(|(score, job)| job.id == "job-so" && *score == 80),
            "the better-scoring row is the one kept"
        );
        assert!(!kept.iter().any(|(_, job)| job.id == "job-un"));

        // Same score and same age: the source priority decides, somalijobs over unjobs.
        let tie = job_from(serde_json::json!({
            "_id": "job-un-tie",
            "title": "Driver",
            "organization": "ICRC",
            "location": "Mogadishu",
            "source": "unjobs",
            "created_at": "2026-11-20T09:00:00Z",
        }));
        let preferred = job_from(serde_json::json!({
            "_id": "job-so-tie",
            "title": "Driver",
            "organization": "ICRC",
            "location": "Mogadishu",
            "source": "somalijobs",
            "created_at": "2026-11-20T09:00:00Z",
        }));
        let (kept, collapsed) = dedupe_rows(vec![(90, tie), (90, preferred)]);
        assert_eq!(collapsed, 1);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].1.id, "job-so-tie", "somalijobs outranks unjobs");

        // Two cities, same title and employer: two real postings, both kept.
        let garowe = job_from(serde_json::json!({
            "_id": "job-garowe",
            "title": "Housekeeper",
            "organization": "ICRC",
            "location": "Garowe",
            "source": "somalijobs",
        }));
        let baidoa = job_from(serde_json::json!({
            "_id": "job-baidoa",
            "title": "Housekeeper",
            "organization": "ICRC",
            "location": "Baidoa",
            "source": "somalijobs",
        }));
        let (kept, collapsed) = dedupe_rows(vec![(80, garowe), (80, baidoa)]);
        assert_eq!(collapsed, 0);
        assert_eq!(kept.len(), 2);
    }

    /// The country parameter is validated, never silently widened to "all" on a typo.
    #[test]
    fn a_country_parameter_must_be_a_code_or_all() {
        use super::resolve_country;

        assert_eq!(resolve_country(None, None).unwrap(), "so");
        assert_eq!(resolve_country(Some("  "), None).unwrap(), "so");
        assert_eq!(
            resolve_country(None, Some("Hargeisa, Somaliland")).unwrap(),
            "so"
        );
        assert_eq!(resolve_country(None, Some("Djibouti")).unwrap(), "dj");
        assert_eq!(
            resolve_country(Some("ALL"), Some("Djibouti")).unwrap(),
            "all"
        );
        assert_eq!(resolve_country(Some("ke"), Some("Djibouti")).unwrap(), "ke");
        assert!(resolve_country(Some("europe"), None).is_err());
        assert!(resolve_country(Some("s0"), None).is_err());
    }

    /// The exact shapes the live collection has: an employer suffix on one board, the same
    /// acronym in a different position, and one-letter spelling variants of a town.
    #[test]
    fn the_employer_and_town_shapes_the_boards_really_use_are_one_vacancy() {
        use super::same_vacancy;

        let somalijobs = job_from(serde_json::json!({
            "_id": "job-a",
            "title": "Climate Education Consultant",
            "organization": "UNICEF",
            "location": "Mogadishu",
        }));
        let unjobs = job_from(serde_json::json!({
            "_id": "job-b",
            "title": "Climate Education Consultant",
            "organization": "UNICEF - United Nations Children's Fund",
            "location": "Mogadishu",
        }));
        assert!(
            same_vacancy(&somalijobs, &unjobs),
            "an employer suffix must not make a second vacancy"
        );

        let icrc_a = job_from(serde_json::json!({
            "_id": "job-c",
            "title": "Housekeeper",
            "organization": "INTERNATIONAL COMMITTEE OF THE RED CROSS (ICRC)",
            "location": "Garowe",
        }));
        let icrc_b = job_from(serde_json::json!({
            "_id": "job-d",
            "title": "Housekeeper",
            "organization": "ICRC - International Committee of the Red Cross",
            "location": "Garoowe, Somalia",
        }));
        assert!(same_vacancy(&icrc_a, &icrc_b), "word order and spelling");

        // ... but two towns of the same vacancy stay two rows.
        let dhobley = job_from(serde_json::json!({
            "_id": "job-e",
            "title": "Housekeeper",
            "organization": "ICRC - International Committee of the Red Cross",
            "location": "Dhobley, Somalia",
        }));
        assert!(
            !same_vacancy(&icrc_a, &dhobley),
            "Garowe and Dhobley are two postings, not one"
        );

        // A different employer with the same title is never merged.
        let other = job_from(serde_json::json!({
            "_id": "job-f",
            "title": "Housekeeper",
            "organization": "NRC",
            "location": "Garowe",
        }));
        assert!(!same_vacancy(&icrc_a, &other));

        // A common word is not a prefix: "UN" must not swallow "UNICEF".
        let un = job_from(serde_json::json!({
            "_id": "job-g",
            "title": "Housekeeper",
            "organization": "UN",
            "location": "Garowe",
        }));
        assert!(!same_vacancy(&icrc_a, &un));
    }

    // -----------------------------------------------------------------------
    // The split: tenders live in their own collection
    // -----------------------------------------------------------------------

    /// Ingest routes by `kind` and stores NO `kind` field. A tender lands in `tenders`, a
    /// job and a payload with no label land in `jobs`; the same (external_id, source) key
    /// keeps a repeated run idempotent wherever it went.
    #[tokio::test]
    async fn ingest_routes_tenders_to_their_own_collection_and_stores_no_kind_field() {
        use super::{IngestPayload, ingest_jobs};
        use crate::config::Config;
        use crate::state::AppState;
        use axum::extract::State;

        let uri = std::env::var("MONGODB_URI")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017/jobify".to_string());
        let Ok(client) = mongodb::Client::with_uri_str(&uri).await else {
            eprintln!("skipping: no MongoDB reachable at {uri}");
            return;
        };
        let db = client.database("jobify_test_ingest_split");
        let _ = db.drop().await;
        let state = AppState::new(db.clone(), Config::from_env());

        let payload = serde_json::from_value::<IngestPayload>(serde_json::json!([
            {
                "external_id": "tender-1",
                "source": "reliefweb",
                "title": "Water network rehabilitation tender",
                "kind": "tender",
                "url": "https://example.org/tender-1",
            },
            {
                "external_id": "job-1",
                "source": "reliefweb",
                "title": "Data Analyst",
                "kind": "job",
                "url": "https://example.org/job-1",
            },
            {
                "external_id": "job-2",
                "source": "reliefweb",
                "title": "Driver",
                "url": "https://example.org/job-2",
            }
        ]))
        .expect("an ingest payload must deserialize");

        let (status, Json(body)) = ingest_jobs(State(state.clone()), Json(payload))
            .await
            .expect("ingest must succeed");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["received"], 3);
        assert_eq!(body["data"]["persisted"], 3);
        assert_eq!(body["data"]["failed"], 0);

        let tenders = db.collection::<Document>("tenders");
        let jobs = db.collection::<Document>("jobs");
        assert_eq!(
            tenders.count_documents(doc! {}).await.unwrap(),
            1,
            "only the tender may land in `tenders`"
        );
        assert_eq!(
            jobs.count_documents(doc! {}).await.unwrap(),
            2,
            "kind=job and a missing kind both land in `jobs`"
        );

        let stored = tenders
            .find_one(doc! { "external_id": "tender-1" })
            .await
            .unwrap()
            .expect("the tender must be stored in its own collection");
        assert!(
            stored.get("kind").is_none(),
            "a stored document must carry no kind field: {stored:?}"
        );
        assert_eq!(
            stored.get_str("title").unwrap(),
            "Water network rehabilitation tender"
        );

        let stored_job = jobs
            .find_one(doc! { "external_id": "job-1" })
            .await
            .unwrap()
            .expect("the job must be stored in `jobs`");
        assert!(stored_job.get("kind").is_none(), "no kind on a job either");

        // The upsert key still holds: a second run updates the same tender instead of
        // adding a copy, so re-ingest is idempotent in the new collection too.
        let repeat = serde_json::from_value::<IngestPayload>(serde_json::json!({
            "external_id": "tender-1",
            "source": "reliefweb",
            "title": "Water network rehabilitation tender (revised)",
            "kind": "tender",
        }))
        .expect("a single payload must deserialize");
        let _ = ingest_jobs(State(state), Json(repeat)).await.unwrap();
        assert_eq!(
            tenders.count_documents(doc! {}).await.unwrap(),
            1,
            "re-ingesting the same (external_id, source) must upsert, not duplicate"
        );

        let _ = db.drop().await;
    }

    /// `GET /jobs?kind=` selects the collection and never mixes the two unless both were
    /// asked for. The merged list is newest-first across BOTH collections, not one page
    /// followed by the other.
    #[tokio::test]
    async fn the_feed_reads_the_named_collection_and_merges_both_newest_first() {
        use super::{JobsQuery, list};
        use crate::config::Config;
        use crate::state::AppState;
        use axum::extract::{Query, State};

        let uri = std::env::var("MONGODB_URI")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017/jobify".to_string());
        let Ok(client) = mongodb::Client::with_uri_str(&uri).await else {
            eprintln!("skipping: no MongoDB reachable at {uri}");
            return;
        };
        let db = client.database("jobify_test_feed_split");
        let _ = db.drop().await;
        let state = AppState::new(db.clone(), Config::from_env());

        db.collection::<Document>("jobs")
            .insert_one(doc! {
                "_id": "job-old",
                "title": "Driver",
                "location": "Mogadishu",
                "deadline": "2099-01-01",
                "created_at": "2026-01-01T09:00:00Z",
            })
            .await
            .unwrap();
        db.collection::<Document>("jobs")
            .insert_one(doc! {
                "_id": "job-new",
                "title": "Data Analyst",
                "location": "Mogadishu",
                "deadline": "2099-01-01",
                "created_at": "2026-03-01T09:00:00Z",
            })
            .await
            .unwrap();
        db.collection::<Document>("tenders")
            .insert_many([
                doc! {
                    "_id": "tender-middle",
                    "title": "Vehicle maintenance tender",
                    "location": "Mogadishu",
                    "deadline": "2099-01-01",
                    "created_at": "2026-02-01T09:00:00Z",
                },
                doc! {
                    "_id": "tender-newest",
                    "title": "Solar installation tender",
                    "location": "Mogadishu",
                    "deadline": "2099-01-01",
                    "created_at": "2026-04-01T09:00:00Z",
                },
            ])
            .await
            .unwrap();

        let query = |kind: Option<&str>| JobsQuery {
            q: None,
            search: None,
            location: None,
            organization: None,
            category: None,
            country: Some("all".to_string()),
            kind: kind.map(str::to_string),
            page: 1,
            limit: 20,
        };
        let ids = |body: &serde_json::Value| -> Vec<String> {
            body["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["_id"].as_str().unwrap().to_string())
                .collect()
        };

        let (_, Json(tenders)) = list(State(state.clone()), None, Query(query(Some("tender"))))
            .await
            .unwrap();
        assert_eq!(
            ids(&tenders),
            vec!["tender-newest".to_string(), "tender-middle".to_string()],
            "kind=tender must read only the tenders collection: {tenders}"
        );
        assert_eq!(tenders["meta"]["total"], 2);

        let (_, Json(jobs)) = list(State(state.clone()), None, Query(query(Some("job"))))
            .await
            .unwrap();
        assert_eq!(
            ids(&jobs),
            vec!["job-new".to_string(), "job-old".to_string()],
            "kind=job must read only the jobs collection: {jobs}"
        );

        // No `kind` at all means both, ordered by created_at across the merge.
        let (_, Json(both)) = list(State(state.clone()), None, Query(query(None)))
            .await
            .unwrap();
        assert_eq!(
            ids(&both),
            vec![
                "tender-newest".to_string(),
                "job-new".to_string(),
                "tender-middle".to_string(),
                "job-old".to_string(),
            ],
            "the merged feed must be newest-first across both collections: {both}"
        );
        assert_eq!(both["meta"]["total"], 4);

        let _ = db.drop().await;
    }

    /// A tender-scoped match with an empty `tenders` collection is `no_candidates`, not
    /// `below_threshold`: nothing was scored, so nothing can have scored low. The same
    /// member still sees their job when the scope includes `jobs`.
    #[tokio::test]
    async fn a_tender_scoped_match_is_no_candidates_when_tenders_is_empty() {
        use super::{ForUserQuery, jobs_for_user};
        use crate::config::Config;
        use crate::state::AppState;
        use axum::extract::{Path, Query, State};

        let uri = std::env::var("MONGODB_URI")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017/jobify".to_string());
        let Ok(client) = mongodb::Client::with_uri_str(&uri).await else {
            eprintln!("skipping: no MongoDB reachable at {uri}");
            return;
        };
        let db = client.database("jobify_test_tender_no_candidates");
        let _ = db.drop().await;
        let state = AppState::new(db.clone(), Config::from_env());

        db.collection::<Document>("discord_links")
            .insert_one(doc! {
                "_id": "link-950",
                "discord_id": "900000000000000950",
                "user_id": "user-950",
            })
            .await
            .unwrap();
        db.collection::<Document>("users")
            .insert_one(doc! {
                "_id": "user-950",
                "name": "Sahra",
                "education": [],
                "match_profile": {
                    "categories": ["ict"],
                    "keywords": ["data", "analyst", "sql", "reporting"],
                    "locations": [],
                    "category_source": "llm",
                },
            })
            .await
            .unwrap();
        // A job that WOULD match: proves the empty answer comes from the scope, not the fit.
        db.collection::<Document>("jobs")
            .insert_one(doc! {
                "_id": "job-950",
                "title": "Data Analyst",
                "organization": "Hormuud",
                "description": "SQL reporting",
                "category": "ict",
                "deadline": "2099-01-01",
                "created_at": crate::util::now_iso(),
            })
            .await
            .unwrap();

        let (status, Json(tenders_only)) = jobs_for_user(
            State(state.clone()),
            Path("900000000000000950".to_string()),
            Query(ForUserQuery {
                limit: 5,
                kind: Some("tender".to_string()),
                country: None,
                threshold: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(tenders_only["meta"]["reason"], "no_candidates");
        assert_eq!(
            tenders_only["meta"]["considered"], 0,
            "the jobs row must not be read under a tender scope: {tenders_only}"
        );
        assert_eq!(tenders_only["meta"]["total"], 0);
        assert_eq!(tenders_only["data"].as_array().unwrap().len(), 0);

        // Same member, jobs scope: the row is there, so the empty answer above was the
        // collection and not the profile.
        let (_, Json(jobs_only)) = jobs_for_user(
            State(state),
            Path("900000000000000950".to_string()),
            Query(ForUserQuery {
                limit: 5,
                kind: Some("job".to_string()),
                country: None,
                threshold: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(jobs_only["meta"]["reason"], "ok");
        assert_eq!(jobs_only["data"][0]["job"]["_id"], "job-950");

        let _ = db.drop().await;
    }
}
