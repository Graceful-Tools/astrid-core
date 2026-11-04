//! The shell itself: the keyboard, the one editing session, the theme, the palette and the
//! first-run tour. Commands about the window rather than about the account or its work.
//!
//! Split out of `dispatch/mod.rs` with the rest of the arms (task 8a91c5b0). `hotkey` lived in
//! `account.rs` until then, which was where there was room rather than where it belongs: a chord
//! is not an account setting, it is this installation remembering a key.

use super::*;

/// Where "the tour has been seen" is remembered.
///
/// The cache, so it belongs to this installation: somebody who has used the app for a year on a
/// laptop still wants to be shown where the hotkey is the first time they open it on a desktop.
const TOUR_KEY: &str = "tour.seen";

/// The global quick-add chord, as chosen or as shipped, with its parts for `RegisterHotKey`.
fn hotkey(app: &App) -> Response {
    let stored = app
        .store
        .metadata(crate::keyboard::chord::KEY)
        .ok()
        .flatten()
        .unwrap_or_else(|| crate::keyboard::chord::DEFAULT.to_string());
    // A stored chord this build cannot read falls back to the default rather than to nothing:
    // an app with no way to summon it is worse than one with the shipped way.
    let chord = crate::keyboard::chord::parse(&stored).unwrap_or_else(|_| {
        crate::keyboard::chord::parse(crate::keyboard::chord::DEFAULT)
            .expect("the default is a chord")
    });
    Response::ok(serde_json::json!({
        "chord": chord.to_string(),
        "ctrl": chord.ctrl,
        "alt": chord.alt,
        "shift": chord.shift,
        "win": chord.win,
        "key": chord.key.to_string(),
    }))
}

/// Step the one editing session and answer with what the shell must do (task e71ed760).
///
/// The machine is pure ([`crate::editing`]); this is where its session lives between calls. A
/// poisoned lock is recovered rather than propagated: the session is one small value, and a
/// panic elsewhere must not leave every editor in the window unable to open.
fn editing(
    app: &App,
    step: impl FnOnce(&crate::editing::Session) -> crate::editing::Transition,
) -> Response {
    let mut session = app
        .editing
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let transition = step(&session);
    *session = transition.session.clone();
    Response::ok(serde_json::json!({
        "active": transition.session.active,
        "commit": transition.commit,
        "cancel": transition.cancel,
    }))
}

/// Every command about the shell itself — the keys, the editing session, the theme, the palette, handed here whole by [`super::run`]
/// (task 8a91c5b0).
///
/// Every arm is short on purpose: it names a service call, and the service decides. An arm that
/// grew a rule would be a rule the shell's vocabulary had quietly acquired, in a file nobody
/// would look in for it.
pub(super) fn run(app: &App, command: Command) -> Response {
    match command {
        Command::ResolveShortcut {
            key,
            has_selection,
            is_text_field_focused,
            is_modal_presented,
        } => {
            let context = crate::keyboard::Context {
                has_selection,
                is_text_field_focused,
                is_modal_presented,
            };
            Response::ok(serde_json::json!({
                "action": crate::keyboard::action_for(&key, context).map(crate::keyboard::action_name),
            }))
        }
        Command::Shortcuts => Response::ok(
            crate::keyboard::ALL
                .iter()
                .map(|binding| {
                    serde_json::json!({
                        "keys": binding.keys,
                        "action": crate::keyboard::action_name(binding.action),
                        "requiresSelection": binding.requires_selection,
                        "title": binding.title,
                    })
                })
                .collect::<Vec<_>>(),
        ),
        Command::Hotkey => hotkey(app),
        Command::SetHotkey { chord } => match crate::keyboard::chord::parse(&chord) {
            Ok(parsed) => {
                match app
                    .store
                    .set_metadata(crate::keyboard::chord::KEY, &parsed.to_string())
                {
                    Ok(()) => hotkey(app),
                    Err(error) => Response::failed(error.into()),
                }
            }
            Err(reason) => Response::failed(Failure::bad_request(reason.to_string())),
        },
        Command::BeginEditing { editor } => {
            editing(app, |session| crate::editing::begin(session, &editor))
        }
        Command::EndEditing { editor } => {
            editing(app, |session| crate::editing::end(session, &editor))
        }
        Command::CancelEditing { editor } => {
            editing(app, |session| crate::editing::cancel(session, &editor))
        }
        Command::CommitAllEditing => editing(app, crate::editing::commit_all),
        Command::Theme => {
            let chosen = app
                .store
                .metadata(crate::theme::KEY)
                .ok()
                .flatten()
                .map(|value| crate::theme::Theme::parse(&value))
                .unwrap_or_default();
            Response::ok(serde_json::json!({
                "theme": chosen,
                "isDark": chosen.is_dark(),
                "choices": crate::theme::Theme::all()
                    .iter()
                    .map(|theme| theme.wire())
                    .collect::<Vec<_>>(),
            }))
        }
        Command::SetTheme { theme } => {
            match app.store.set_metadata(crate::theme::KEY, theme.wire()) {
                Ok(()) => Response::ok(serde_json::json!({
                    "theme": theme,
                    "isDark": theme.is_dark(),
                })),
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::HasSeenTour => Response::ok(serde_json::json!({
            "seen": app
                .store
                .metadata(TOUR_KEY)
                .ok()
                .flatten()
                .is_some_and(|value| value == "yes"),
        })),
        Command::TourSeen => match app.store.set_metadata(TOUR_KEY, "yes") {
            Ok(()) => Response::done(),
            Err(error) => Response::failed(error.into()),
        },
        Command::Palette { query } => {
            let lists = app.store.lists().unwrap_or_default();
            // Bounded: both of the palette's passes drop finished tasks first, so the
            // `completed` column keeps exactly what the matcher would (task `3845b8a6`).
            let tasks = app.store.tasks_incomplete().unwrap_or_default();
            Response::ok(serde_json::json!({
                "rows": crate::palette::search(&query, &lists, &tasks),
            }))
        }
        // `super::run` is one exhaustive match over every variant and the groups it routes are
        // disjoint, so the compiler has already proved nothing else reaches this arm.
        _ => misrouted("shell"),
    }
}
