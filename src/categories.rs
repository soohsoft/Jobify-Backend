//! Canonical job-category taxonomy.
//!
//! Broad, flat, and deliberately few: fifteen categories a job seeker can recognise at a
//! glance. The earlier 54-slug tree asked people to choose between "Public Health & Healthcare
//! Management" and "Clinical Medicine & Nursing" before they could see a single job, and the
//! distinctions rarely matched how anyone describes their own work.
//!
//! **Slugs are the wire and database contract.** They are what the scraper sends in
//! `JobInput.category` and what `GET /jobs?category=` matches against, so a variant must never
//! be renamed without migrating stored values. `label()` is the human-facing display name and
//! may change freely.
//!
//! `CategoryGroup` survives as a single group so callers that ask for "the group of this
//! category" keep working; with a flat taxonomy there is nothing to group.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CategoryGroup {
    /// Every category. Kept so the grouping API stays stable now that the taxonomy is flat.
    AllFields,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Health,
    Education,
    BusinessAndFinance,
    ComputerTechnology,
    EnergyElectricalAndSolar,
    Agriculture,
    Fishery,
    Livestock,
    EngineeringAndConstruction,
    LogisticsAndSupplyChain,
    TelecommunicationsAndMedia,
    LegalAndRegulatoryServices,
    /// Environment and climate work. WASH used to live inside this variant; it now has its own
    /// category, because "Environment, Water & Sanitation (WASH)" and a humanitarian "WASH"
    /// would otherwise both claim every water and sanitation posting.
    EnvironmentAndClimate,
    ManufacturingAndProduction,
    HospitalityAndTourism,

    // Humanitarian & development
    HumanRightsAndAdvocacy,
    HumanitarianAidAndEmergencyRelief,
    PeacebuildingAndSecurity,
    GenderEqualityAndSocialInclusion,
    YouthAndChildDevelopment,
    MigrationRefugeesAndDisplaced,
    ArtsCultureAndHeritage,
    GovernanceDemocracyAndCivicParticipation,
    Protection,
    LivelihoodsAndResilience,
    FoodSecurity,
    ShelterAndNonFoodItems,
    Wash,
}

impl CategoryGroup {
    pub const ALL: &'static [CategoryGroup] = &[CategoryGroup::AllFields];

    pub fn slug(self) -> &'static str {
        match self {
            CategoryGroup::AllFields => "all_fields",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CategoryGroup::AllFields => "All fields",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        let slug = slug.trim().to_ascii_lowercase();
        CategoryGroup::ALL
            .iter()
            .copied()
            .find(|group| group.slug() == slug)
    }

    /// Categories belonging to this group, in display order.
    pub fn categories(self) -> &'static [Category] {
        match self {
            CategoryGroup::AllFields => Category::ALL,
        }
    }
}

