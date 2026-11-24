//! `GET /categories` — the canonical taxonomy as an API.
//!
//! The bot's onboarding asks a member to pick their sectors from a multi-select, and the
//! scraper/client already mirror this table. Rather than shipping a third hand-maintained
//! copy (which is exactly how "logistics" and "logistics_and_supply_chain" end up meaning
//! different things), the list is served from `categories.rs` — the one source of truth
//! `/jobs?category=` and the match profile are validated against.

use axum::{Json, Router, http::StatusCode, routing::get};
use serde_json::{Value, json};

use crate::categories::{Category, CategoryGroup};
use crate::state::AppState;

use super::ApiResult;

pub fn router() -> Router<AppState> {
    Router::new().route("/categories", get(list_categories))
}

/// The pure part, so the contract is unit-testable without a database.
///
/// `group` is `all_fields` for every row today: the taxonomy is flat, and the field stays
/// in the payload because consumers key on it and a future second group must not be a
/// breaking change.
pub fn catalogue() -> Vec<Value> {
    Category::ALL
        .iter()
        .map(|category| {
            json!({
                "slug": category.slug(),
                "label": category.label(),
                "group": category.group().slug(),
            })
        })
        .collect()
}

async fn list_categories() -> ApiResult {
    let categories = catalogue();
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "categories": categories,
                "count": categories.len(),
            },
            "meta": {
                "groups": CategoryGroup::ALL
                    .iter()
                    .map(|group| json!({ "slug": group.slug(), "label": group.label() }))
                    .collect::<Vec<_>>(),
            }
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_is_served_with_a_slug_label_and_group() {
        let list = catalogue();
        assert_eq!(list.len(), Category::ALL.len());
        assert_eq!(list.len(), 28);

        for row in &list {
            let slug = row["slug"].as_str().expect("slug is a string");
            let label = row["label"].as_str().expect("label is a string");
            let group = row["group"].as_str().expect("group is a string");
            assert!(!label.is_empty(), "{slug} has no label");
            assert!(
                Category::from_slug(slug).is_some(),
                "{slug} does not resolve back to a category"
            );
            assert!(
                CategoryGroup::from_slug(group).is_some(),
                "{group} does not resolve to a group"
            );
        }
    }

    #[test]
    fn slugs_are_unique_so_a_client_can_use_one_as_a_value() {
        let list = catalogue();
        let mut slugs: Vec<&str> = list.iter().filter_map(|r| r["slug"].as_str()).collect();
        let total = slugs.len();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), total, "the list contains a duplicate slug");
    }

    #[test]
    fn the_ict_slug_the_bot_seeds_with_is_present() {
        let list = catalogue();
        assert!(list.iter().any(|row| row["slug"] == "ict"));
        assert!(list.iter().any(|row| row["label"] == "ICT"));
    }
}
