//! Lists: creating, editing, deleting, and answering what the signed-in user may do with one.
//!
//! Ported from `astrid-ios/Astrid App/Core/Services/ListService.swift` and
//! `ListMemberService.swift`.
//!
//! The permission questions live here rather than in the shell, and they are questions rather than
//! roles: the shell asks "may this person delete this list?" and renders the answer. A
//! `role == "admin"` written in XAML is the bug shape where a control appears for someone who
//! cannot use it — which is what happened on Mac before `ListPermissions.swift` existed. The rules
//! themselves are in [`crate::permissions`], locked against web's own by a fixture.

use serde_json::json;

use super::{Context, Result, ServiceError};
use crate::api::endpoints;
use crate::model::{ListMember, Privacy, TaskList};
use crate::outbox::{self, journal, kind};
use crate::permissions::{self, ListAccess, ListRole};

/// An edit to a list. Doubly optional where clearing is possible, for the reason given on
/// [`super::TaskChanges`].
#[derive(Debug, Clone, Default)]
pub struct ListChanges {
    pub name: Option<String>,
    pub color: Option<Option<String>>,
    pub description: Option<Option<String>>,
    pub privacy: Option<Privacy>,
    pub is_favorite: Option<bool>,
    pub favorite_order: Option<Option<i64>>,
    pub sort_by: Option<Option<String>>,
    pub manual_sort_order: Option<Vec<String>>,
    pub show_subtasks: Option<bool>,
    pub default_assignee_id: Option<Option<String>>,
    pub default_priority: Option<Option<i64>>,
    pub default_due_time: Option<Option<String>>,
    pub filter_completion: Option<Option<String>>,
    /// The other six saved filters. Every one of them was already applied by
    /// [`crate::filters`] and settable by nothing, so a filter set on web could be read here and
    /// never changed.
    pub filter_priority: Option<Option<String>>,
    pub filter_due_date: Option<Option<String>>,
    pub filter_assignee: Option<Option<String>>,
    pub filter_repeating: Option<Option<String>>,
    pub filter_assigned_by: Option<Option<String>>,
    pub filter_in_lists: Option<Option<String>>,
    pub recently_completed_window: Option<Option<crate::model::RecentlyCompletedWindow>>,
}

impl ListChanges {
    pub fn name(name: impl Into<String>) -> Self {
        ListChanges {
            name: Some(name.into()),
            ..Default::default()
        }
    }

    pub fn apply(&self, list: &mut TaskList) {
        if let Some(value) = &self.name {
            list.name = value.clone();
        }
        if let Some(value) = &self.color {
            list.color = value.clone();
        }
        if let Some(value) = &self.description {
            list.description = value.clone();
        }
        if let Some(value) = self.privacy {
            list.privacy = Some(value);
        }
        if let Some(value) = self.is_favorite {
            list.is_favorite = Some(value);
        }
        if let Some(value) = self.favorite_order {
            list.favorite_order = value;
        }
        if let Some(value) = &self.sort_by {
            list.sort_by = value.clone();
        }
        if let Some(value) = &self.manual_sort_order {
            list.manual_sort_order = Some(value.clone());
        }
        if let Some(value) = self.show_subtasks {
            list.show_subtasks = Some(value);
        }
        if let Some(value) = &self.default_assignee_id {
            list.default_assignee_id = value.clone();
            if value.is_none() {
                list.default_assignee = None;
            }
        }
        if let Some(value) = self.default_priority {
            list.default_priority = value;
        }
        if let Some(value) = &self.default_due_time {
            list.default_due_time = value.clone();
        }
        if let Some(value) = &self.filter_priority {
            list.filter_priority = value.clone();
        }
        if let Some(value) = &self.filter_due_date {
            list.filter_due_date = value.clone();
        }
        if let Some(value) = &self.filter_assignee {
            list.filter_assignee = value.clone();
        }
        if let Some(value) = &self.filter_repeating {
            list.filter_repeating = value.clone();
        }
        if let Some(value) = &self.filter_assigned_by {
            list.filter_assigned_by = value.clone();
        }
        if let Some(value) = &self.filter_in_lists {
            list.filter_in_lists = value.clone();
        }
        if let Some(value) = &self.filter_completion {
            list.filter_completion = value.clone();
        }
        if let Some(value) = &self.recently_completed_window {
            list.recently_completed_window = value.clone();
        }
    }

