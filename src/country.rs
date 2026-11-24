//! Which country a vacancy is in, derived once at ingest from its `location` text.
//!
//! The product is Somalia-wide, and a member in Somalia must not be shown a Djibouti
//! listing just because the same board carries both. The job document therefore stores a
//! short ISO-ish code (`so`, `dj`, `et`, `ke`, `ug`, …) or `null` when the text names no
//! place this module knows, and every list filters on it.
//!
//! Two rules matter for correctness:
//!
//! - **Somaliland and Puntland are Somalia.** Hargeisa, Garowe, Bosaso and every region
//!   name resolve to `so`; treating them as unknown is how a Somali member's feed emptied.
//! - **Somali place names win over a foreign one in the same string.** "Mogadishu and
//!   Nairobi" is a Somali posting that also mentions Kenya, so it resolves to `so`. A
//!   string that only names a foreign country ("Jigjiga, Ethiopia") resolves to that
//!   country, so it is excluded from the Somali default.
//!
//! The mapping is deliberately curated rather than a geocoder: a wrong answer here silently
//! hides a job, so an unrecognised location stays `null` instead of being guessed at.

/// The default country for every list, and the one the product is built around.
pub const SOMALIA: &str = "so";

/// `?country=all` — the escape hatch that turns the filter off.
pub const ALL_COUNTRIES: &str = "all";

/// Cities, regions and spellings that mean Somalia. Both the English and the Somali
/// spelling of each place is listed because the sources use both ("Mogadishu"/"Muqdisho",
/// "Garowe"/"Garoowe"), and near-spellings are listed rather than guessed at.
const SOMALI_PLACES: &[&str] = &[
    // The country itself.
    "somalia",
    "soomaaliya",
    "somali republic",
    // Administrative regions / states, which appear as a location on their own.
    "somaliland",
    "puntland",
    "jubaland",
    "galmudug",
    "hirshabelle",
    // The state the sources call "South West" (the literal name is on the task list, and
    // some boards store only that).
    "south west",
    "southwest",
    "south west state",
    "banadir",
    "benadir",
    // Cities and towns, both spellings where they differ.
    "mogadishu",
    "muqdisho",
    "hargeisa",
    "hargeysa",
    "bosaso",
    "boosaaso",
    "kismayo",
    "kismaayo",
    "baidoa",
    "baydhabo",
    "galkayo",
    "gaalkacyo",
    "garowe",
    "garoowe",
    "beledweyne",
    "belet weyne",
    "dhuusamareeb",
    "dhusamareeb",
    "dhusamareb",
    "jowhar",
    "joohar",
    "marka",
    "merka",
    "baraawe",
    "brava",
    "berbera",
    "burao",
    "burco",
    "las anod",
    "laascaanood",
    "lascanood",
    "lasanod",
    "baardheere",
    "bardhere",
    "afgooye",
    "afgoye",
    "hudur",
    "dhobley",
    "dhoobley",
    "dobley",
    "jamaame",
    "jilib",
    "guriel",
    "sheekh",
    "heliwa",
    "baledogle",
    "balad",
    "qardho",
    "eyl",
    "lasqoray",
    "isle of somalia",
];

