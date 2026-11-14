//! A repeat summary, rendered end to end from `strings/`, is grammatical in every language
//! (task 3564db06).
//!
//! This is the acceptance line of the task: **German and Japanese repeat summaries are grammatical
//! in a fixture test**. Everything else in this repository checks the summary one layer at a time —
//! [`astrid_rules::rows::repeat`]'s unit tests check which *parts* a pattern produces, and
//! `strings_catalog.rs` checks that every part's key has *words* in twelve languages. Neither can
//! see the failure this closes, because it only exists where the two meet:
//!
//! > "Every 1 weeks", "Alle 1 Wochen", "on the 3th Tuesday"
//!
//! Correct parts, correct catalog, ungrammatical sentence. So this test does the join a shell does
//! — pick the category, look up the words, substitute the arguments — and records the result in
//! `contracts/fixtures/repeat-summary-l10n.json`, where a client can replay it.
//!
//! An integration test and not a unit one, for the reason `strings_catalog.rs` gives: it reads
//! `strings/` from the repository, and `astrid-rules` builds for `wasm32-unknown-unknown` where
//! there is no filesystem.
//!
//! ## The renderer here is not a product renderer
//!
//! It handles the ICU subset this catalog actually uses — named `{placeholder}`s and a flat object
//! of CLDR categories — because each client renders natively (ICU for web, `<plurals>` for Android,
//! `.stringsdict` for Apple, one resource per category for Windows) and a fifth implementation is
//! not something to ship. It exists to make the *catalog data* assertable, which is what the
//! acceptance asks about.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use astrid_rules::model::{date, CustomRepeatingPattern, MonthWeekday, RepeatFromMode, Repeating};
use astrid_rules::plurals::{self, CountForm};
use astrid_rules::rows::repeat::{summary, SummaryPart};
use serde::Deserialize;

const FIXTURE: &str = include_str!("../../../contracts/fixtures/repeat-summary-l10n.json");

/// The three languages the fixture records, and why these three.
///
/// English because it is the one everybody can check by eye; **German** because it is a two-form
/// language whose plural is *not* English's (`Alle 2 Wochen` against `Every 2 weeks`, and an
/// ordinal that is a full stop rather than a suffix); **Japanese** because it has no plural
/// categories at all, which is the case a `count == 1` shell gets wrong in the other direction — it
/// would reach for a singular string that should not exist.
const LANGUAGES: [&str; 3] = ["en", "de", "ja"];

// ── The fixture ─────────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    /// Language tag → the whole sentence.
    rendered: BTreeMap<String, String>,
}

// ── The patterns, by the fixture's names ────────────────────────────────────────────────────────

/// Every case the fixture records, as the arguments [`summary`] takes.
///
/// Keyed by name rather than positional so the fixture stays readable, and so a case added to one
/// and not the other fails loudly rather than shifting every assertion by one.
fn cases() -> Vec<(&'static str, Vec<SummaryPart>)> {
    let custom = |pattern: CustomRepeatingPattern, from: Option<RepeatFromMode>| {
        summary(Some(Repeating::Custom), Some(&pattern), from)
    };
    let weeks = |interval: i64, weekdays: Option<Vec<String>>| CustomRepeatingPattern {
        unit: Some("weeks".into()),
        interval: Some(interval),
        weekdays,
        ..Default::default()
    };

    vec![
        // The plural cases. One is the whole point: "Every 1 weeks" is the bug.
        ("every-1-week", custom(weeks(1, None), None)),
        ("every-2-weeks", custom(weeks(2, None), None)),
        (
            "every-1-day",
            custom(
                CustomRepeatingPattern {
                    unit: Some("days".into()),
                    interval: Some(1),
                    ..Default::default()
                },
                None,
            ),
        ),
        (
            "every-5-days",
            custom(
                CustomRepeatingPattern {
                    unit: Some("days".into()),
                    interval: Some(5),
                    ..Default::default()
                },
                None,
            ),
        ),
        (
            "every-1-month",
            custom(
                CustomRepeatingPattern {
                    unit: Some("months".into()),
                    interval: Some(1),
                    ..Default::default()
                },
                None,
            ),
        ),
        (
            "every-1-year",
            custom(
                CustomRepeatingPattern {
                    unit: Some("years".into()),
                    interval: Some(1),
                    ..Default::default()
                },
                None,
            ),
        ),
        // Weekdays, and the from-due-date tail.
        (
            "every-2-weeks-mon-wed-from-due-date",
            custom(
                weeks(2, Some(vec!["monday".into(), "wednesday".into()])),
                Some(RepeatFromMode::DueDate),
            ),
        ),
        // The ordinal cases. 1st / 2nd / 3rd / 11th are the four English ordinal categories, and
        // they are all a full stop in German and all bare in Japanese.
        ("on-the-1st", custom(same_date(1), None)),
        ("on-the-2nd", custom(same_date(2), None)),
        ("on-the-3rd", custom(same_date(3), None)),
        ("on-the-11th", custom(same_date(11), None)),
        ("on-the-21st", custom(same_date(21), None)),
        ("on-the-3rd-tuesday", custom(same_weekday(3), None)),
        ("on-the-last-tuesday", custom(same_weekday(-1), None)),
        // Ends.
        (
            "ends-after-10",
            custom(
                CustomRepeatingPattern {
                    unit: Some("days".into()),
                    interval: Some(3),
                    end_condition: Some("after_occurrences".into()),
                    end_after_occurrences: Some(10),
                    ..Default::default()
                },
                None,
            ),
        ),
        (
            "ends-on-a-date",
            custom(
                CustomRepeatingPattern {
                    unit: Some("days".into()),
                    interval: Some(3),
                    end_condition: Some("until_date".into()),
                    end_until_date: date::parse("2027-03-03T00:00:00Z"),
                    ..Default::default()
                },
                None,
            ),
        ),
        // A preset, which has no count at all and so exercises no category.
        (
            "weekly-preset",
            summary(Some(Repeating::Weekly), None, None),
        ),
    ]
}

