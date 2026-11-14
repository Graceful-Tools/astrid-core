//! Which CLDR category a number selects, in each of the twelve languages (task 3564db06).
//!
//! A repeat summary comes back as parts carrying numbers — `repeat.every_n_weeks` with 2,
//! `repeat.on_nth_weekday` with 3 — and the catalog holds a form per CLDR category. Something has
//! to say *which* category. Today every client says it for itself, and says it wrong:
//! astrid-windows' `Converters.cs` tests `count == 1` and calls an `Ordinal()` helper that appends
//! `st`/`nd`/`rd`/`th`. Both are English rules wearing no label. German wants "3." and Japanese
//! wants no agreement at all, and neither falls out of `count == 1`.
//!
//! This is a rule — pure, total, and the same answer on every platform — so it lives here rather
//! than four times over. The core still returns no words (rule 10): a category is not a word, it is
//! the name of the row to look the word up in.
//!
//! ## The rules are CLDR's, transcribed
//!
//! Each arm below is the *integer* reading of that language's rule in the CLDR plural-rules data.
//! Transcribed rather than depended on, because the alternative is pulling ICU into a crate that
//! has to build for `wasm32-unknown-unknown` and that another task is actively trying to shrink —
//! for twelve languages whose rules are a dozen lines.
//!
//! Only the integer rules, because every number this core counts is one: an interval, a day of the
//! month, a week of the month, a number of occurrences. The fractional operands (`v`, `f`, `t`)
//! are therefore zero throughout, which is what makes the transcription short enough to read.
//!
//! An unknown language gets `other`, the one category every language has. A tag this build has
//! never met is a client newer than this core, and `other` is a word rather than a crash.

use serde::Serialize;

/// What a number in a message is counting — and so which of CLDR's two sets of rules applies.
///
/// They are genuinely different rules, not two uses of one: English's *cardinal* 3 is `other`
/// ("3 weeks") while its *ordinal* 3 is `few` ("3rd"). Nothing can pick the right form without
/// being told which question is being asked, and the arm that chose the key already knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum CountForm {
    /// How many: "every 2 weeks", "10 times".
    Cardinal,
    /// Which one in order: "the 3rd Tuesday", "on the 21st".
    Ordinal,
}

/// The catalog key that spells a number as an ordinal in the local language.
///
/// It is a plural key whose categories are the **ordinal** ones, so English holds four forms
/// (`{count}st`, `{count}nd`, `{count}rd`, `{count}th`), German one (`{count}.`) and Japanese one
/// (`{count}`). `repeat.on_day_of_month` then interpolates the result as its `{ordinal}` argument,
/// which is why that key needs no change: what moved is where "3rd" comes from.
pub const ORDINAL_KEY: &str = "format.ordinal";

/// Every category any rule below can return. The catalog gate reads it to check a plural key's
/// forms are categories rather than typos.
pub const CATEGORIES: [&str; 5] = ["one", "two", "few", "many", "other"];

/// The CLDR category `count` selects for `locale`.
///
/// `locale` is a tag from `strings/locales.json` (`en`, `de`, `zh-CN`, …). The language subtag is
/// what decides — `zh-CN` and `zh-TW` have one rule between them — so a client passing `de-DE` or
/// `pt-BR` is answered rather than falling through to `other`.
///
/// Negative counts are read by their absolute value, which is what CLDR's `n` operand means. No
/// repeat produces one, but "the -1st Tuesday" is a worse answer than "the 1st".
pub fn category(locale: &str, form: CountForm, count: i64) -> &'static str {
    let language = locale
        .split(['-', '_'])
        .next()
        .unwrap_or(locale)
        .to_ascii_lowercase();
    let n = count.unsigned_abs();
    match form {
        CountForm::Cardinal => cardinal(&language, n),
        CountForm::Ordinal => ordinal(&language, n),
    }
}

/// CLDR cardinal rules, integers only.
fn cardinal(language: &str, n: u64) -> &'static str {
    // A nonzero multiple of a million. Three of the twelve give it its own form, and while no
    // repeat interval will ever reach it, leaving it out would make this function wrong rather
    // than bounded.
    let millions = n != 0 && n.is_multiple_of(1_000_000);
    match language {
        // One form. Agreement is not a thing these languages do to a noun by number.
        "ja" | "ko" | "zh" => "other",

        // One and other, the familiar two.
        "en" | "de" | "nl" => {
            if n == 1 {
                "one"
            } else {
                "other"
            }
        }

        // The same two, plus a form for millions.
        "es" | "it" => {
            if n == 1 {
                "one"
            } else if millions {
                "many"
            } else {
                "other"
            }
        }

        // French and Portuguese count zero as singular — "0 semaine", not "0 semaines".
        "fr" | "pt" => {
            if n <= 1 {
                "one"
            } else if millions {
                "many"
            } else {
                "other"
            }
        }

        // Russian: three forms, decided by the last digit except in the teens.
        "ru" => {
            let last = n % 10;
            let teens = (11..=14).contains(&(n % 100));
            if last == 1 && !teens {
                "one"
            } else if (2..=4).contains(&last) && !teens {
                "few"
            } else {
                "many"
            }
        }

        _ => "other",
    }
}

