//! Keyboard step: the layout drives the default language and timezone.
//! The RTC is always kept in UTC.

use crate::error::InstallError;
use crate::frontend::{Frontend, ask_valid, choose_from};

/// Locale used when the layout is not in the table.
pub const FALLBACK_LOCALE: &str = "en_US.UTF-8";

/// Language, timezone and country of one keyboard layout.
struct Layout {
    code: &'static str,
    locale: &'static str,
    /// Zones of the layout's country; more than one means "ask".
    zones: &'static [&'static str],
}

const US_ZONES: &[&str] = &[
    "America/New_York",
    "America/Chicago",
    "America/Denver",
    "America/Phoenix",
    "America/Los_Angeles",
    "America/Anchorage",
    "Pacific/Honolulu",
];

const LAYOUTS: &[Layout] = &[
    Layout {
        code: "it",
        locale: "it_IT.UTF-8",
        zones: &["Europe/Rome"],
    },
    Layout {
        code: "de",
        locale: "de_DE.UTF-8",
        zones: &["Europe/Berlin"],
    },
    Layout {
        code: "fr",
        locale: "fr_FR.UTF-8",
        zones: &["Europe/Paris"],
    },
    Layout {
        code: "es",
        locale: "es_ES.UTF-8",
        zones: &["Europe/Madrid", "Atlantic/Canary", "Africa/Ceuta"],
    },
    Layout {
        code: "pt",
        locale: "pt_PT.UTF-8",
        zones: &["Europe/Lisbon", "Atlantic/Azores", "Atlantic/Madeira"],
    },
    Layout {
        code: "gb",
        locale: "en_GB.UTF-8",
        zones: &["Europe/London"],
    },
    Layout {
        code: "uk",
        locale: "en_GB.UTF-8",
        zones: &["Europe/London"],
    },
    // `us` names a country but no single zone: asked explicitly.
    Layout {
        code: "us",
        locale: "en_US.UTF-8",
        zones: US_ZONES,
    },
    Layout {
        code: "ch",
        locale: "de_CH.UTF-8",
        zones: &["Europe/Zurich"],
    },
    Layout {
        code: "at",
        locale: "de_AT.UTF-8",
        zones: &["Europe/Vienna"],
    },
    Layout {
        code: "be",
        locale: "nl_BE.UTF-8",
        zones: &["Europe/Brussels"],
    },
    Layout {
        code: "nl",
        locale: "nl_NL.UTF-8",
        zones: &["Europe/Amsterdam"],
    },
    Layout {
        code: "se",
        locale: "sv_SE.UTF-8",
        zones: &["Europe/Stockholm"],
    },
    Layout {
        code: "no",
        locale: "nb_NO.UTF-8",
        zones: &["Europe/Oslo"],
    },
    Layout {
        code: "dk",
        locale: "da_DK.UTF-8",
        zones: &["Europe/Copenhagen"],
    },
    Layout {
        code: "fi",
        locale: "fi_FI.UTF-8",
        zones: &["Europe/Helsinki"],
    },
    Layout {
        code: "pl",
        locale: "pl_PL.UTF-8",
        zones: &["Europe/Warsaw"],
    },
    Layout {
        code: "cz",
        locale: "cs_CZ.UTF-8",
        zones: &["Europe/Prague"],
    },
    Layout {
        code: "ru",
        locale: "ru_RU.UTF-8",
        zones: &[
            "Europe/Moscow",
            "Europe/Kaliningrad",
            "Europe/Samara",
            "Asia/Yekaterinburg",
            "Asia/Novosibirsk",
            "Asia/Vladivostok",
        ],
    },
    Layout {
        code: "jp",
        locale: "ja_JP.UTF-8",
        zones: &["Asia/Tokyo"],
    },
    Layout {
        code: "br",
        locale: "pt_BR.UTF-8",
        zones: &[
            "America/Sao_Paulo",
            "America/Manaus",
            "America/Fortaleza",
            "America/Noronha",
        ],
    },
    Layout {
        code: "ca",
        locale: "en_CA.UTF-8",
        zones: &[
            "America/Toronto",
            "America/Vancouver",
            "America/Edmonton",
            "America/Winnipeg",
            "America/Halifax",
            "America/St_Johns",
        ],
    },
];

/// Keyboard, language and timezone answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Locale {
    pub keyboard: String,
    pub language: String,
    pub timezone: String,
    /// The hardware clock always runs in UTC.
    pub rtc_utc: bool,
}

/// Layout codes known to the table.
pub fn layout_codes() -> Vec<&'static str> {
    LAYOUTS.iter().map(|l| l.code).collect()
}

fn find(code: &str) -> Option<&'static Layout> {
    LAYOUTS.iter().find(|l| l.code == code)
}

/// Locale derived from a layout, `FALLBACK_LOCALE` when unknown.
pub fn locale_for(code: &str) -> &'static str {
    find(code).map_or(FALLBACK_LOCALE, |l| l.locale)
}

/// Candidate zones for a layout; empty when the layout is unknown.
pub fn zones_for(code: &str) -> &'static [&'static str] {
    find(code).map_or(&[], |l| l.zones)
}