/// Places that are NOT Somalia, with the code each one implies. Matched only when no Somali
/// place appears in the same string (see `from_location`).
const FOREIGN_PLACES: &[(&str, &str)] = &[
    // Neighbours — the ones that actually appear on the Somali boards.
    ("djibouti", "dj"),
    ("jibouti", "dj"),
    ("tadjourah", "dj"),
    ("ethiopia", "et"),
    ("ethiopian", "et"),
    ("addis ababa", "et"),
    ("addis", "et"),
    ("jigjiga", "et"),
    ("jijiga", "et"),
    ("harar", "et"),
    ("dire dawa", "et"),
    ("kenya", "ke"),
    ("kenyan", "ke"),
    ("nairobi", "ke"),
    ("mombasa", "ke"),
    ("kisumu", "ke"),
    ("uganda", "ug"),
    ("ugandan", "ug"),
    ("kampala", "ug"),
    ("tanzania", "tz"),
    ("dar es salaam", "tz"),
    ("rwanda", "rw"),
    ("kigali", "rw"),
    ("burundi", "bi"),
    ("south sudan", "ss"),
    ("sudan", "sd"),
    ("khartoum", "sd"),
    ("egypt", "eg"),
    ("cairo", "eg"),
    // Gulf and the wider region, which appear on UN and INGO postings.
    ("yemen", "ye"),
    ("sanaa", "ye"),
    ("aden", "ye"),
    ("saudi arabia", "sa"),
    ("riyadh", "sa"),
    ("dubai", "ae"),
    ("abu dhabi", "ae"),
    ("united arab emirates", "ae"),
    ("qatar", "qa"),
    ("doha", "qa"),
    ("kuwait", "kw"),
    ("jordan", "jo"),
    ("amman", "jo"),
    ("lebanon", "lb"),
    ("beirut", "lb"),
    ("iraq", "iq"),
    ("baghdad", "iq"),
    ("syria", "sy"),
    ("damascus", "sy"),
    ("turkey", "tr"),
    ("ankara", "tr"),
    ("istanbul", "tr"),
    // Donor and HQ countries.
    ("united kingdom", "gb"),
    ("london", "gb"),
    ("united states", "us"),
    ("washington", "us"),
    ("new york", "us"),
    ("canada", "ca"),
    ("ottawa", "ca"),
    ("switzerland", "ch"),
    ("geneva", "ch"),
    ("belgium", "be"),
    ("brussels", "be"),
    ("netherlands", "nl"),
    ("the hague", "nl"),
    ("amsterdam", "nl"),
    ("germany", "de"),
    ("berlin", "de"),
    ("france", "fr"),
    ("paris", "fr"),
    ("italy", "it"),
    ("rome", "it"),
    ("spain", "es"),
    ("madrid", "es"),
    ("sweden", "se"),
    ("stockholm", "se"),
    ("norway", "no"),
    ("oslo", "no"),
    ("denmark", "dk"),
    ("copenhagen", "dk"),
    ("poland", "pl"),
    ("warsaw", "pl"),
    ("ukraine", "ua"),
    ("russia", "ru"),
    ("moscow", "ru"),
    // Further afield, same reason: a location is either in Somalia or it is not.
    ("australia", "au"),
    ("japan", "jp"),
    ("tokyo", "jp"),
    ("china", "cn"),
    ("beijing", "cn"),
    ("india", "in"),
    ("new delhi", "in"),
    ("pakistan", "pk"),
    ("islamabad", "pk"),
    ("bangladesh", "bd"),
    ("dhaka", "bd"),
    ("afghanistan", "af"),
    ("kabul", "af"),
    ("nigeria", "ng"),
    ("abuja", "ng"),
    ("ghana", "gh"),
    ("accra", "gh"),
    ("senegal", "sn"),
    ("dakar", "sn"),
    ("south africa", "za"),
    ("johannesburg", "za"),
];

/// Lowercase alphanumeric words separated by single spaces, so a place name can be matched
/// as whole words rather than as a substring ("Aden" is not inside "Braden").
fn normalise(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_space = false;
    for character in value.chars() {
        if character.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            for lower in character.to_lowercase() {
                out.push(lower);
            }
        } else {
            pending_space = true;
        }
    }
    out
}

fn contains_place(text: &str, place: &str) -> bool {
    // `text` and `place` are both normalised, so padding with a space on each side makes a
    // single-word and a multi-word needle the same test.
    format!(" {text} ").contains(&format!(" {place} "))
}

/// The country a location text belongs to, or `None` when it names no place this module
/// knows. An unrecognised location is never guessed at.
pub fn from_location(location: Option<&str>) -> Option<&'static str> {
    let text = normalise(location?);
    if text.is_empty() {
        return None;
    }
    // Somali names first: "Mogadishu and Nairobi" is a Somali posting, and the same string
    // naming a foreign city must not move it abroad.
    if SOMALI_PLACES
        .iter()
        .any(|place| contains_place(&text, place))
    {
        return Some(SOMALIA);
    }
    FOREIGN_PLACES
        .iter()
        .find(|(place, _)| contains_place(&text, place))
        .map(|(_, code)| *code)
}

/// The country a list defaults to for one member: their own location when it maps to a
/// country, Somalia otherwise. Every account in the product that has no location is a
/// Somalia account, which is why `so` is the fallback rather than "no filter".
pub fn default_for_member(location: Option<&str>) -> &'static str {
    from_location(location).unwrap_or(SOMALIA)
}

/// True when the text names a country that is not Somalia. Used for a job whose `country`
/// was never derived: such a row is kept in the Somali default ONLY when its location says
/// nothing foreign, because showing a Djibouti job to a Somali member is the failure this
/// whole module exists to prevent.
pub fn is_foreign_looking(location: Option<&str>) -> bool {
    matches!(from_location(location), Some(code) if code != SOMALIA)
}

#[cfg(test)]
mod tests {
    use super::{ALL_COUNTRIES, SOMALIA, default_for_member, from_location, is_foreign_looking};

