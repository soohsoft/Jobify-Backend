//! Canonical job-category taxonomy.
//!
//! Two levels, mirroring how the list was specified:
//! - [`CategoryGroup`] — the 8 top-level buckets
//! - [`Category`] — the 54 selectable categories
//!
//! **Slugs are the wire and database contract.** They are what the scraper
//! sends in `JobInput.category` and what `GET /jobs?category=` matches
//! against, so a variant must never be renamed without migrating stored
//! values. `label()` is the human-facing display name and may change freely.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CategoryGroup {
    AdministrationAndOperations,
    HumanitarianAndDevelopment,
    EconomicsFinanceAndLegal,
    TechnicalEngineeringAndIt,
    ResearchAndData,
    HealthAndNutrition,
    EducationAndCommunication,
    EnvironmentSecurityAndTrade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    // Administration & Operations
    ManagementAndLeadership,
    Administration,
    Operations,
    HumanResources,
    LogisticsAndSupplyChain,
    ProcurementAndPurchasing,
    CustomerServiceAndClientCare,
    SecretarialAndFrontOfficeSupport,
    AirportAndTransitManagement,
    ArchivingAndDocumentControl,

    // Humanitarian & Development (NGO)
    ProjectAndProgramManagement,
    Wash,
    ChildProtectionAndSafeguarding,
    GenderAndSocialInclusion,
    HumanRightsAndSocialWork,
    DisasterRiskManagementAndClimateResilience,
    ClusterCoordination,
    DonorRelationsAndGrantsManagement,
    CommunityDevelopmentAndGrassrootsOrganizing,

    // Economics, Finance & Legal
    Accounting,
    Finance,
    AuditingAndRiskCompliance,
    BankingAndMicrofinance,
    LegalServicesAndJudiciary,
    PublicPolicyAndPoliticalAffairs,
    BidsTendersAndContractManagement,

    // Technical, Engineering & IT
    CivilEngineeringAndInfrastructure,
    MechanicalAndElectricalEngineering,
    InformationTechnologyAndNetworking,
    SoftwareEngineeringAndWebDevelopment,
    InformationAndDatabaseManagement,
    Telecommunications,

    // Research & Data
    MonitoringAndEvaluation,
    DataCollectionAndSurveying,
    StatisticsAndDataAnalytics,
    IndependentConsultanciesAndAdvisory,

    // Health & Nutrition
    PublicHealthAndHealthcareManagement,
    ClinicalMedicineAndNursing,
    NutritionAndFoodSecurity,
    EmergencyResponse,
    MentalHealthAndPsychosocialSupport,
    PharmaceuticalsAndLabTechnology,

    // Education & Communication
    PrimaryAndSecondaryEducation,
    HigherEducationAndAcademia,
    VocationalTrainingAndCapacityBuilding,
    CommunicationsAndPublicRelations,
    MediaJournalismAndBroadcasting,
    TranslationInterpretationAndLinguistics,
    GraphicDesignArtAndCreativeMedia,
    MarketingAndAdvertising,

    // Environment, Security & Trade
    AgricultureLivestockAndVeterinarySciences,
    EnvironmentForestryAndMarineResources,
    SecuritySafetyAndRiskMitigation,
    RetailSalesAndCommercialTrade,
}

const ADMINISTRATION_AND_OPERATIONS: &[Category] = &[
    Category::ManagementAndLeadership,
    Category::Administration,
    Category::Operations,
    Category::HumanResources,
    Category::LogisticsAndSupplyChain,
    Category::ProcurementAndPurchasing,
    Category::CustomerServiceAndClientCare,
    Category::SecretarialAndFrontOfficeSupport,
    Category::AirportAndTransitManagement,
    Category::ArchivingAndDocumentControl,
];

const HUMANITARIAN_AND_DEVELOPMENT: &[Category] = &[
    Category::ProjectAndProgramManagement,
    Category::Wash,
    Category::ChildProtectionAndSafeguarding,
    Category::GenderAndSocialInclusion,
    Category::HumanRightsAndSocialWork,
    Category::DisasterRiskManagementAndClimateResilience,
    Category::ClusterCoordination,
    Category::DonorRelationsAndGrantsManagement,
    Category::CommunityDevelopmentAndGrassrootsOrganizing,
];

