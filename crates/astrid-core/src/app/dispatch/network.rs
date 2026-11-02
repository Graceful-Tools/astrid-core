//! Sync, the outbox, the journal and the cache — every command about reaching the server, or
//! about what this machine is holding for it.
//!
//! Split out of `dispatch/mod.rs` with the rest of the arms (task 8a91c5b0). The eleven domain
//! modules that existed then were each about one kind of THING — a task, a list, a board — and a
//! sync pass is about none of them, so it had nowhere to live but the router.

use super::*;

/// The newest few refused writes, as `{ kind, error }` — why a write did not land, readable where
/// the count is shown. The journal keeps them, so the reason is never lost to a prune.
fn dead_letters(app: &App) -> Vec<serde_json::Value> {
    crate::outbox::journal::dead_letters(&app.store, 5)
        .unwrap_or_default()
        .into_iter()
        .map(|entry| serde_json::json!({ "kind": entry.kind, "error": entry.last_error }))
        .collect()
}

/// Every command about sync, the outbox or the cache, handed here whole by [`super::run`]
/// (task 8a91c5b0).
///
/// Every arm is short on purpose: it names a service call, and the service decides. An arm that
/// grew a rule would be a rule the shell's vocabulary had quietly acquired, in a file nobody
/// would look in for it.
pub(super) async fn run(app: &App, command: Command) -> Response {
    match command {
        Command::ImportJournalEntry {
            entry_kind,
            payload,
            client_request_id,
            temp_id,
        } => {
            use crate::outbox::kind;
            let known = [
                kind::CREATE_TASK,
                kind::UPDATE_TASK,
                kind::DELETE_TASK,
                kind::COMPLETE_TASK,
                kind::CREATE_LIST,
                kind::UPDATE_LIST,
                kind::CREATE_COMMENT,
                kind::UPDATE_COMMENT,
                kind::DELETE_COMMENT,
                // The Apple apps queued membership changes offline in Core Data (D31).
                kind::INVITE_TO_LIST,
                kind::SET_MEMBER_ROLE,
                kind::REMOVE_MEMBER,
            ];
            if !known.contains(&entry_kind.as_str()) {
                return Response::failed(Failure::bad_request(format!(
                    "{entry_kind} is not a write this journal imports"
                )));
            }
            // Imported once: an upgrade that runs again after a partial failure must not queue a
            // write it already moved. One `EXISTS` for the pair, rather than reading and decoding
            // the whole journal to scan for it — this is asked once per entry of a migrating
            // client's journal, so the scan was quadratic in the size of it (task 71ddacb8).
            match crate::outbox::journal::holds(&app.store, &entry_kind, &client_request_id) {
                Ok(true) => return Response::done(),
                Ok(false) => {}
                Err(error) => return Response::failed(error.into()),
            }
            let entry =
                crate::outbox::build(&entry_kind, payload, &client_request_id, app.clock.now());
            let entry = match temp_id {
                Some(temp_id) => entry.for_temp_id(temp_id),
                None => entry,
            };
            match crate::outbox::journal::enqueue(&app.store, &entry) {
                Ok(_) => Response::done(),
                Err(error) => Response::failed(Failure::from(error)),
            }
        }
        Command::SeedCache {
            current_user,
            tasks,
            lists,
            comments,
            users,
            projects,
            channels,
            messages,
        } => {
            let store = &app.store;
            // Only into an empty cache: a seed over one this crate already filled would put a
            // stale copy over what the server said. `EXISTS`, so the usual answer — a cache that
            // is not empty, on every launch after the first — stops at one row instead of
            // decoding the account to call `.is_empty()` on it (task 71ddacb8).
            match store.has_any_tasks() {
                Ok(false) => {}
                Ok(true) => return Response::ok(serde_json::json!({ "seeded": false })),
                Err(error) => return Response::failed(Failure::from(error)),
            }
            let written = store
                .upsert_lists(&lists)
                .and_then(|()| store.upsert_tasks(&tasks))
                .and_then(|()| store.upsert_comments(&comments))
                .and_then(|()| store.upsert_users(&users))
                .and_then(|()| store.upsert_projects(&projects))
                .and_then(|()| store.upsert_channels(&channels))
                .and_then(|()| store.upsert_messages(&messages));
            if let (Ok(()), Some(user)) = (&written, current_user) {
                if let Err(error) = app.context.account().set_current_user(&user).await {
                    return Response::failed(error.into());
                }
            }
            match written {
                Ok(()) => Response::ok(serde_json::json!({
                    "seeded": true,
                    "tasks": tasks.len(),
                    "lists": lists.len(),
                })),
                Err(error) => Response::failed(Failure::from(error)),
            }
        }
        Command::OutboxStats => match crate::outbox::journal::stats(&app.store) {
            Ok(stats) => Response::ok(serde_json::json!({
                "pending": stats.pending,
                "running": stats.running,
                "completed": stats.completed,
                "failed": stats.failed,
                "hasUnsentWork": stats.has_unsent_work(),
                "deadLetters": dead_letters(app),
                // When the last full pass began, for Settings' "Last Sync" (iOS; AATD-55).
                "lastSync": app.store.metadata(crate::sync::LAST_SYNC_KEY).ok().flatten(),
            })),
            Err(error) => Response::failed(error.into()),
        },
        Command::Sync => {
            let mut report = app.sync.sync().await;
            // What its push step delivered is announced like any delivery; what it pulled in the
            // caller reads from the answer, as it always has.
            if let Some(delivered) = report.delivered.take() {
                crate::app::background::announce_delivery(app, delivered);
            }
            // The flags ride on a pass that reached the server: what this person may see can
            // change while the app is open, and a board that appears on the next pass beats one
            // that appears on the next launch. Best effort, like the projects.
            if report.fetched {
                let _ = app.context.account().refresh_features().await;
            }
            Response::ok(serde_json::json!({
                "fetched": report.fetched,
                "skipped": report.skipped,
                "delta": report.delta,
                "listsAdded": report.lists_added,
                "listsUpdated": report.lists_updated,
                "listsDeleted": report.lists_deleted,
                "tasksAdded": report.tasks_added,
                "tasksUpdated": report.tasks_updated,
                "tasksDeleted": report.tasks_deleted,
                "pushesFailed": report.pushes_failed,
            }))
        }
        Command::StreamState => {
            Response::ok(serde_json::json!({ "live": app.realtime().is_live() }))
        }
        Command::ClearCache => match app.store.clear_rows(crate::sync::LAST_SYNC_KEY) {
            Ok(()) => {
                app.realtime().publish(crate::realtime::Change::Synced {
                    task_ids: Vec::new(),
                    list_ids: Vec::new(),
                });
                Response::done()
            }
            Err(error) => Response::failed(error.into()),
        },
        Command::NetworkRestored => {
            match crate::outbox::journal::wake_offline(&app.store, app.clock.now()) {
                Ok(woken) => {
                    app.outbox_nudge().notify_one();
                    app.realtime().reconnect_now();
                    Response::ok(serde_json::json!({ "woken": woken }))
                }
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::ReconnectStream => {
            app.realtime().reconnect_now();
            Response::done()
        }
        Command::RetryDeadLetters => {
            match crate::outbox::journal::revive_dead(&app.store, app.clock.now()) {
                Ok(revived) => {
                    // The delivery loop sends them; the caller does not wait on the network.
                    app.outbox_nudge().notify_one();
                    Response::ok(serde_json::json!({ "revived": revived }))
                }
                Err(error) => Response::failed(error.into()),
            }
        }
        Command::Drain => match app.runner.drain().await {
            Ok(report) => Response::ok(serde_json::json!({
                "completed": report.completed,
                "retried": report.retried,
                "deadLettered": report.dead_lettered,
            })),
            Err(error) => Response::failed(error.into()),
        },
        // `super::run` is one exhaustive match over every variant and the groups it routes are
        // disjoint, so the compiler has already proved nothing else reaches this arm.
        _ => misrouted("network"),
    }
}