fn same_date(day: i64) -> CustomRepeatingPattern {
    CustomRepeatingPattern {
        unit: Some("months".into()),
        interval: Some(1),
        month_repeat_type: Some("same_date".into()),
        month_day: Some(day),
        ..Default::default()
    }
}

fn same_weekday(week_of_month: i64) -> CustomRepeatingPattern {
    CustomRepeatingPattern {
        unit: Some("months".into()),
        interval: Some(1),
        month_repeat_type: Some("same_weekday".into()),
        month_weekday: Some(MonthWeekday {
            weekday: "tuesday".into(),
            week_of_month,
        }),
        ..Default::default()
    }
}

// ── Rendering, the way a shell does it ──────────────────────────────────────────────────────────

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("astrid-rules lives at <root>/crates/astrid-rules")
        .to_path_buf()
}

/// One language's words, straight out of `strings/<tag>.json`.
fn words(locale: &str) -> serde_json::Map<String, serde_json::Value> {
    let path = root().join(format!("strings/{locale}.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("strings/{locale}.json: {err}"))
}

/// The words for `key`, picking the CLDR category when the value is an object of forms.
///
/// `category` is [`None`] for a key with no count. A plural key reached without one falls back to
/// `other`, which is the category every language has — a shell with nothing to count has nothing
/// to agree with.
fn message(
    table: &serde_json::Map<String, serde_json::Value>,
    locale: &str,
    key: &str,
    category: Option<&str>,
) -> String {
    let value = table
        .get(key)
        .unwrap_or_else(|| panic!("strings/{locale}.json has no {key}"));
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Object(forms) => {
            let wanted = category.unwrap_or("other");
            let form = forms.get(wanted).or_else(|| forms.get("other"));
            form.and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("strings/{locale}.json has no {wanted} form for {key}"))
                .to_string()
        }
        _ => panic!("strings/{locale}.json has neither words nor forms for {key}"),
    }
}