const ECONOMICS_FINANCE_AND_LEGAL: &[Category] = &[
    Category::Accounting,
    Category::Finance,
    Category::AuditingAndRiskCompliance,
    Category::BankingAndMicrofinance,
    Category::LegalServicesAndJudiciary,
    Category::PublicPolicyAndPoliticalAffairs,
    Category::BidsTendersAndContractManagement,
];

const TECHNICAL_ENGINEERING_AND_IT: &[Category] = &[
    Category::CivilEngineeringAndInfrastructure,
    Category::MechanicalAndElectricalEngineering,
    Category::InformationTechnologyAndNetworking,
    Category::SoftwareEngineeringAndWebDevelopment,
    Category::InformationAndDatabaseManagement,
    Category::Telecommunications,
];

const RESEARCH_AND_DATA: &[Category] = &[
    Category::MonitoringAndEvaluation,
    Category::DataCollectionAndSurveying,
    Category::StatisticsAndDataAnalytics,
    Category::IndependentConsultanciesAndAdvisory,
];

const HEALTH_AND_NUTRITION: &[Category] = &[
    Category::PublicHealthAndHealthcareManagement,
    Category::ClinicalMedicineAndNursing,
    Category::NutritionAndFoodSecurity,
    Category::EmergencyResponse,
    Category::MentalHealthAndPsychosocialSupport,
    Category::PharmaceuticalsAndLabTechnology,
];

const EDUCATION_AND_COMMUNICATION: &[Category] = &[
    Category::PrimaryAndSecondaryEducation,
    Category::HigherEducationAndAcademia,
    Category::VocationalTrainingAndCapacityBuilding,
    Category::CommunicationsAndPublicRelations,
    Category::MediaJournalismAndBroadcasting,
    Category::TranslationInterpretationAndLinguistics,
    Category::GraphicDesignArtAndCreativeMedia,
    Category::MarketingAndAdvertising,
];

const ENVIRONMENT_SECURITY_AND_TRADE: &[Category] = &[
    Category::AgricultureLivestockAndVeterinarySciences,
    Category::EnvironmentForestryAndMarineResources,
    Category::SecuritySafetyAndRiskMitigation,
    Category::RetailSalesAndCommercialTrade,
];

