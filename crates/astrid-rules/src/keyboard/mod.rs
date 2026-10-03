//! The keyboard scheme — a cross-platform contract, not a Windows convention.
//!
//! Ported from `astrid-ios/Astrid Mac/Keyboard/KeyboardShortcuts.swift`, which itself mirrors the
//! canonical web set in `astrid-web/hooks/useKeyboardShortcuts.ts` (`KEYBOARD_SHORTCUTS`). The
//! scheme is single-key and modifier-less (Gmail-style) so muscle memory transfers 1:1 between web,
//! Mac and Windows. Changing a binding means changing every platform in the same change.
//!
//! Ctrl-menu equivalents, Ctrl+K for the command palette and Ctrl+1..9 for list jumps are ADDITIVE
//! shell concerns and deliberately not in this table: a bare key here must never be shadowed by a
//! Windows accelerator.
//!
//! The tests lock this table against `contracts/fixtures/shortcuts.json`, which is generated from
//! the web source — so a web change fails this crate's tests rather than shipping a silent
//! divergence.

/// One logical action a key can trigger. The name is stable and is what the shell dispatches on.
pub mod chord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutAction {
    NewTask,
    CompleteTask,
    DueDateEarlier,
    DueDateLater,
    JumpToDate,
    Postpone,
    RemoveDueDate,
    EditLists,
    EditTitle,
    EditDescription,
    AddComment,
    AssignNoOne,
    PriorityNone,
    PriorityLow,
    PriorityMedium,
    PriorityHigh,
    DeleteTask,
    TogglePanel,
    CycleFilters,
    SelectPrevious,
    SelectNext,
    OutdentTask,
    IndentTask,
    ShowShortcuts,
}

/// A bare-key binding.
///
/// `keys` holds the canonical key first, then any aliases (`j` and the down arrow).
/// `requires_selection` mirrors web's `if (selectedTask)` guard. `web_action` is the exact handler
/// name in `useKeyboardShortcuts.ts`, which is what the parity test compares — a rename on web
/// should be visible here rather than inferred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinding {
    pub keys: &'static [&'static str],
    pub action: ShortcutAction,
    pub requires_selection: bool,
    pub web_action: &'static str,
    pub title: &'static str,
}

/// The name an action travels under.
///
/// One string per action, used by the shortcut answer, by the palette, and by the shell that
/// carries them out. Living here rather than beside one of its callers is what stops the palette
/// naming an action one thing and the keyboard naming it another — which would be two
/// implementations of "new task" wearing the same label.
pub fn action_name(action: ShortcutAction) -> &'static str {
    use ShortcutAction as A;
    match action {
        A::NewTask => "newTask",
        A::CompleteTask => "completeTask",
        A::DueDateEarlier => "dueDateEarlier",
        A::DueDateLater => "dueDateLater",
        A::JumpToDate => "jumpToDate",
        A::Postpone => "postpone",
        A::RemoveDueDate => "removeDueDate",
        A::EditLists => "editLists",
        A::EditTitle => "editTitle",
        A::EditDescription => "editDescription",
        A::AddComment => "addComment",
        A::AssignNoOne => "assignNoOne",
        A::PriorityNone => "priorityNone",
        A::PriorityLow => "priorityLow",
        A::PriorityMedium => "priorityMedium",
        A::PriorityHigh => "priorityHigh",
        A::DeleteTask => "deleteTask",
        A::TogglePanel => "togglePanel",
        A::CycleFilters => "cycleFilters",
        A::SelectPrevious => "selectPrevious",
        A::SelectNext => "selectNext",
        A::OutdentTask => "outdentTask",
        A::IndentTask => "indentTask",
        A::ShowShortcuts => "showShortcuts",
    }
}