    pub fn to_body(&self) -> serde_json::Value {
        let mut body = serde_json::Map::new();
        let mut set = |key: &str, value: serde_json::Value| {
            body.insert(key.to_string(), value);
        };
        if let Some(value) = &self.name {
            set("name", json!(value));
        }
        if let Some(value) = &self.color {
            set("color", json!(value));
        }
        if let Some(value) = &self.description {
            set("description", json!(value));
        }
        if let Some(value) = self.privacy {
            set("privacy", json!(value));
        }
        if let Some(value) = self.is_favorite {
            set("isFavorite", json!(value));
        }
        if let Some(value) = self.favorite_order {
            set("favoriteOrder", json!(value));
        }
        if let Some(value) = &self.sort_by {
            set("sortBy", json!(value));
        }
        if let Some(value) = &self.manual_sort_order {
            set("manualSortOrder", json!(value));
        }
        if let Some(value) = self.show_subtasks {
            set("showSubtasks", json!(value));
        }
        if let Some(value) = &self.default_assignee_id {
            set("defaultAssigneeId", json!(value));
        }
        if let Some(value) = self.default_priority {
            set("defaultPriority", json!(value));
        }
        if let Some(value) = &self.default_due_time {
            set("defaultDueTime", json!(value));
        }
        if let Some(value) = &self.filter_priority {
            set("filterPriority", json!(value));
        }
        if let Some(value) = &self.filter_due_date {
            set("filterDueDate", json!(value));
        }
        if let Some(value) = &self.filter_assignee {
            set("filterAssignee", json!(value));
        }
        if let Some(value) = &self.filter_repeating {
            set("filterRepeating", json!(value));
        }
        if let Some(value) = &self.filter_assigned_by {
            set("filterAssignedBy", json!(value));
        }
        if let Some(value) = &self.filter_in_lists {
            set("filterInLists", json!(value));
        }
        if let Some(value) = &self.filter_completion {
            set("filterCompletion", json!(value));
        }
        if let Some(value) = &self.recently_completed_window {
            set("recentlyCompletedWindow", json!(value));
        }
        serde_json::Value::Object(body)
    }
}

pub struct ListService {
    context: Context,
}

impl ListService {
    pub fn new(context: Context) -> Self {
        ListService { context }
    }

    // ─── Reads ────────────────────────────────────────────────────────────────────────────────

    pub fn list(&self, id: &str) -> Result<Option<TaskList>> {
        Ok(self.context.store.list(id)?)
    }

    /// Every list, as cached.
    pub fn all(&self) -> Result<Vec<TaskList>> {
        Ok(self.context.store.lists()?)
    }

    /// The My Tasks entry: the view the app opens on, above the lists.
    ///
    /// Not in the list collection and never will be — it has no row on the server, its filters
    /// belong to the account rather than to a list, and its scope ("mine or nobody's") is not
    /// something a list filter can express. It is answered here so the id and the name are the
    /// core's, the same way the two virtual board columns are named in `astrid_core::board`.
    pub fn my_tasks(&self) -> TaskList {
        let mut entry = TaskList::new(crate::filters::my_tasks::VIRTUAL_ID, "My Tasks");
        entry.is_virtual = Some(true);
        entry.virtual_list_type = Some("my-tasks".into());
        entry
    }

    /// The lists a task can be filed in.
    ///
    /// Not the same as the lists a person can navigate into: a virtual list ("Today", "Not in a
    /// List") is somewhere to look and a board column is a state, and neither is somewhere a task
    /// can live. Offering either as a destination produces a task that belongs to a view.
    pub fn destinations(&self) -> Result<Vec<TaskList>> {
        Ok(self
            .context
            .store
            .lists()?
            .into_iter()
            .filter(|list| list.is_domain_list() && !list.is_virtual.unwrap_or(false))
            .collect())
    }

