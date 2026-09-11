//! The quick-add parser answers as astrid-web's `parseTaskInput` does, in every locale.
//!
//! `contracts/fixtures/smart.json` records the web's answers for a set of inputs under a pinned
//! clock (a Wednesday, TZ=UTC). Each is parsed here on the same day and compared: the title, the
//! lists, the priority, the repeat, the weekdays, and the calendar day of the date — a day rather
//! than an instant, because the web stamps the moment and this crate stores the day (CONTRACTS.md
//! D12).

use astrid_core::model::TaskList;
use astrid_core::parse::smart::{self, Keywords};

const FIXTURE: &str = include_str!("../../../contracts/fixtures/smart.json");

#[derive(serde::Deserialize)]
struct Fixture {
    now: String,
    lists: Vec<FixtureList>,
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct FixtureList {
    id: String,
    name: String,
    #[serde(rename = "isVirtual")]
    is_virtual: bool,
}

#[derive(serde::Deserialize)]
struct Case {
    locale: String,
    input: String,
    title: String,
    #[serde(rename = "listIds")]
    list_ids: Vec<String>,
    #[serde(rename = "dueDate")]
    due_date: Option<String>,
    priority: Option<i64>,
    repeating: Option<String>,
    weekdays: Vec<String>,
}

#[test]
fn every_input_parses_as_web_parses_it() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("the fixture parses");
    assert!(fixture.cases.len() >= 100, "{} cases", fixture.cases.len());
    let today = fixture.now[..10]
        .parse::<chrono::NaiveDate>()
        .expect("the pinned clock is a date");
    let lists: Vec<TaskList> = fixture
        .lists
        .iter()
        .map(|list| {
            let mut made = TaskList::new(&list.id, &list.name);
            made.is_virtual = Some(list.is_virtual);
            made
        })
        .collect();

    for case in &fixture.cases {
        let keywords = Keywords::for_locale(&case.locale);
        let parsed = smart::parse(&case.input, &lists, keywords, today);
        let at = format!("[{}] {:?}", case.locale, case.input);
        assert_eq!(parsed.title, case.title, "{at}: title");
        // The web's answer is the tagged lists followed, when it is a real list and not already
        // there, by the selected one. The parser answers the tags; the create command appends
        // the open list the same way. So the tags must be a prefix, and the difference at most
        // the selected list.
        assert!(
            case.list_ids.starts_with(&parsed.list_ids)
                && case.list_ids.len() - parsed.list_ids.len() <= 1,
            "{at}: tagged {:?} vs web {:?}",
            parsed.list_ids,
            case.list_ids
        );
        assert_eq!(
            parsed.due_day.map(|day| day.to_string()),
            case.due_date.as_deref().map(|iso| iso[..10].to_string()),
            "{at}: due day"
        );
        assert_eq!(parsed.priority, case.priority, "{at}: priority");
        assert_eq!(parsed.repeating, case.repeating, "{at}: repeating");
        assert_eq!(parsed.weekdays, case.weekdays, "{at}: weekdays");
    }
}