/// Accepts `Area/City` shaped names (`Europe/Rome`, `America/Argentina/Salta`).
pub fn validate_timezone(tz: &str) -> Result<(), String> {
    let ok = tz.split('/').count() >= 2
        && tz.split('/').all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+'))
        });
    ok.then_some(())
        .ok_or_else(|| format!("not a timezone name: {tz} (expected Area/City)"))
}

/// Accepts `ll_CC.UTF-8` shaped locales.
pub fn validate_locale(locale: &str) -> Result<(), String> {
    let ok = locale.strip_suffix(".UTF-8").is_some_and(|base| {
        base.split_once('_').is_some_and(|(l, c)| {
            (2..=3).contains(&l.len())
                && l.chars().all(|c| c.is_ascii_lowercase())
                && c.len() == 2
                && c.chars().all(|c| c.is_ascii_uppercase())
        })
    });
    ok.then_some(())
        .ok_or_else(|| format!("not a UTF-8 locale: {locale} (expected e.g. it_IT.UTF-8)"))
}

/// Pick the timezone: automatic for a one-zone country, asked otherwise.
pub fn ask_timezone(fe: &mut dyn Frontend, code: &str) -> Result<String, InstallError> {
    match zones_for(code) {
        [only] => Ok((*only).to_owned()),
        [] => ask_valid(fe, "Timezone (Area/City)", &validate_timezone),
        zones => {
            let i = choose_from(fe, "Timezone", zones)?;
            Ok(zones[i].to_owned())
        }
    }
}

/// Ask the layout, then derive language and timezone from it.
pub fn run(fe: &mut dyn Frontend) -> Result<Locale, InstallError> {
    let codes = layout_codes();
    let keyboard = codes[choose_from(fe, "Keyboard layout", &codes)?].to_owned();
    let timezone = ask_timezone(fe, &keyboard)?;
    Ok(Locale {
        language: locale_for(&keyboard).to_owned(),
        keyboard,
        timezone,
        rtc_utc: true,
    })
}

/// Redo language and timezone only, from the summary.
pub fn edit_regional(fe: &mut dyn Frontend, current: &Locale) -> Result<Locale, InstallError> {
    let language = ask_valid(fe, "Language (locale)", &validate_locale)?;
    let timezone = ask_valid(fe, "Timezone (Area/City)", &validate_timezone)?;
    Ok(Locale {
        language,
        timezone,
        ..current.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Script;

    #[test]
    fn single_zone_country_is_automatic() {
        let mut fe = Script::new().pick(0); // "it"
        let l = run(&mut fe).unwrap();
        assert_eq!(l.keyboard, "it");
        assert_eq!(l.language, "it_IT.UTF-8");
        assert_eq!(l.timezone, "Europe/Rome");
        assert!(l.rtc_utc);
        fe.assert_done();
    }

    #[test]
    fn multi_zone_layout_asks_the_timezone() {
        let idx = layout_codes().iter().position(|c| *c == "us").unwrap();
        let mut fe = Script::new().pick(idx).pick(1);
        let l = run(&mut fe).unwrap();
        assert_eq!(l.language, "en_US.UTF-8");
        assert_eq!(l.timezone, "America/Chicago");
        fe.assert_done();
    }

    #[test]
    fn unknown_layout_asks_free_text_and_falls_back() {
        assert_eq!(locale_for("xx"), FALLBACK_LOCALE);
        let mut fe = Script::new().text("nonsense").text("Asia/Seoul");
        assert_eq!(ask_timezone(&mut fe, "xx").unwrap(), "Asia/Seoul");
    }

    #[test]
    fn table_covers_the_required_layouts() {
        for c in [
            "it", "de", "fr", "es", "pt", "gb", "uk", "us", "ch", "at", "be", "nl", "se", "no",
            "dk", "fi", "pl", "cz", "ru", "jp", "br", "ca",
        ] {
            assert!(find(c).is_some(), "{c}");
        }
        assert!(
            layout_codes()
                .iter()
                .all(|c| validate_locale(locale_for(c)).is_ok())
        );
    }

    #[test]
    fn validators() {
        assert!(validate_timezone("Europe/Rome").is_ok());
        assert!(validate_timezone("America/Argentina/Salta").is_ok());
        assert!(validate_timezone("Rome").is_err());
        assert!(validate_timezone("../etc").is_err());
        assert!(validate_locale("it_IT.UTF-8").is_ok());
        assert!(validate_locale("it_IT").is_err());
    }

    #[test]
    fn regional_edit_keeps_keyboard() {
        let cur = Locale {
            keyboard: "it".into(),
            language: "it_IT.UTF-8".into(),
            timezone: "Europe/Rome".into(),
            rtc_utc: true,
        };
        let mut fe = Script::new().text("en_GB.UTF-8").text("Europe/London");
        let l = edit_regional(&mut fe, &cur).unwrap();
        assert_eq!(
            (l.keyboard.as_str(), l.language.as_str()),
            ("it", "en_GB.UTF-8")
        );
        assert_eq!(l.timezone, "Europe/London");
    }
}