    #[test]
    fn every_somali_city_and_region_resolves_to_somalia() {
        // The exact list the Somalia default has to cover, including the alternative
        // spellings the boards use.
        let cities = [
            ("Mogadishu", "so"),
            ("Muqdisho", "so"),
            ("Hargeisa", "so"),
            ("Hargeysa", "so"),
            ("Bosaso", "so"),
            ("Boosaaso", "so"),
            ("Kismayo", "so"),
            ("Kismaayo", "so"),
            ("Baidoa", "so"),
            ("Baydhabo", "so"),
            ("Galkayo", "so"),
            ("Gaalkacyo", "so"),
            ("Garowe", "so"),
            ("Garoowe", "so"),
            ("Beledweyne", "so"),
            ("Dhuusamareeb", "so"),
            ("Jowhar", "so"),
            ("Marka", "so"),
            ("Baraawe", "so"),
            ("Berbera", "so"),
            ("Burao", "so"),
            ("Burco", "so"),
            ("Las Anod", "so"),
            ("Laascaanood", "so"),
            ("Baardheere", "so"),
            ("Afgooye", "so"),
            ("Puntland", "so"),
            ("Somaliland", "so"),
            ("Jubaland", "so"),
            ("Galmudug", "so"),
            ("Hirshabelle", "so"),
            ("South West", "so"),
            ("Banadir", "so"),
            ("Somalia", "so"),
            ("Soomaaliya", "so"),
        ];
        for (place, code) in cities {
            assert_eq!(from_location(Some(place)), Some(code), "{place}");
        }
    }

    #[test]
    fn a_city_inside_a_country_string_keeps_the_city_country() {
        // The shapes the scraper actually stores.
        assert_eq!(from_location(Some("Mogadishu, Somalia")), Some(SOMALIA));
        assert_eq!(from_location(Some("Hargeisa, Somaliland")), Some(SOMALIA));
        assert_eq!(from_location(Some("Garoowe, Somalia")), Some(SOMALIA));
        assert_eq!(
            from_location(Some("Baidoa (Baydhabo), Somalia")),
            Some(SOMALIA)
        );
        assert_eq!(
            from_location(Some("Heliwa, Mogadishu, Somalia")),
            Some(SOMALIA)
        );
        assert_eq!(from_location(Some("Somalia & Somaliland")), Some(SOMALIA));
        // A Somali posting that also names a foreign city stays Somali.
        assert_eq!(from_location(Some("Mogadishu and Nairobi")), Some(SOMALIA));
    }

    #[test]
    fn foreign_countries_resolve_to_their_own_code() {
        assert_eq!(from_location(Some("Djibouti")), Some("dj"));
        assert_eq!(from_location(Some("Tadjourah, Djibouti")), Some("dj"));
        assert_eq!(from_location(Some("Ethiopia")), Some("et"));
        assert_eq!(from_location(Some("Addis Ababa")), Some("et"));
        assert_eq!(from_location(Some("Jigjiga, Ethiopia")), Some("et"));
        assert_eq!(from_location(Some("Kenya")), Some("ke"));
        assert_eq!(from_location(Some("Nairobi")), Some("ke"));
        assert_eq!(from_location(Some("Kampala, Uganda")), Some("ug"));
        assert_eq!(from_location(Some("Cairo, Egypt")), Some("eg"));
    }

    #[test]
    fn an_unknown_or_empty_location_is_null_not_a_guess() {
        assert_eq!(from_location(None), None);
        assert_eq!(from_location(Some("")), None);
        assert_eq!(from_location(Some("   ")), None);
        assert_eq!(from_location(Some("Multiple Locations")), None);
        assert_eq!(from_location(Some("East Africa")), None);
        assert_eq!(from_location(Some("Remote")), None);
    }

    #[test]
    fn a_member_defaults_to_their_own_country_and_somalia_without_one() {
        assert_eq!(default_for_member(None), SOMALIA);
        assert_eq!(default_for_member(Some("Kismayo, Mogadishu")), SOMALIA);
        assert_eq!(default_for_member(Some("Hargeisa, Somaliland")), SOMALIA);
        assert_eq!(default_for_member(Some("Djibouti")), "dj");
        assert_eq!(default_for_member(Some("Nairobi")), "ke");
    }

    #[test]
    fn foreign_looking_is_about_the_place_not_about_being_unknown() {
        assert!(is_foreign_looking(Some("Djibouti")));
        assert!(is_foreign_looking(Some("Addis Ababa")));
        assert!(!is_foreign_looking(Some("Mogadishu")));
        assert!(!is_foreign_looking(Some("Mogadishu and Nairobi")));
        assert!(!is_foreign_looking(Some("East Africa")));
        assert!(!is_foreign_looking(None));
    }

    #[test]
    fn the_all_escape_hatch_is_its_own_sentinel() {
        assert_ne!(ALL_COUNTRIES, SOMALIA);
        assert_eq!(ALL_COUNTRIES, "all");
    }
}