impl CategoryGroup {
    pub const ALL: &'static [CategoryGroup] = &[
        CategoryGroup::AdministrationAndOperations,
        CategoryGroup::HumanitarianAndDevelopment,
        CategoryGroup::EconomicsFinanceAndLegal,
        CategoryGroup::TechnicalEngineeringAndIt,
        CategoryGroup::ResearchAndData,
        CategoryGroup::HealthAndNutrition,
        CategoryGroup::EducationAndCommunication,
        CategoryGroup::EnvironmentSecurityAndTrade,
    ];

    /// Stable machine slug (wire + database contract).
    pub fn slug(self) -> &'static str {
        match self {
            CategoryGroup::AdministrationAndOperations => "administration_and_operations",
            CategoryGroup::HumanitarianAndDevelopment => "humanitarian_and_development",
            CategoryGroup::EconomicsFinanceAndLegal => "economics_finance_and_legal",
            CategoryGroup::TechnicalEngineeringAndIt => "technical_engineering_and_it",
            CategoryGroup::ResearchAndData => "research_and_data",
            CategoryGroup::HealthAndNutrition => "health_and_nutrition",
            CategoryGroup::EducationAndCommunication => "education_and_communication",
            CategoryGroup::EnvironmentSecurityAndTrade => "environment_security_and_trade",
        }
    }

    /// Human-facing display name.
    pub fn label(self) -> &'static str {
        match self {
            CategoryGroup::AdministrationAndOperations => "Administration & Operations",
            CategoryGroup::HumanitarianAndDevelopment => "Humanitarian & Development (NGO)",
            CategoryGroup::EconomicsFinanceAndLegal => "Economics, Finance & Legal",
            CategoryGroup::TechnicalEngineeringAndIt => "Technical, Engineering & IT",
            CategoryGroup::ResearchAndData => "Research & Data",
            CategoryGroup::HealthAndNutrition => "Health & Nutrition",
            CategoryGroup::EducationAndCommunication => "Education & Communication",
            CategoryGroup::EnvironmentSecurityAndTrade => "Environment, Security & Trade",
        }
    }

    /// Categories belonging to this group, in display order.
    pub fn categories(self) -> &'static [Category] {
        match self {
            CategoryGroup::AdministrationAndOperations => ADMINISTRATION_AND_OPERATIONS,
            CategoryGroup::HumanitarianAndDevelopment => HUMANITARIAN_AND_DEVELOPMENT,
            CategoryGroup::EconomicsFinanceAndLegal => ECONOMICS_FINANCE_AND_LEGAL,
            CategoryGroup::TechnicalEngineeringAndIt => TECHNICAL_ENGINEERING_AND_IT,
            CategoryGroup::ResearchAndData => RESEARCH_AND_DATA,
            CategoryGroup::HealthAndNutrition => HEALTH_AND_NUTRITION,
            CategoryGroup::EducationAndCommunication => EDUCATION_AND_COMMUNICATION,
            CategoryGroup::EnvironmentSecurityAndTrade => ENVIRONMENT_SECURITY_AND_TRADE,
        }
    }

    pub fn from_slug(slug: &str) -> Option<CategoryGroup> {
        CategoryGroup::ALL
            .iter()
            .copied()
            .find(|group| group.slug() == slug)
    }
}

/// Slugs of the OTHER categories in the same group as `slug`.
///
/// Used to widen a search that found nothing: a WASH officer's own category can be empty
/// while its group (Humanitarian & Development) has live roles, and "here is what is close"
/// beats an empty screen. Returns an empty list for an unknown slug.
pub fn sibling_slugs(slug: &str) -> Vec<String> {
    let Some(category) = Category::from_slug(slug) else {
        return Vec::new();
    };
    category
        .group()
        .categories()
        .iter()
        .filter(|other| other.slug() != category.slug())
        .map(|other| other.slug().to_string())
        .collect()
}