/// `{name}` → the value, for the ICU subset this catalog uses.
fn fill(template: &str, arguments: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (name, value) in arguments {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// One part as a phrase, the way a shell renders it: ask the core for the category, ask the catalog
/// for the words, substitute.
///
/// **This is the join the task is about.** The count is not formatted as a bare number when the part
/// says it is an ordinal — it goes through the catalog's own ordinal key, whose categories are the
/// *ordinal* ones. That is why English says "3rd" and German "3." without either client knowing a
/// rule about either language.
fn phrase(
    part: &SummaryPart,
    locale: &str,
    table: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let cardinal = part
        .count
        .filter(|_| part.form == Some(CountForm::Cardinal))
        .map(|count| plurals::category(locale, CountForm::Cardinal, count));

    let mut arguments: Vec<(&str, String)> = Vec::new();
    if let Some(count) = part.count {
        arguments.push(("count", count.to_string()));
        if part.form == Some(CountForm::Ordinal) {
            let category = plurals::category(locale, CountForm::Ordinal, count);
            let spelled = message(table, locale, plurals::ORDINAL_KEY, Some(category));
            arguments.push(("ordinal", fill(&spelled, &[("count", count.to_string())])));
        }
    }
    if !part.values.is_empty() {
        // Weekdays and months cross the wire as names the shell translates from its own table, the
        // same as today — the catalog does not carry them. Rendered as they arrive, so the sentence
        // is assertable without inventing a thirteenth table here.
        let joined = part.values.join(", ");
        arguments.push(("weekdays", joined.clone()));
        arguments.push(("weekday", joined.clone()));
        arguments.push(("month", joined));
    }
    if let Some(date) = &part.date {
        arguments.push(("date", date.clone()));
    }

    fill(&message(table, locale, part.key, cardinal), &arguments)
}

/// The whole summary, joined with a space. Joining is the shell's, and a space is what the three
/// languages here want; the point of returning parts at all is that a language wanting otherwise
/// can do otherwise.
fn render(parts: &[SummaryPart], locale: &str) -> String {
    let table = words(locale);
    parts
        .iter()
        .map(|part| phrase(part, locale, &table))
        .collect::<Vec<_>>()
        .join(" ")
}

// ── The acceptance ──────────────────────────────────────────────────────────────────────────────

/// **The acceptance line (task 3564db06).** Every recorded pattern renders exactly as the fixture
/// says, in English, German and Japanese.
#[test]
fn every_repeat_summary_reads_as_the_fixture_records_3564db06() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("the fixture parses");
    let recorded: BTreeMap<&str, &Case> = fixture
        .cases
        .iter()
        .map(|case| (case.name.as_str(), case))
        .collect();

    for (name, parts) in cases() {
        let case = recorded.get(name).unwrap_or_else(|| {
            panic!("{name} is not in contracts/fixtures/repeat-summary-l10n.json")
        });
        for locale in LANGUAGES {
            let expected = case
                .rendered
                .get(locale)
                .unwrap_or_else(|| panic!("{name} records nothing for {locale}"));
            assert_eq!(&render(&parts, locale), expected, "{name} in {locale}");
        }
    }

    assert_eq!(
        recorded.len(),
        cases().len(),
        "the fixture and this test disagree about how many cases there are"
    );
}

/// The failure that made this task: a shell's `count == 1` rule, and an English ordinal helper.
///
/// Spelled out rather than left implicit in the fixture, because the fixture is a wall of strings
/// and this is the one line of it somebody re-reading the task needs to see.
#[test]
fn the_singular_is_not_the_plural_with_a_1_in_it_3564db06() {
    let one = cases()
        .into_iter()
        .find(|(name, _)| *name == "every-1-week")
        .expect("every-1-week")
        .1;

    // English and German each collapse to their own singular. Neither says "1".
    assert_eq!(render(&one, "en"), "Every week");
    assert_eq!(render(&one, "de"), "Jede Woche");
    // Japanese has no plural categories, so it says the one thing it has — with the number, which
    // is correct there and is what a `count == 1` shell would have suppressed.
    assert_eq!(render(&one, "ja"), "1週間ごと");
}

/// German's ordinal is a full stop and Japanese has none at all, so neither can come from English's
/// `st/nd/rd/th` — which is the helper every client currently carries.
#[test]
fn the_ordinal_is_the_catalogs_not_the_shells_3564db06() {
    let on_the = |name: &str| {
        cases()
            .into_iter()
            .find(|(case, _)| *case == name)
            .unwrap_or_else(|| panic!("{name}"))
            .1
    };

    assert!(render(&on_the("on-the-3rd"), "en").ends_with("on the 3rd"));
    assert!(render(&on_the("on-the-3rd"), "de").ends_with("am 3."));
    assert!(render(&on_the("on-the-3rd"), "ja").ends_with("3日に"));

    // 11th, not 11st: English's ordinal `one` category excludes n % 100 == 11, and the core — not
    // the shell — is what knows that.
    assert!(render(&on_the("on-the-11th"), "en").ends_with("on the 11th"));
}

/// Rewrites the fixture from the current rules and the current catalog.
///
/// `cargo test -p astrid-rules --test repeat_summary_l10n -- --ignored`, then **read the diff** —
/// that is the review, and it is the whole point of recording sentences rather than asserting
/// them in code. A change here that reworded a shipped summary shows up as a changed line in a
/// file somebody has to look at.
///
/// Ignored rather than a `gen-core-contracts` arm because the renderer it needs is this test's,
/// and a fifth ICU implementation in the xtask would be a thing to maintain for one file.
#[test]
#[ignore = "writes contracts/fixtures/repeat-summary-l10n.json; run deliberately and read the diff"]
fn write_the_fixture_3564db06() {
    let cases: Vec<serde_json::Value> = cases()
        .into_iter()
        .map(|(name, parts)| {
            let rendered: serde_json::Map<String, serde_json::Value> = LANGUAGES
                .iter()
                .map(|locale| {
                    (
                        (*locale).to_string(),
                        serde_json::Value::String(render(&parts, locale)),
                    )
                })
                .collect();
            serde_json::json!({ "name": name, "rendered": rendered })
        })
        .collect();

    let file = serde_json::json!({
        "note": "GENERATED by `cargo test -p astrid-rules --test repeat_summary_l10n -- --ignored` \
                 (task 3564db06). Every repeat summary, rendered end to end from strings/.",
        "cases": cases,
    });
    let path = root().join("contracts/fixtures/repeat-summary-l10n.json");
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&file).expect("JSON")),
    )
    .unwrap_or_else(|err| panic!("cannot write {}: {err}", path.display()));
}

/// Every part that carries a count says what kind of number it is. A count with no form is the
/// state this task found the core in: the shell had to decide from a hardcoded list of keys.
#[test]
fn every_counted_part_says_whether_it_is_cardinal_or_ordinal_3564db06() {
    for (name, parts) in cases() {
        for part in parts {
            assert_eq!(
                part.count.is_some(),
                part.form.is_some(),
                "{name}: {} has a count without a form, or a form without a count",
                part.key
            );
        }
    }
}