    /// Favourites, in the order the user arranged them. Ties fall back to the name, so a set of
    /// favourites saved before ordering existed still has a stable order rather than SQLite's.
    pub fn favorites(&self) -> Result<Vec<TaskList>> {
        let mut favorites: Vec<TaskList> = self
            .context
            .store
            .lists()?
            .into_iter()
            .filter(|list| list.is_favorite.unwrap_or(false) && list.is_domain_list())
            .collect();
        favorites.sort_by(|a, b| {
            a.favorite_order
                .unwrap_or(i64::MAX)
                .cmp(&b.favorite_order.unwrap_or(i64::MAX))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(favorites)
    }

    // ─── What the signed-in user may do ───────────────────────────────────────────────────────

    /// The user's role, or `None` for no access.
    pub fn role(&self, user_id: &str, list: &TaskList) -> Option<ListRole> {
        permissions::role_in_list(user_id, &access_of(list))
    }

    pub fn can_view(&self, user_id: &str, list: &TaskList) -> bool {
        permissions::can_view_list(user_id, &access_of(list))
    }

    /// Whether the user may add or edit tasks in this list.
    pub fn can_edit_tasks(&self, user_id: &str, list: &TaskList) -> bool {
        permissions::can_edit_tasks(user_id, &access_of(list))
    }

    /// Whether the user may edit one particular task in it — which also depends on who wrote it.
    pub fn can_edit_task(
        &self,
        user_id: &str,
        task_creator_id: Option<&str>,
        list: &TaskList,
    ) -> bool {
        permissions::can_edit_task(user_id, task_creator_id, &access_of(list))
    }

    /// Whether the user may change the list's own settings.
    pub fn can_manage(&self, user_id: &str, list: &TaskList) -> bool {
        permissions::can_manage_list(user_id, &access_of(list))
    }

    pub fn can_manage_members(&self, user_id: &str, list: &TaskList) -> bool {
        permissions::can_manage_members(user_id, &access_of(list))
    }

    pub fn can_delete(&self, user_id: &str, list: &TaskList) -> bool {
        permissions::can_delete_list(user_id, &access_of(list))
    }

    // ─── Writes ───────────────────────────────────────────────────────────────────────────────

    pub fn create(&self, name: &str, color: Option<String>) -> Result<TaskList> {
        self.create_with(name, color, None)
    }

    /// Create a list, saying what privacy it has.
    ///
    /// `None` leaves that to the server, as [`Self::create`] always has. A list made from a
    /// task's own editor names one — the privacy its sibling lists have (task d3f3b111) — so a
    /// task in a shared list does not quietly gain a private one nobody else can see.
    pub fn create_with(
        &self,
        name: &str,
        color: Option<String>,
        privacy: Option<crate::model::Privacy>,
    ) -> Result<TaskList> {
        let now = self.context.clock.now();
        let temp_id = outbox::new_temp_id();

        let mut list = TaskList::new(temp_id.clone(), name);
        list.color = color.clone();
        list.privacy = privacy;
        list.created_at = Some(now);
        list.updated_at = Some(now);
        self.context.store.upsert_list(&list)?;

        let mut body = json!({ "name": name, "color": color });
        if let Some(privacy) = privacy {
            body["privacy"] = json!(privacy);
        }
        let entry = outbox::build(kind::CREATE_LIST, json!({ "body": body }), &temp_id, now)
            .for_temp_id(&temp_id);
        journal::enqueue(&self.context.store, &entry)?;

        Ok(list)
    }

    pub fn update(&self, id: &str, changes: &ListChanges) -> Result<TaskList> {
        let now = self.context.clock.now();
        let mut list = self.require(id)?;
        changes.apply(&mut list);
        list.updated_at = Some(now);
        self.context.store.upsert_list(&list)?;

        let entry = outbox::build(
            kind::UPDATE_LIST,
            json!({ "listId": id, "body": changes.to_body() }),
            &outbox::new_temp_id(),
            now,
        );
        let entry = match crate::model::is_temp_id(id) {
            true => entry.for_temp_id(id),
            false => entry,
        };
        journal::enqueue(&self.context.store, &entry)?;
        Ok(list)
    }

    /// Delete a list. The tasks in it are the server's business — it decides what happens to a task
    /// that was only in this list, and guessing here would show the user an answer the next sync
    /// contradicts.
    /// Delete a list.
    ///
    /// If it mirrored a remote one, the remote list is written down as excluded first — in an
    /// all-lists mode the next pass would see an unlinked remote list and helpfully make this one
    /// again, and again after that.
    pub fn delete(&self, id: &str) -> Result<()> {
        let now = self.context.clock.now();
        self.exclude_mirrored_list(id);
        self.context.store.delete_list(id)?;

        let entry = outbox::build(
            kind::DELETE_LIST,
            json!({ "listId": id }),
            &outbox::new_temp_id(),
            now,
        );
        let entry = match crate::model::is_temp_id(id) {
            true => entry.for_temp_id(id),
            false => entry,
        };
        journal::enqueue(&self.context.store, &entry)?;
        Ok(())
    }

    /// Say no to the remote list this one mirrored, if it mirrored one.
    ///
    /// Best effort, and for the same reasons as the task ledger: the link is gone by the time a
    /// pass could look it up, and a list somebody asked to delete has to go whatever happens here.
    fn exclude_mirrored_list(&self, id: &str) {
        use crate::external::ledger;
        let store = &self.context.store;
        if let Some((container_id, _)) = ledger::twin(store, "google.lists", id) {
            let _ = ledger::exclude(store, "google", &container_id);
            let _ = ledger::forget_link(store, "google.lists", id);
        }
    }

    pub fn set_favorite(&self, id: &str, favorite: bool) -> Result<TaskList> {
        self.update(
            id,
            &ListChanges {
                is_favorite: Some(favorite),
                ..Default::default()
            },
        )
    }

    // ─── Membership ───────────────────────────────────────────────────────────────────────────
    //
    // These reach the network directly rather than through the Outbox, and that is deliberate: an
    // invitation is not a local fact. Queuing one offline would show a member in the list who does
    // not exist, and the "optimistic" row would be indistinguishable from a real one to every
    // permission check that reads it afterwards.

    pub async fn members(&self, list_id: &str) -> Result<Vec<ListMember>> {
        let request = self.context.client.get(endpoints::list_members(list_id));
        let members = self
            .context
            .client
            .send_collection::<ListMember>(request, Some(endpoints::envelope::MEMBERS))
            .await?;
        Ok(members.into_items())
    }

    pub async fn invite(&self, list_id: &str, email: &str, role: &str) -> Result<()> {
        let request = self
            .context
            .client
            .post(endpoints::list_members(list_id))
            .value(json!({ "email": email, "role": role }));
        self.context.client.send(request).await?;
        Ok(())
    }

    pub async fn set_member_role(&self, list_id: &str, user_id: &str, role: &str) -> Result<()> {
        let request = self
            .context
            .client
            .put(endpoints::list_member(list_id, user_id))
            .value(json!({ "role": role }));
        self.context.client.send(request).await?;
        Ok(())
    }

    pub async fn remove_member(&self, list_id: &str, user_id: &str) -> Result<()> {
        let request = self
            .context
            .client
            .delete(endpoints::list_member(list_id, user_id));
        self.context.client.send(request).await?;
        Ok(())
    }

    pub async fn leave(&self, list_id: &str) -> Result<()> {
        let request = self.context.client.post(endpoints::leave_list(list_id));
        self.context.client.send(request).await?;
        self.context.store.delete_list(list_id)?;
        Ok(())
    }

    fn require(&self, id: &str) -> Result<TaskList> {
        self.context
            .store
            .list(id)?
            .ok_or_else(|| ServiceError::NotFound {
                kind: "list",
                id: id.to_string(),
            })
    }
}

/// The part of a list that decides access.
///
/// Built here rather than by the caller so no code path can construct one that leaves out
/// `list_members` and quietly resolves every collaborator to no access.
fn access_of(list: &TaskList) -> ListAccess {
    ListAccess {
        owner_id: list.owner_id.clone().unwrap_or_default(),
        owner: list.owner.as_ref().map(|owner| permissions::UserRef {
            id: owner.id.clone(),
        }),
        privacy: list
            .privacy
            .map(|privacy| match privacy {
                Privacy::Private => "PRIVATE",
                Privacy::Shared => "SHARED",
                Privacy::Public => "PUBLIC",
            })
            .unwrap_or("PRIVATE")
            .to_string(),
        public_list_type: list.public_list_type.clone(),
        list_members: list
            .list_members
            .iter()
            .flatten()
            .map(|member| permissions::ListMembership {
                user_id: member.user_id.clone(),
                role: Some(member.role.clone()),
                user: member.user.as_ref().map(|user| permissions::UserRef {
                    id: user.id.clone(),
                }),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiClient, StubTransport};
    use crate::platform::{FixedClock, MemorySecureStore};
    use crate::store::Store;
    use std::sync::Arc;

    struct Fixture {
        service: ListService,
        store: Arc<Store>,
    }

    fn fixture() -> Fixture {
        fixture_with(StubTransport::new())
    }

    /// Deleting a mirrored list has to be remembered here, because the link is gone the moment
    /// the list is — and in an all-lists mode the next pass would make the list again.
    #[test]
    fn deleting_a_mirrored_list_says_no_to_the_remote_one() {
        let fixture = fixture();
        crate::external::ledger::remember_links(
            &fixture.store,
            "google.lists",
            "google",
            [("l1".to_string(), "c1".to_string())],
        )
        .expect("remembers");
        fixture
            .store
            .upsert_list(&TaskList::new("l1", "Groceries"))
            .expect("writes");

        fixture.service.delete("l1").expect("deletes");

        assert_eq!(
            crate::external::ledger::excluded(&fixture.store, "google"),
            vec!["c1".to_string()]
        );
    }

    /// A list nobody mirrored has no remote counterpart to refuse, and excluding one would have
    /// auto-link quietly skipping a list the person never asked it to.
    #[test]
    fn deleting_an_unmirrored_list_excludes_nothing() {
        let fixture = fixture();
        fixture
            .store
            .upsert_list(&TaskList::new("l1", "Groceries"))
            .expect("writes");

        fixture.service.delete("l1").expect("deletes");

        assert!(crate::external::ledger::excluded(&fixture.store, "google").is_empty());
    }

    fn fixture_with(transport: StubTransport) -> Fixture {
        let store = Arc::new(Store::in_memory().expect("opens"));
        let context = Context::new(
            Arc::new(ApiClient::new(
                "https://astrid.cc",
                Arc::new(transport),
                Arc::new(MemorySecureStore::new()),
            )),
            store.clone(),
            Arc::new(FixedClock::parsed("2026-09-07T12:00:00Z")),
        );
        Fixture {
            service: context.lists(),
            store,
        }
    }

    fn list_json(value: serde_json::Value) -> TaskList {
        serde_json::from_value(value).expect("decodes")
    }

    #[test]
    fn a_created_list_is_usable_before_it_is_sent() {
        let fixture = fixture();
        let created = fixture
            .service
            .create("Home", Some("#ff0000".into()))
            .expect("creates");

        assert!(crate::model::is_temp_id(&created.id));
        assert_eq!(fixture.service.all().expect("reads").len(), 1);
        let entries = journal::all(&fixture.store).expect("reads");
        assert_eq!(entries[0].kind, kind::CREATE_LIST);
        assert_eq!(entries[0].payload["body"]["name"], "Home");
    }

    #[test]
    fn an_edit_sends_only_what_changed() {
        let fixture = fixture();
        fixture
            .store
            .upsert_list(&TaskList::new("l1", "Home"))
            .expect("stores");

        fixture
            .service
            .update("l1", &ListChanges::name("House"))
            .expect("updates");
        assert_eq!(
            fixture
                .store
                .list("l1")
                .expect("reads")
                .expect("present")
                .name,
            "House"
        );
        let body = &journal::all(&fixture.store).expect("reads")[0].payload["body"];
        assert_eq!(body.as_object().expect("an object").len(), 1);
        assert_eq!(body["name"], "House");
    }

    /// Board columns and virtual lists are not destinations. A picker that offered "Doing" would
    /// be offering a state, and one that offered "Today" would be offering a view.
    #[test]
    fn status_rows_and_virtual_lists_are_not_offered_as_destinations() {
        let fixture = fixture();
        fixture
            .store
            .upsert_lists(&[
                TaskList::new("l1", "Home"),
                list_json(json!({ "id": "s1", "name": "Doing", "listType": "status" })),
                list_json(json!({ "id": "v1", "name": "Today", "isVirtual": true })),
            ])
            .expect("stores");
        let destinations: Vec<String> = fixture
            .service
            .destinations()
            .expect("reads")
            .into_iter()
            .map(|list| list.id)
            .collect();
        assert_eq!(destinations, vec!["l1"]);
    }

    /// Favourites saved before ordering existed have no order at all. Falling back to the name
    /// keeps the sidebar stable instead of reshuffling on every launch.
    #[test]
    fn favourites_are_ordered_and_ties_fall_back_to_the_name() {
        let fixture = fixture();
        fixture
            .store
            .upsert_lists(&[
                list_json(json!({ "id": "a", "name": "Zebra", "isFavorite": true })),
                list_json(json!({ "id": "b", "name": "Apple", "isFavorite": true })),
                list_json(
                    json!({ "id": "c", "name": "Middle", "isFavorite": true, "favoriteOrder": 1 }),
                ),
                list_json(json!({ "id": "d", "name": "Not one" })),
            ])
            .expect("stores");

        let order: Vec<String> = fixture
            .service
            .favorites()
            .expect("reads")
            .into_iter()
            .map(|list| list.id)
            .collect();
        assert_eq!(order, vec!["c", "b", "a"]);
    }

    /// The shell asks a question and renders the answer. These are the questions.
    #[test]
    fn permission_questions_are_answered_here_rather_than_in_the_shell() {
        let fixture = fixture();
        let list = list_json(json!({
            "id": "l1",
            "name": "Shared",
            "privacy": "SHARED",
            "ownerId": "owner",
            "listMembers": [
                { "userId": "admin", "role": "admin" },
                { "userId": "member", "role": "member" }
            ]
        }));

        assert_eq!(fixture.service.role("owner", &list), Some(ListRole::Owner));
        assert_eq!(fixture.service.role("admin", &list), Some(ListRole::Admin));
        assert_eq!(
            fixture.service.role("member", &list),
            Some(ListRole::Member)
        );
        assert_eq!(fixture.service.role("stranger", &list), None);

        assert!(fixture.service.can_delete("owner", &list));
        assert!(!fixture.service.can_delete("admin", &list));
        assert!(fixture.service.can_manage_members("admin", &list));
        assert!(!fixture.service.can_manage_members("member", &list));
        assert!(fixture.service.can_edit_tasks("member", &list));
        assert!(!fixture.service.can_view("stranger", &list));
    }

    /// A list with no privacy on it — the thinner responses omit it — must not read as public.
    #[test]
    fn a_list_with_no_stated_privacy_is_treated_as_private() {
        let fixture = fixture();
        let list = list_json(json!({ "id": "l1", "name": "Home", "ownerId": "owner" }));
        assert!(!fixture.service.can_view("stranger", &list));
        assert!(fixture.service.can_view("owner", &list));
    }

    /// An invitation is not a local fact: it goes over the wire, and a failure is a failure the
    /// user has to see rather than a member row that quietly is not one.
    #[tokio::test]
    async fn inviting_someone_goes_to_the_server_rather_than_the_journal() {
        let fixture = fixture_with(StubTransport::new().push_json(
            "/api/v1/lists/l1/members",
            200,
            json!({ "ok": true }),
        ));
        fixture
            .service
            .invite("l1", "ada@example.com", "member")
            .await
            .expect("invites");
        assert!(
            journal::all(&fixture.store).expect("reads").is_empty(),
            "membership does not travel through the Outbox"
        );
    }

    #[tokio::test]
    async fn leaving_a_list_removes_it_from_the_cache_once_the_server_agrees() {
        let fixture = fixture_with(StubTransport::new().push_json(
            "/api/v1/lists/l1/leave",
            200,
            json!({ "ok": true }),
        ));
        fixture
            .store
            .upsert_list(&TaskList::new("l1", "Shared"))
            .expect("stores");

        fixture.service.leave("l1").await.expect("leaves");
        assert!(fixture.store.list("l1").expect("reads").is_none());
    }

    #[tokio::test]
    async fn a_refused_leave_leaves_the_list_where_it_was() {
        let fixture = fixture_with(StubTransport::new().push_json(
            "/api/v1/lists/l1/leave",
            403,
            json!({ "error": "no" }),
        ));
        fixture
            .store
            .upsert_list(&TaskList::new("l1", "Shared"))
            .expect("stores");

        assert!(fixture.service.leave("l1").await.is_err());
        assert!(
            fixture.store.list("l1").expect("reads").is_some(),
            "the list must not vanish on a failure the user can see"
        );
    }
}