/// CLDR ordinal rules, integers only.
///
/// Far fewer languages inflect an ordinal than inflect a plural: nine of the twelve have one form,
/// which is exactly why a shell that owns an English `st`/`nd`/`rd`/`th` helper looks right until
/// somebody switches language.
fn ordinal(language: &str, n: u64) -> &'static str {
    match language {
        // English's four: 1st, 2nd, 3rd, 4th — and 11th, 12th, 13th, which is the whole reason
        // this is a rule and not a lookup of the last digit.
        "en" => {
            let last = n % 10;
            let teens = (11..=13).contains(&(n % 100));
            match last {
                1 if !teens => "one",
                2 if !teens => "two",
                3 if !teens => "few",
                _ => "other",
            }
        }
        // French marks only the first: "1er", then "2e", "3e".
        "fr" => {
            if n == 1 {
                "one"
            } else {
                "other"
            }
        }
        // Italian's four exceptions take a different suffix.
        "it" => {
            if matches!(n, 8 | 11 | 80 | 800) {
                "many"
            } else {
                "other"
            }
        }
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this exists to end: a shell's `count == 1`. It is right in English and German, and
    /// wrong in both directions elsewhere.
    #[test]
    fn one_is_not_the_singular_everywhere_3564db06() {
        assert_eq!(category("en", CountForm::Cardinal, 1), "one");
        assert_eq!(category("de", CountForm::Cardinal, 1), "one");
        // Japanese does not agree with a number at all, so 1 is the same form as 2.
        assert_eq!(category("ja", CountForm::Cardinal, 1), "other");
        assert_eq!(category("ja", CountForm::Cardinal, 2), "other");
        // French and Portuguese put zero in the singular, which `count == 1` never would.
        assert_eq!(category("fr", CountForm::Cardinal, 0), "one");
        assert_eq!(category("pt", CountForm::Cardinal, 0), "one");
        assert_eq!(category("en", CountForm::Cardinal, 0), "other");
        // And 21 is singular in Russian, which `count == 1` never would either.
        assert_eq!(category("ru", CountForm::Cardinal, 21), "one");
    }

    #[test]
    fn russian_has_three_forms_and_the_teens_are_the_exception_3564db06() {
        for (n, expected) in [
            (1, "one"),
            (2, "few"),
            (4, "few"),
            (5, "many"),
            (11, "many"),
            (12, "many"),
            (14, "many"),
            (21, "one"),
            (22, "few"),
            (25, "many"),
            (111, "many"),
            (121, "one"),
        ] {
            assert_eq!(category("ru", CountForm::Cardinal, n), expected, "{n}");
        }
    }

    /// English's ordinal is the one every client hand-rolls, and the teens are where a hand-rolled
    /// one says "11st".
    #[test]
    fn english_ordinals_know_the_teens_3564db06() {
        for (n, expected) in [
            (1, "one"),
            (2, "two"),
            (3, "few"),
            (4, "other"),
            (11, "other"),
            (12, "other"),
            (13, "other"),
            (21, "one"),
            (22, "two"),
            (23, "few"),
            (31, "one"),
            (111, "other"),
        ] {
            assert_eq!(category("en", CountForm::Ordinal, n), expected, "{n}");
        }
    }

    /// Cardinal and ordinal are different questions about the same number, and nine of the twelve
    /// languages answer the ordinal one with a single form.
    #[test]
    fn the_two_forms_are_different_questions_3564db06() {
        assert_eq!(category("en", CountForm::Cardinal, 3), "other");
        assert_eq!(category("en", CountForm::Ordinal, 3), "few");
        for language in ["de", "es", "pt", "nl", "ru", "ja", "ko", "zh-CN", "zh-TW"] {
            for n in [1, 2, 3, 11, 21] {
                assert_eq!(
                    category(language, CountForm::Ordinal, n),
                    "other",
                    "{language} {n}"
                );
            }
        }
        assert_eq!(category("fr", CountForm::Ordinal, 1), "one");
        assert_eq!(category("fr", CountForm::Ordinal, 2), "other");
        assert_eq!(category("it", CountForm::Ordinal, 8), "many");
        assert_eq!(category("it", CountForm::Ordinal, 9), "other");
    }

    /// The region subtag is not the rule. A client that says `de-DE` or `zh-Hans-CN` gets the
    /// language's answer rather than falling through to `other`, which would be silently wrong for
    /// Russian and French.
    #[test]
    fn the_region_does_not_change_the_rule_3564db06() {
        assert_eq!(category("de-DE", CountForm::Cardinal, 1), "one");
        assert_eq!(category("ru-RU", CountForm::Cardinal, 2), "few");
        assert_eq!(category("pt_BR", CountForm::Cardinal, 0), "one");
        assert_eq!(category("zh-Hans-CN", CountForm::Cardinal, 1), "other");
        assert_eq!(category("EN-GB", CountForm::Ordinal, 3), "few");
    }

    /// A language this build has never met still gets a form it can look up, rather than a panic
    /// or an empty string on screen.
    #[test]
    fn an_unknown_language_falls_back_to_other_3564db06() {
        assert_eq!(category("cy", CountForm::Cardinal, 3), "other");
        assert_eq!(category("", CountForm::Ordinal, 1), "other");
    }

    /// CLDR's `n` is an absolute value. No repeat produces a negative count, but "the -1st
    /// Tuesday" would be a worse answer than "the 1st".
    #[test]
    fn a_negative_count_is_read_by_its_size_3564db06() {
        assert_eq!(category("en", CountForm::Ordinal, -1), "one");
        assert_eq!(category("ru", CountForm::Cardinal, -2), "few");
    }

    /// Every arm returns something [`CATEGORIES`] names — the list the catalog gate checks a
    /// plural key's forms against.
    #[test]
    fn every_answer_is_a_named_category_3564db06() {
        for language in [
            "en", "es", "fr", "de", "it", "pt", "nl", "ru", "ja", "ko", "zh-CN", "zh-TW", "xx",
        ] {
            for form in [CountForm::Cardinal, CountForm::Ordinal] {
                for n in 0..=130 {
                    let answer = category(language, form, n);
                    assert!(
                        CATEGORIES.contains(&answer),
                        "{language} {form:?} {n} answered {answer}"
                    );
                }
            }
        }
    }
}