/// The canonical shared table. Keys, guards and order mirror web `KEYBOARD_SHORTCUTS`.
pub const ALL: &[KeyBinding] = &[
    KeyBinding {
        keys: &["n"],
        action: ShortcutAction::NewTask,
        requires_selection: false,
        web_action: "onNewTask",
        title: "New task",
    },
    KeyBinding {
        keys: &["x"],
        action: ShortcutAction::CompleteTask,
        requires_selection: true,
        web_action: "onCompleteTask",
        title: "Complete selected task",
    },
    KeyBinding {
        keys: &["\u{2190}"],
        action: ShortcutAction::DueDateEarlier,
        requires_selection: true,
        web_action: "onMakeDueDateEarlier",
        title: "Make due date one day earlier",
    },
    KeyBinding {
        keys: &["\u{2192}"],
        action: ShortcutAction::DueDateLater,
        requires_selection: true,
        web_action: "onMakeDueDateLater",
        title: "Make due date one day later",
    },
    KeyBinding {
        keys: &["d"],
        action: ShortcutAction::JumpToDate,
        requires_selection: false,
        web_action: "onJumpToDate",
        title: "Jump to 'Date'",
    },
    KeyBinding {
        keys: &["p"],
        action: ShortcutAction::Postpone,
        requires_selection: true,
        web_action: "onPostponeTask",
        title: "Postpone task by one week",
    },
    KeyBinding {
        keys: &["v"],
        action: ShortcutAction::RemoveDueDate,
        requires_selection: true,
        web_action: "onRemoveDueDate",
        title: "Remove task due date",
    },
    KeyBinding {
        keys: &["i"],
        action: ShortcutAction::EditLists,
        requires_selection: true,
        web_action: "onEditTaskLists",
        title: "Edit task lists",
    },
    KeyBinding {
        keys: &["t"],
        action: ShortcutAction::EditTitle,
        requires_selection: true,
        web_action: "onEditTaskTitle",
        title: "Edit task title",
    },
    KeyBinding {
        keys: &["s"],
        action: ShortcutAction::EditDescription,
        requires_selection: true,
        web_action: "onEditTaskDescription",
        title: "Edit task description",
    },
    KeyBinding {
        keys: &["c"],
        action: ShortcutAction::AddComment,
        requires_selection: true,
        web_action: "onAddTaskComment",
        title: "Add a new task comment",
    },
    KeyBinding {
        keys: &["e"],
        action: ShortcutAction::AssignNoOne,
        requires_selection: true,
        web_action: "onAssignToNoOne",
        title: "Assign task to 'No One'",
    },
    KeyBinding {
        keys: &["0"],
        action: ShortcutAction::PriorityNone,
        requires_selection: true,
        web_action: "onSetPriority(0)",
        title: "Set priority to None",
    },
    KeyBinding {
        keys: &["1"],
        action: ShortcutAction::PriorityLow,
        requires_selection: true,
        web_action: "onSetPriority(1)",
        title: "Set priority to Low",
    },
    KeyBinding {
        keys: &["2"],
        action: ShortcutAction::PriorityMedium,
        requires_selection: true,
        web_action: "onSetPriority(2)",
        title: "Set priority to Medium",
    },
    KeyBinding {
        keys: &["3"],
        action: ShortcutAction::PriorityHigh,
        requires_selection: true,
        web_action: "onSetPriority(3)",
        title: "Set priority to High",
    },
    KeyBinding {
        keys: &["Delete", "Backspace"],
        action: ShortcutAction::DeleteTask,
        requires_selection: true,
        web_action: "onDeleteTask",
        title: "Delete selected task",
    },
    KeyBinding {
        keys: &["o"],
        action: ShortcutAction::TogglePanel,
        requires_selection: false,
        web_action: "onToggleTaskPanel",
        title: "Open/close task edit panel",
    },
    KeyBinding {
        keys: &["l"],
        action: ShortcutAction::CycleFilters,
        requires_selection: false,
        web_action: "onCycleListFilters",
        title: "Cycle through list filters/tags",
    },
    KeyBinding {
        keys: &["k", "\u{2191}"],
        action: ShortcutAction::SelectPrevious,
        requires_selection: false,
        web_action: "onSelectPreviousTask",
        title: "Select previous task",
    },
    KeyBinding {
        keys: &["j", "\u{2193}"],
        action: ShortcutAction::SelectNext,
        requires_selection: false,
        web_action: "onSelectNextTask",
        title: "Select next task",
    },
    KeyBinding {
        keys: &["["],
        action: ShortcutAction::OutdentTask,
        requires_selection: true,
        web_action: "onOutdentTask",
        title: "Move task out of its parent",
    },
    KeyBinding {
        keys: &["]"],
        action: ShortcutAction::IndentTask,
        requires_selection: true,
        web_action: "onIndentTask",
        title: "Nest task under the task above",
    },
    KeyBinding {
        keys: &["?"],
        action: ShortcutAction::ShowShortcuts,
        requires_selection: false,
        web_action: "onShowHotkeyMenu",
        title: "Show hotkey listing",
    },
];