impl Category {
    /// Every category, in group order.
    pub const ALL: &'static [Category] = &[
        Category::ManagementAndLeadership,
        Category::Administration,
        Category::Operations,
        Category::HumanResources,
        Category::LogisticsAndSupplyChain,
        Category::ProcurementAndPurchasing,
        Category::CustomerServiceAndClientCare,
        Category::SecretarialAndFrontOfficeSupport,
        Category::AirportAndTransitManagement,
        Category::ArchivingAndDocumentControl,
        Category::ProjectAndProgramManagement,
        Category::Wash,
        Category::ChildProtectionAndSafeguarding,
        Category::GenderAndSocialInclusion,
        Category::HumanRightsAndSocialWork,
        Category::DisasterRiskManagementAndClimateResilience,
        Category::ClusterCoordination,
        Category::DonorRelationsAndGrantsManagement,
        Category::CommunityDevelopmentAndGrassrootsOrganizing,
        Category::Accounting,
        Category::Finance,
        Category::AuditingAndRiskCompliance,
        Category::BankingAndMicrofinance,
        Category::LegalServicesAndJudiciary,
        Category::PublicPolicyAndPoliticalAffairs,
        Category::BidsTendersAndContractManagement,
        Category::CivilEngineeringAndInfrastructure,
        Category::MechanicalAndElectricalEngineering,
        Category::InformationTechnologyAndNetworking,
        Category::SoftwareEngineeringAndWebDevelopment,
        Category::InformationAndDatabaseManagement,
        Category::Telecommunications,
        Category::MonitoringAndEvaluation,
        Category::DataCollectionAndSurveying,
        Category::StatisticsAndDataAnalytics,
        Category::IndependentConsultanciesAndAdvisory,
        Category::PublicHealthAndHealthcareManagement,
        Category::ClinicalMedicineAndNursing,
        Category::NutritionAndFoodSecurity,
        Category::EmergencyResponse,
        Category::MentalHealthAndPsychosocialSupport,
        Category::PharmaceuticalsAndLabTechnology,
        Category::PrimaryAndSecondaryEducation,
        Category::HigherEducationAndAcademia,
        Category::VocationalTrainingAndCapacityBuilding,
        Category::CommunicationsAndPublicRelations,
        Category::MediaJournalismAndBroadcasting,
        Category::TranslationInterpretationAndLinguistics,
        Category::GraphicDesignArtAndCreativeMedia,
        Category::MarketingAndAdvertising,
        Category::AgricultureLivestockAndVeterinarySciences,
        Category::EnvironmentForestryAndMarineResources,
        Category::SecuritySafetyAndRiskMitigation,
        Category::RetailSalesAndCommercialTrade,
    ];

    /// Stable machine slug (wire + database contract).
    pub fn slug(self) -> &'static str {
        match self {
            Category::ManagementAndLeadership => "management_and_leadership",
            Category::Administration => "administration",
            Category::Operations => "operations",
            Category::HumanResources => "human_resources",
            Category::LogisticsAndSupplyChain => "logistics_and_supply_chain",
            Category::ProcurementAndPurchasing => "procurement_and_purchasing",
            Category::CustomerServiceAndClientCare => "customer_service_and_client_care",
            Category::SecretarialAndFrontOfficeSupport => "secretarial_and_front_office_support",
            Category::AirportAndTransitManagement => "airport_and_transit_management",
            Category::ArchivingAndDocumentControl => "archiving_and_document_control",
            Category::ProjectAndProgramManagement => "project_and_program_management",
            Category::Wash => "wash",
            Category::ChildProtectionAndSafeguarding => "child_protection_and_safeguarding",
            Category::GenderAndSocialInclusion => "gender_and_social_inclusion",
            Category::HumanRightsAndSocialWork => "human_rights_and_social_work",
            Category::DisasterRiskManagementAndClimateResilience => {
                "disaster_risk_management_and_climate_resilience"
            }
            Category::ClusterCoordination => "cluster_coordination",
            Category::DonorRelationsAndGrantsManagement => "donor_relations_and_grants_management",
            Category::CommunityDevelopmentAndGrassrootsOrganizing => {
                "community_development_and_grassroots_organizing"
            }
            Category::Accounting => "accounting",
            Category::Finance => "finance",
            Category::AuditingAndRiskCompliance => "auditing_and_risk_compliance",
            Category::BankingAndMicrofinance => "banking_and_microfinance",
            Category::LegalServicesAndJudiciary => "legal_services_and_judiciary",
            Category::PublicPolicyAndPoliticalAffairs => "public_policy_and_political_affairs",
            Category::BidsTendersAndContractManagement => "bids_tenders_and_contract_management",
            Category::CivilEngineeringAndInfrastructure => "civil_engineering_and_infrastructure",
            Category::MechanicalAndElectricalEngineering => "mechanical_and_electrical_engineering",
            Category::InformationTechnologyAndNetworking => "information_technology_and_networking",
            Category::SoftwareEngineeringAndWebDevelopment => {
                "software_engineering_and_web_development"
            }
            Category::InformationAndDatabaseManagement => "information_and_database_management",
            Category::Telecommunications => "telecommunications",
            Category::MonitoringAndEvaluation => "monitoring_and_evaluation",
            Category::DataCollectionAndSurveying => "data_collection_and_surveying",
            Category::StatisticsAndDataAnalytics => "statistics_and_data_analytics",
            Category::IndependentConsultanciesAndAdvisory => {
                "independent_consultancies_and_advisory"
            }
            Category::PublicHealthAndHealthcareManagement => {
                "public_health_and_healthcare_management"
            }
            Category::ClinicalMedicineAndNursing => "clinical_medicine_and_nursing",
            Category::NutritionAndFoodSecurity => "nutrition_and_food_security",
            Category::EmergencyResponse => "emergency_response",
            Category::MentalHealthAndPsychosocialSupport => {
                "mental_health_and_psychosocial_support"
            }
            Category::PharmaceuticalsAndLabTechnology => "pharmaceuticals_and_lab_technology",
            Category::PrimaryAndSecondaryEducation => "primary_and_secondary_education",
            Category::HigherEducationAndAcademia => "higher_education_and_academia",
            Category::VocationalTrainingAndCapacityBuilding => {
                "vocational_training_and_capacity_building"
            }
            Category::CommunicationsAndPublicRelations => "communications_and_public_relations",
            Category::MediaJournalismAndBroadcasting => "media_journalism_and_broadcasting",
            Category::TranslationInterpretationAndLinguistics => {
                "translation_interpretation_and_linguistics"
            }
            Category::GraphicDesignArtAndCreativeMedia => "graphic_design_art_and_creative_media",
            Category::MarketingAndAdvertising => "marketing_and_advertising",
            Category::AgricultureLivestockAndVeterinarySciences => {
                "agriculture_livestock_and_veterinary_sciences"
            }
            Category::EnvironmentForestryAndMarineResources => {
                "environment_forestry_and_marine_resources"
            }
            Category::SecuritySafetyAndRiskMitigation => "security_safety_and_risk_mitigation",
            Category::RetailSalesAndCommercialTrade => "retail_sales_and_commercial_trade",
        }
    }

    /// Human-facing display name.
    pub fn label(self) -> &'static str {
        match self {
            Category::ManagementAndLeadership => "Management & Leadership",
            Category::Administration => "Administration",
            Category::Operations => "Operations",
            Category::HumanResources => "Human Resources",
            Category::LogisticsAndSupplyChain => "Logistics & Supply Chain",
            Category::ProcurementAndPurchasing => "Procurement & Purchasing",
            Category::CustomerServiceAndClientCare => "Customer Service & Client Care",
            Category::SecretarialAndFrontOfficeSupport => "Secretarial & Front Office Support",
            Category::AirportAndTransitManagement => "Airport & Transit Management",
            Category::ArchivingAndDocumentControl => "Archiving & Document Control",
            Category::ProjectAndProgramManagement => "Project & Program Management",
            Category::Wash => "WASH (Water, Sanitation, & Hygiene)",
            Category::ChildProtectionAndSafeguarding => "Child Protection & Safeguarding",
            Category::GenderAndSocialInclusion => "Gender & Social Inclusion",
            Category::HumanRightsAndSocialWork => "Human Rights & Social Work",
            Category::DisasterRiskManagementAndClimateResilience => {
                "Disaster Risk Management & Climate Resilience"
            }
            Category::ClusterCoordination => "Cluster Coordination",
            Category::DonorRelationsAndGrantsManagement => "Donor Relations & Grants Management",
            Category::CommunityDevelopmentAndGrassrootsOrganizing => {
                "Community Development & Grassroots Organizing"
            }
            Category::Accounting => "Accounting",
            Category::Finance => "Finance",
            Category::AuditingAndRiskCompliance => "Auditing & Risk Compliance",
            Category::BankingAndMicrofinance => "Banking & Micro-finance",
            Category::LegalServicesAndJudiciary => "Legal Services & Judiciary",
            Category::PublicPolicyAndPoliticalAffairs => "Public Policy & Political Affairs",
            Category::BidsTendersAndContractManagement => "Bids, Tenders, & Contract Management",
            Category::CivilEngineeringAndInfrastructure => "Civil Engineering & Infrastructure",
            Category::MechanicalAndElectricalEngineering => "Mechanical & Electrical Engineering",
            Category::InformationTechnologyAndNetworking => {
                "Information Technology (IT) & Networking"
            }
            Category::SoftwareEngineeringAndWebDevelopment => {
                "Software Engineering & Web Development"
            }
            Category::InformationAndDatabaseManagement => "Information & Database Management",
            Category::Telecommunications => "Telecommunications",
            Category::MonitoringAndEvaluation => "Monitoring & Evaluation (M&E)",
            Category::DataCollectionAndSurveying => "Data Collection & Surveying",
            Category::StatisticsAndDataAnalytics => "Statistics & Data Analytics",
            Category::IndependentConsultanciesAndAdvisory => "Independent Consultancies & Advisory",
            Category::PublicHealthAndHealthcareManagement => {
                "Public Health & Healthcare Management"
            }
            Category::ClinicalMedicineAndNursing => "Clinical Medicine & Nursing",
            Category::NutritionAndFoodSecurity => "Nutrition & Food Security",
            Category::EmergencyResponse => "Emergency Response",
            Category::MentalHealthAndPsychosocialSupport => "Mental Health & Psychosocial Support",
            Category::PharmaceuticalsAndLabTechnology => "Pharmaceuticals & Lab Technology",
            Category::PrimaryAndSecondaryEducation => "Primary & Secondary Education",
            Category::HigherEducationAndAcademia => "Higher Education & Academia",
            Category::VocationalTrainingAndCapacityBuilding => {
                "Vocational Training & Capacity Building"
            }
            Category::CommunicationsAndPublicRelations => "Communications & Public Relations",
            Category::MediaJournalismAndBroadcasting => "Media, Journalism, & Broadcasting",
            Category::TranslationInterpretationAndLinguistics => {
                "Translation, Interpretation, & Linguistics"
            }
            Category::GraphicDesignArtAndCreativeMedia => "Graphic Design, Art, & Creative Media",
            Category::MarketingAndAdvertising => "Marketing & Advertising",
            Category::AgricultureLivestockAndVeterinarySciences => {
                "Agriculture, Livestock, & Veterinary Sciences"
            }
            Category::EnvironmentForestryAndMarineResources => {
                "Environment, Forestry, & Marine Resources"
            }
            Category::SecuritySafetyAndRiskMitigation => "Security, Safety, & Risk Mitigation",
            Category::RetailSalesAndCommercialTrade => "Retail, Sales, & Commercial Trade",
        }
    }

    pub fn group(self) -> CategoryGroup {
        match self {
            Category::ManagementAndLeadership
            | Category::Administration
            | Category::Operations
            | Category::HumanResources
            | Category::LogisticsAndSupplyChain
            | Category::ProcurementAndPurchasing
            | Category::CustomerServiceAndClientCare
            | Category::SecretarialAndFrontOfficeSupport
            | Category::AirportAndTransitManagement
            | Category::ArchivingAndDocumentControl => CategoryGroup::AdministrationAndOperations,

            Category::ProjectAndProgramManagement
            | Category::Wash
            | Category::ChildProtectionAndSafeguarding
            | Category::GenderAndSocialInclusion
            | Category::HumanRightsAndSocialWork
            | Category::DisasterRiskManagementAndClimateResilience
            | Category::ClusterCoordination
            | Category::DonorRelationsAndGrantsManagement
            | Category::CommunityDevelopmentAndGrassrootsOrganizing => {
                CategoryGroup::HumanitarianAndDevelopment
            }

            Category::Accounting
            | Category::Finance
            | Category::AuditingAndRiskCompliance
            | Category::BankingAndMicrofinance
            | Category::LegalServicesAndJudiciary
            | Category::PublicPolicyAndPoliticalAffairs
            | Category::BidsTendersAndContractManagement => CategoryGroup::EconomicsFinanceAndLegal,

            Category::CivilEngineeringAndInfrastructure
            | Category::MechanicalAndElectricalEngineering
            | Category::InformationTechnologyAndNetworking
            | Category::SoftwareEngineeringAndWebDevelopment
            | Category::InformationAndDatabaseManagement
            | Category::Telecommunications => CategoryGroup::TechnicalEngineeringAndIt,

            Category::MonitoringAndEvaluation
            | Category::DataCollectionAndSurveying
            | Category::StatisticsAndDataAnalytics
            | Category::IndependentConsultanciesAndAdvisory => CategoryGroup::ResearchAndData,

            Category::PublicHealthAndHealthcareManagement
            | Category::ClinicalMedicineAndNursing
            | Category::NutritionAndFoodSecurity
            | Category::EmergencyResponse
            | Category::MentalHealthAndPsychosocialSupport
            | Category::PharmaceuticalsAndLabTechnology => CategoryGroup::HealthAndNutrition,

            Category::PrimaryAndSecondaryEducation
            | Category::HigherEducationAndAcademia
            | Category::VocationalTrainingAndCapacityBuilding
            | Category::CommunicationsAndPublicRelations
            | Category::MediaJournalismAndBroadcasting
            | Category::TranslationInterpretationAndLinguistics
            | Category::GraphicDesignArtAndCreativeMedia
            | Category::MarketingAndAdvertising => CategoryGroup::EducationAndCommunication,

            Category::AgricultureLivestockAndVeterinarySciences
            | Category::EnvironmentForestryAndMarineResources
            | Category::SecuritySafetyAndRiskMitigation
            | Category::RetailSalesAndCommercialTrade => CategoryGroup::EnvironmentSecurityAndTrade,
        }
    }

    pub fn from_slug(slug: &str) -> Option<Category> {
        Category::ALL.iter().copied().find(|c| c.slug() == slug)
    }

    /// `slug - Label` lines for an LLM prompt, so the model selects from the real
    /// taxonomy instead of inventing slugs that get dropped on the way in.
    pub fn prompt_list() -> String {
        Category::ALL
            .iter()
            .map(|category| format!("{} - {}", category.slug(), category.label()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn taxonomy_is_8_groups_and_53_categories() {
        assert_eq!(CategoryGroup::ALL.len(), 8);
        assert_eq!(Category::ALL.len(), 54);
    }

    #[test]
    fn group_slugs_are_unique_and_parse_back() {
        let mut seen = HashSet::new();
        for group in CategoryGroup::ALL {
            assert!(
                seen.insert(group.slug()),
                "duplicate group slug {}",
                group.slug()
            );
            assert_eq!(CategoryGroup::from_slug(group.slug()), Some(*group));
            assert!(!group.label().is_empty());
        }
        assert_eq!(CategoryGroup::from_slug("nope"), None);
    }

    #[test]
    fn category_slugs_are_unique_and_parse_back() {
        let mut seen = HashSet::new();
        for category in Category::ALL {
            assert!(
                seen.insert(category.slug()),
                "duplicate category slug {}",
                category.slug()
            );
            assert_eq!(Category::from_slug(category.slug()), Some(*category));
            assert!(!category.label().is_empty());
        }
        assert_eq!(Category::from_slug("nope"), None);
    }

    /// `ALL` must be exactly the groups flattened, so a new variant can never
    /// be added to a group without also appearing in `ALL` (or vice versa).
    #[test]
    fn all_is_exactly_the_groups_flattened() {
        let flattened: Vec<Category> = CategoryGroup::ALL
            .iter()
            .flat_map(|group| group.categories().iter().copied())
            .collect();
        assert_eq!(flattened, Category::ALL.to_vec());
    }

    #[test]
    fn every_category_maps_to_the_group_that_lists_it() {
        for group in CategoryGroup::ALL {
            for category in group.categories() {
                assert_eq!(
                    category.group(),
                    *group,
                    "{} is listed under {} but reports {}",
                    category.slug(),
                    group.slug(),
                    category.group().slug()
                );
            }
        }
    }

    /// The hand-written `slug()` must stay in lockstep with the serde
    /// representation, which is what the scraper and API actually exchange.
    #[test]
    fn serde_slugs_match_slug_fn() {
        for group in CategoryGroup::ALL {
            let json = serde_json::to_string(group).unwrap();
            assert_eq!(json, format!("\"{}\"", group.slug()));
        }
        for category in Category::ALL {
            let json = serde_json::to_string(category).unwrap();
            assert_eq!(json, format!("\"{}\"", category.slug()));
            let parsed: Category = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, *category);
        }
    }
}