impl Category {
    /// Every category, in display order.
    pub const ALL: &'static [Category] = &[
        Category::Health,
        Category::Education,
        Category::BusinessAndFinance,
        Category::ComputerTechnology,
        Category::EnergyElectricalAndSolar,
        Category::Agriculture,
        Category::Fishery,
        Category::Livestock,
        Category::EngineeringAndConstruction,
        Category::LogisticsAndSupplyChain,
        Category::TelecommunicationsAndMedia,
        Category::LegalAndRegulatoryServices,
        Category::EnvironmentAndClimate,
        Category::ManufacturingAndProduction,
        Category::HospitalityAndTourism,
        Category::HumanRightsAndAdvocacy,
        Category::HumanitarianAidAndEmergencyRelief,
        Category::PeacebuildingAndSecurity,
        Category::GenderEqualityAndSocialInclusion,
        Category::YouthAndChildDevelopment,
        Category::MigrationRefugeesAndDisplaced,
        Category::ArtsCultureAndHeritage,
        Category::GovernanceDemocracyAndCivicParticipation,
        Category::Protection,
        Category::LivelihoodsAndResilience,
        Category::FoodSecurity,
        Category::ShelterAndNonFoodItems,
        Category::Wash,
    ];

    /// Stable machine slug (wire + database contract).
    pub fn slug(self) -> &'static str {
        match self {
            Category::Health => "health",
            Category::Education => "education",
            Category::BusinessAndFinance => "business_and_finance",
            Category::ComputerTechnology => "computer_technology",
            Category::EnergyElectricalAndSolar => "energy_electrical_and_solar",
            Category::Agriculture => "agriculture",
            Category::Fishery => "fishery",
            Category::Livestock => "livestock",
            Category::EngineeringAndConstruction => "engineering_and_construction",
            Category::LogisticsAndSupplyChain => "logistics_and_supply_chain",
            Category::TelecommunicationsAndMedia => "telecommunications_and_media",
            Category::LegalAndRegulatoryServices => "legal_and_regulatory_services",
            Category::EnvironmentAndClimate => "environment_and_climate",
            Category::HumanRightsAndAdvocacy => "human_rights_and_advocacy",
            Category::HumanitarianAidAndEmergencyRelief => "humanitarian_aid_and_emergency_relief",
            Category::PeacebuildingAndSecurity => "peacebuilding_and_security",
            Category::GenderEqualityAndSocialInclusion => "gender_equality_and_social_inclusion",
            Category::YouthAndChildDevelopment => "youth_and_child_development",
            Category::MigrationRefugeesAndDisplaced => "migration_refugees_and_displaced",
            Category::ArtsCultureAndHeritage => "arts_culture_and_heritage",
            Category::GovernanceDemocracyAndCivicParticipation => {
                "governance_democracy_and_civic_participation"
            }
            Category::Protection => "protection",
            Category::LivelihoodsAndResilience => "livelihoods_and_resilience",
            Category::FoodSecurity => "food_security",
            Category::ShelterAndNonFoodItems => "shelter_and_non_food_items",
            Category::Wash => "wash",
            Category::ManufacturingAndProduction => "manufacturing_and_production",
            Category::HospitalityAndTourism => "hospitality_and_tourism",
        }
    }

    /// Human-facing display name.
    pub fn label(self) -> &'static str {
        match self {
            Category::Health => "Health",
            Category::Education => "Education",
            Category::BusinessAndFinance => "Business & Finance",
            Category::ComputerTechnology => "Computer Technology",
            Category::EnergyElectricalAndSolar => "Energy (Electrical & Solar)",
            Category::Agriculture => "Agriculture",
            Category::Fishery => "Fishery",
            Category::Livestock => "Livestock",
            Category::EngineeringAndConstruction => "Engineering & Construction",
            Category::LogisticsAndSupplyChain => "Logistics & Supply Chain",
            Category::TelecommunicationsAndMedia => "Telecommunications & Media",
            Category::LegalAndRegulatoryServices => "Legal & Regulatory Services",
            Category::EnvironmentAndClimate => "Environment & Climate",
            Category::HumanRightsAndAdvocacy => "Human Rights, Civil Rights & Advocacy",
            Category::HumanitarianAidAndEmergencyRelief => "Humanitarian Aid & Emergency Relief",
            Category::PeacebuildingAndSecurity => "Peacebuilding, Conflict Resolution & Security",
            Category::GenderEqualityAndSocialInclusion => "Gender Equality & Social Inclusion",
            Category::YouthAndChildDevelopment => "Youth & Child Development",
            Category::MigrationRefugeesAndDisplaced => "Migration, Refugees & Displaced Persons",
            Category::ArtsCultureAndHeritage => "Arts, Culture & Heritage Preservation",
            Category::GovernanceDemocracyAndCivicParticipation => {
                "Governance, Democracy & Civic Participation"
            }
            Category::Protection => "Protection",
            Category::LivelihoodsAndResilience => "Livelihoods & Resilience",
            Category::FoodSecurity => "Food Security",
            Category::ShelterAndNonFoodItems => "Shelter & Non-Food Items (NFIs)",
            Category::Wash => "WASH (Water, Sanitation & Hygiene)",
            Category::ManufacturingAndProduction => "Manufacturing & Production",
            Category::HospitalityAndTourism => "Hospitality & Tourism",
        }
    }

    pub fn group(self) -> CategoryGroup {
        CategoryGroup::AllFields
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        let slug = slug.trim().to_ascii_lowercase();
        Category::ALL
            .iter()
            .copied()
            .find(|category| category.slug() == slug)
    }

    /// Prompt-ready list of "- slug — Label" lines for the extraction prompt.
    pub fn prompt_list() -> String {
        Category::ALL
            .iter()
            .map(|category| format!("- {} — {}", category.slug(), category.label()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Slugs of the OTHER categories in the same group as `slug`.
///
/// With a flat taxonomy every category is in the same group, so answering "the rest of the
/// group" would mean "everything" — useless for widening a search. It therefore returns an
/// empty list, and the caller widens to the newest live jobs instead.
pub fn sibling_slugs(_slug: &str) -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taxonomy_is_15_categories_in_one_group() {
        assert_eq!(CategoryGroup::ALL.len(), 1);
        assert_eq!(Category::ALL.len(), 28);
    }

    #[test]
    fn slugs_are_lowercase_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for category in Category::ALL {
            let slug = category.slug();
            assert!(
                slug.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{slug} is not a snake_case slug"
            );
            assert!(seen.insert(slug), "duplicate slug {slug}");
            assert!(!category.label().is_empty());
        }
    }

    #[test]
    fn every_category_round_trips_through_its_slug() {
        for category in Category::ALL {
            assert_eq!(Category::from_slug(category.slug()), Some(*category));
            assert_eq!(
                Category::from_slug(&format!("  {}  ", category.slug())),
                Some(*category)
            );
        }
        assert_eq!(Category::from_slug("nope"), None);
        // The old, finer taxonomy must not resolve any more: a stored value from before the
        // change has to fall out rather than quietly match a broader category.
        assert_eq!(
            Category::from_slug("public_health_and_healthcare_management"),
            None
        );
        // `wash` is a category again, as a humanitarian one — the point of the split.
        assert_eq!(Category::from_slug("wash"), Some(Category::Wash));
        assert_eq!(
            Category::from_slug("environment_water_and_sanitation"),
            None
        );
        assert_eq!(Category::from_slug("computer_tecnology"), None);
    }

    #[test]
    fn prompt_list_carries_one_line_per_category() {
        let list = Category::prompt_list();
        assert_eq!(list.lines().count(), 28);
        assert!(list.contains("- health — Health"));
        assert!(list.contains("- environment_and_climate — Environment & Climate"));
        assert!(list.contains("- wash — WASH (Water, Sanitation & Hygiene)"));
    }

    #[test]
    fn siblings_are_empty_so_a_search_widens_to_everything_instead() {
        assert!(sibling_slugs("health").is_empty());
        assert!(sibling_slugs("not_a_category").is_empty());
    }
}