/// Arrow keys reach the core under whichever name the caller has: the web and Mac tables write them
/// as glyphs, while a Windows `VirtualKey` naturally reads as `ArrowUp`. Both resolve, so the shell
/// is not forced to translate before asking.
pub fn normalize_key(key: &str) -> &str {
    match key {
        "ArrowUp" => "\u{2191}",
        "ArrowDown" => "\u{2193}",
        "ArrowLeft" => "\u{2190}",
        "ArrowRight" => "\u{2192}",
        other => other,
    }
}

/// Look up a binding by pressed key (canonical, alias, or event name). `None` if unbound.
pub fn binding_for(key: &str) -> Option<&'static KeyBinding> {
    let key = normalize_key(key);
    ALL.iter().find(|binding| binding.keys.contains(&key))
}

/// What the app knows about the moment a key was pressed.
///
/// Mirrors web's guard inputs: shortcuts are suppressed while a text field or editor is focused, or
/// while a modal is open, and selection-scoped actions need a selected task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Context {
    pub has_selection: bool,
    pub is_text_field_focused: bool,
    pub is_modal_presented: bool,
}

/// Resolve a pressed bare key to the action to run, or `None` if unbound or suppressed.
pub fn action_for(key: &str, context: Context) -> Option<ShortcutAction> {
    // Web parity: never hijack a key while the user is typing or inside a modal.
    if context.is_text_field_focused || context.is_modal_presented {
        return None;
    }
    let binding = binding_for(key)?;
    // Web parity: selection-scoped actions need a selected task.
    if binding.requires_selection && !context.has_selection {
        return None;
    }
    Some(binding.action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Generated from astrid-web by `contracts/export-from-web.mjs`. Compiled in, so a stale
    /// fixture cannot be missed at runtime.
    const SHORTCUTS_FIXTURE: &str = include_str!("../../../../contracts/fixtures/shortcuts.json");

    #[derive(serde::Deserialize)]
    struct Fixture {
        shortcuts: Vec<WebShortcut>,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct WebShortcut {
        key: String,
        event_key: String,
        web_action: String,
        requires_selection: bool,
    }

    fn web_shortcuts() -> Vec<WebShortcut> {
        serde_json::from_str::<Fixture>(SHORTCUTS_FIXTURE)
            .expect("contracts/fixtures/shortcuts.json is malformed")
            .shortcuts
    }

    /// Every web key resolves here to the same handler with the same selection guard.
    #[test]
    fn every_web_key_is_mirrored() {
        for expected in web_shortcuts() {
            let binding = binding_for(&expected.key).unwrap_or_else(|| {
                panic!(
                    "web key '{}' ({}) has no binding",
                    expected.key, expected.web_action
                )
            });
            assert_eq!(
                binding.web_action, expected.web_action,
                "key '{}' maps to {} here but {} on web",
                expected.key, binding.web_action, expected.web_action
            );
            assert_eq!(
                binding.requires_selection, expected.requires_selection,
                "key '{}' selection guard differs from web",
                expected.key
            );
        }
    }

    /// A Windows shell naturally names arrows `ArrowUp`; those must resolve to the same bindings.
    #[test]
    fn event_key_names_resolve_to_the_same_binding() {
        for expected in web_shortcuts() {
            assert_eq!(
                binding_for(&expected.key),
                binding_for(&expected.event_key),
                "'{}' and '{}' must resolve to one binding",
                expected.key,
                expected.event_key
            );
        }
    }

    /// No extra bare keys crept in — one would collide with a future web key and break parity.
    #[test]
    fn there_are_no_extra_bare_keys() {
        let web: HashSet<String> = web_shortcuts().into_iter().map(|s| s.key).collect();
        let ours: HashSet<String> = ALL
            .iter()
            .flat_map(|b| b.keys.iter().map(|k| (*k).to_string()))
            .collect();
        assert_eq!(ours, web, "bare-key set diverged from web");
    }

    /// 27 web keys across 24 actions: Delete and Backspace share one, and j/down and k/up alias.
    #[test]
    fn action_coverage_matches_web() {
        assert_eq!(ALL.len(), 24, "unexpected number of bindings");
        assert_eq!(
            ALL.iter().map(|b| b.action).collect::<HashSet<_>>().len(),
            24,
            "duplicate actions in the table"
        );
    }

    // Dispatch guards, ported from Mac's KeyboardShortcutHandlerTests.

    #[test]
    fn an_unselected_global_key_fires() {
        assert_eq!(
            action_for("n", Context::default()),
            Some(ShortcutAction::NewTask)
        );
    }

    #[test]
    fn a_selection_scoped_key_is_suppressed_without_a_selection() {
        assert_eq!(
            action_for(
                "x",
                Context {
                    has_selection: false,
                    ..Default::default()
                }
            ),
            None
        );
        assert_eq!(
            action_for(
                "x",
                Context {
                    has_selection: true,
                    ..Default::default()
                }
            ),
            Some(ShortcutAction::CompleteTask)
        );
    }

    /// Even a global key must not fire while the user is typing.
    #[test]
    fn suppressed_while_typing_in_a_text_field() {
        assert_eq!(
            action_for(
                "n",
                Context {
                    is_text_field_focused: true,
                    ..Default::default()
                }
            ),
            None
        );
    }

    #[test]
    fn suppressed_while_a_modal_is_open() {
        assert_eq!(
            action_for(
                "n",
                Context {
                    is_modal_presented: true,
                    ..Default::default()
                }
            ),
            None
        );
    }

    #[test]
    fn alias_keys_resolve() {
        let ctx = Context::default();
        assert_eq!(
            action_for("\u{2193}", ctx),
            Some(ShortcutAction::SelectNext)
        );
        assert_eq!(action_for("j", ctx), Some(ShortcutAction::SelectNext));
        assert_eq!(
            action_for("ArrowDown", ctx),
            Some(ShortcutAction::SelectNext)
        );
        assert_eq!(
            action_for("\u{2191}", ctx),
            Some(ShortcutAction::SelectPrevious)
        );
        assert_eq!(action_for("k", ctx), Some(ShortcutAction::SelectPrevious));
        assert_eq!(
            action_for("ArrowUp", ctx),
            Some(ShortcutAction::SelectPrevious)
        );
    }

    #[test]
    fn delete_aliases_require_a_selection() {
        assert_eq!(
            action_for(
                "Backspace",
                Context {
                    has_selection: false,
                    ..Default::default()
                }
            ),
            None
        );
        assert_eq!(
            action_for(
                "Delete",
                Context {
                    has_selection: true,
                    ..Default::default()
                }
            ),
            Some(ShortcutAction::DeleteTask)
        );
    }

    #[test]
    fn an_unbound_key_resolves_to_nothing() {
        assert_eq!(
            action_for(
                "z",
                Context {
                    has_selection: true,
                    ..Default::default()
                }
            ),
            None
        );
    }

    #[test]
    fn priority_keys_require_a_selection() {
        assert_eq!(
            action_for(
                "2",
                Context {
                    has_selection: false,
                    ..Default::default()
                }
            ),
            None
        );
        assert_eq!(
            action_for(
                "2",
                Context {
                    has_selection: true,
                    ..Default::default()
                }
            ),
            Some(ShortcutAction::PriorityMedium)
        );
    }
}
