//! Who a list is shared with: inviting, changing a role, removing, and the invitations not yet
//! accepted.
//!
//! Ported from `astrid-ios/Astrid App/Core/Services/ListMemberService.swift`.
//!
//! **Try now; queue only when the network is what failed.** A membership change is a question the
//! server answers — there may be no such person, or this account may not be allowed — and online
//! that answer has to reach the person who asked, with nothing changed. So every change is sent
//! at once, and a refusal is returned as an error. Only when the request never reached the server
//! is the change journalled, applied to the cache, and sent when the connection returns: the
//! Apple apps queued membership changes offline, and a person on a train who removes somebody
//! should not be told to try again later.
//!
//! **A queued invitation is an invitation, not a member.** It appears among the list's pending
//! invitations, which no permission check reads, so it cannot be mistaken for somebody who has
//! access (the reason the core used to refuse offline invites at all). The server's answer
//! replaces it: a member if the address belonged to an account, an invitation if not.
//!
//! Changes to one list keep their order: while one is queued, the next queues behind it rather
//! than overtaking it.

use serde::Serialize;
use serde_json::json;

use super::{Context, Result};
use crate::api::{endpoints, ApiError};
use crate::model::{ListInvite, ListMember, TaskList};
use crate::outbox::{self, journal, kind, Status};

/// What a membership change came to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberChange {
    /// Journalled, to be sent when the connection returns; the cache already shows it.
    pub queued: bool,
    /// An invitation that found an account: the new member.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<ListMember>,
    /// An invitation waiting to be accepted — the server's, or this device's while queued.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invitation: Option<ListInvite>,
}

pub struct MemberService {
    context: Context,
}

impl MemberService {
    pub fn new(context: Context) -> Self {
        MemberService { context }
    }

    /// Invite somebody by email.
    pub async fn invite(&self, list_id: &str, email: &str, role: &str) -> Result<MemberChange> {
        let body = json!({ "email": email, "role": role });
        let temp_id = outbox::new_temp_id();
        let request = self
            .context
            .client
            .post(endpoints::list_members(list_id))
            .value(with_request_id(&body, &temp_id));
        match self.try_now(list_id, request).await? {
            Some(answer) => Ok(record_invite_answer(
                &self.context.store,
                list_id,
                email,
                role,
                None,
                &answer,
            )),
            None => {
                let invitation = ListInvite {
                    id: temp_id.clone(),
                    list_id: list_id.to_string(),
                    email: email.to_string(),
                    role: role.to_string(),
                    token: String::new(),
                    created_at: Some(self.context.clock.now()),
                    created_by: None,
                };
                self.edit_list(list_id, |list| {
                    let invitations = list.invitations.get_or_insert_with(Vec::new);
                    invitations.retain(|existing| !existing.email.eq_ignore_ascii_case(email));
                    invitations.push(invitation.clone());
                })?;
                self.enqueue(
                    kind::INVITE_TO_LIST,
                    json!({ "listId": list_id, "body": body }),
                    Some(&temp_id),
                )?;
                Ok(MemberChange {
                    queued: true,
                    invitation: Some(invitation),
                    ..MemberChange::default()
                })
            }
        }
    }

    pub async fn set_role(&self, list_id: &str, user_id: &str, role: &str) -> Result<MemberChange> {
        let body = json!({ "role": role });
        let request = self
            .context
            .client
            .put(endpoints::list_member(list_id, user_id))
            .value(body.clone());
        let queued = self.try_now(list_id, request).await?.is_none();
        if queued {
            self.enqueue(
                kind::SET_MEMBER_ROLE,
                json!({ "listId": list_id, "userId": user_id, "body": body }),
                None,
            )?;
        }
        self.edit_list(list_id, |list| {
            for member in list.list_members.iter_mut().flatten() {
                if member.user_id == user_id {
                    member.role = role.to_string();
                }
            }
        })?;
        Ok(MemberChange {
            queued,
            ..MemberChange::default()
        })
    }

    pub async fn remove(&self, list_id: &str, user_id: &str) -> Result<MemberChange> {
        let request = self
            .context
            .client
            .delete(endpoints::list_member(list_id, user_id));
        let queued = self.try_now_allowing_gone(list_id, request).await?;
        if queued {
            self.enqueue(
                kind::REMOVE_MEMBER,
                json!({ "listId": list_id, "userId": user_id }),
                None,
            )?;
        }
        self.edit_list(list_id, |list| {
            if let Some(members) = list.list_members.as_mut() {
                members.retain(|member| member.user_id != user_id);
            }
        })?;
        Ok(MemberChange {
            queued,
            ..MemberChange::default()
        })
    }

    /// Withdraw an invitation not yet accepted. Addressed by email: an unaccepted invitation has no
    /// user to name, and there may not even be an account yet (AITD-388).
    pub async fn cancel_invitation(&self, list_id: &str, email: &str) -> Result<MemberChange> {
        // One still queued here never reached the server: withdrawing it is withdrawing the entry.
        if self.withdraw_queued_invite(list_id, email)? {
            return Ok(MemberChange::default());
        }
        let body = json!({ "email": email });
        let request = self
            .context
            .client
            .delete(endpoints::list_invitations(list_id))
            .value(body.clone());
        let queued = self.try_now_allowing_gone(list_id, request).await?;
        if queued {
            self.enqueue(
                kind::CANCEL_INVITATION,
                json!({ "listId": list_id, "body": body }),
                None,
            )?;
        }
        self.edit_list(list_id, |list| {
            if let Some(invitations) = list.invitations.as_mut() {
                invitations.retain(|invite| !invite.email.eq_ignore_ascii_case(email));
            }
        })?;
        Ok(MemberChange {
            queued,
            ..MemberChange::default()
        })
    }

    /// Change an invitation's role before it is accepted — the invitation twin of [`Self::set_role`].
    pub async fn set_invitation_role(
        &self,
        list_id: &str,
        email: &str,
        role: &str,
    ) -> Result<MemberChange> {
        let body = json!({ "email": email, "role": role });
        let request = self
            .context
            .client
            .put(endpoints::list_invitations(list_id))
            .value(body.clone());
        let queued = self.try_now(list_id, request).await?.is_none();
        if queued {
            self.enqueue(
                kind::SET_INVITATION_ROLE,
                json!({ "listId": list_id, "body": body }),
                None,
            )?;
        }
        self.edit_list(list_id, |list| {
            for invite in list.invitations.iter_mut().flatten() {
                if invite.email.eq_ignore_ascii_case(email) {
                    invite.role = role.to_string();
                }
            }
        })?;
        Ok(MemberChange {
            queued,
            ..MemberChange::default()
        })
    }

    // ─── How a change is sent ─────────────────────────────────────────────────────────────────

    /// Send now. `Some(answer)` when the server took it, `None` when it must be queued — the
    /// network failed, or an earlier change to this list is still waiting — and an error when the
    /// server refused it.
    async fn try_now(
        &self,
        list_id: &str,
        request: crate::api::Request,
    ) -> Result<Option<serde_json::Value>> {
        if self.has_queued_changes(list_id)? {
            return Ok(None);
        }
        match self.context.client.send(request).await {
            Ok(answer) => Ok(Some(answer)),
            Err(ApiError::Transport(_)) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// [`Self::try_now`] for a removal, where "already gone" is the outcome asked for. Answers
    /// whether it was queued.
    async fn try_now_allowing_gone(
        &self,
        list_id: &str,
        request: crate::api::Request,
    ) -> Result<bool> {
        match self.try_now(list_id, request).await {
            Ok(answer) => Ok(answer.is_none()),
            Err(super::ServiceError::Api(error)) if error.status() == Some(404) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn has_queued_changes(&self, list_id: &str) -> Result<bool> {
        Ok(journal::all(&self.context.store)?.iter().any(|entry| {
            is_member_kind(&entry.kind)
                && matches!(entry.status, Status::Pending | Status::Running)
                && entry.payload.get("listId").and_then(|id| id.as_str()) == Some(list_id)
        }))
    }

    fn withdraw_queued_invite(&self, list_id: &str, email: &str) -> Result<bool> {
        let queued = journal::all(&self.context.store)?
            .into_iter()
            .find(|entry| {
                entry.kind == kind::INVITE_TO_LIST
                    && entry.status == Status::Pending
                    && entry.payload.get("listId").and_then(|id| id.as_str()) == Some(list_id)
                    && entry.payload["body"]["email"]
                        .as_str()
                        .is_some_and(|queued| queued.eq_ignore_ascii_case(email))
            });
        let Some(entry) = queued else {
            return Ok(false);
        };
        journal::remove(&self.context.store, &entry.id)?;
        self.edit_list(list_id, |list| {
            if let Some(invitations) = list.invitations.as_mut() {
                invitations.retain(|invite| !invite.email.eq_ignore_ascii_case(email));
            }
        })?;
        Ok(true)
    }

    fn enqueue(
        &self,
        entry_kind: &str,
        payload: serde_json::Value,
        temp_id: Option<&str>,
    ) -> Result<()> {
        let key = temp_id
            .map(str::to_string)
            .unwrap_or_else(outbox::new_temp_id);
        let entry = outbox::build(entry_kind, payload, &key, self.context.clock.now());
        let entry = match temp_id {
            Some(temp_id) => entry.for_temp_id(temp_id),
            None => entry,
        };
        journal::enqueue(&self.context.store, &entry)?;
        Ok(())
    }

    fn edit_list(&self, list_id: &str, edit: impl FnOnce(&mut TaskList)) -> Result<()> {
        if let Some(mut list) = self.context.store.list(list_id)? {
            edit(&mut list);
            self.context.store.upsert_list(&list)?;
        }
        Ok(())
    }
}

/// The kinds this module journals.
pub(crate) fn is_member_kind(entry_kind: &str) -> bool {
    matches!(
        entry_kind,
        kind::INVITE_TO_LIST
            | kind::SET_MEMBER_ROLE
            | kind::REMOVE_MEMBER
            | kind::CANCEL_INVITATION
            | kind::SET_INVITATION_ROLE
    )
}

fn with_request_id(body: &serde_json::Value, request_id: &str) -> serde_json::Value {
    let mut body = body.clone();
    body["clientRequestId"] = json!(request_id);
    body
}

/// Read `/lists/{id}/members`: the members, and the invitations waiting beside them.
///
/// The web flattens the person into each row — `{ id, name, email, image, role, isAIAgent,
/// type }`, `id` being the user's — and lists pending invitations as rows of `type: "invite"`
/// whose id is `invite_<invitation id>`. A row already in the `ListMember` shape (`userId`) is
/// read as one.
pub(crate) fn read_roster(
    list_id: &str,
    rows: &serde_json::Value,
) -> (Vec<ListMember>, Vec<ListInvite>) {
    let mut members = Vec::new();
    let mut invitations = Vec::new();
    for row in rows.as_array().into_iter().flatten() {
        let text = |key: &str| {
            row.get(key)
                .and_then(|value| value.as_str())
                .map(str::to_string)
        };
        if row.get("type").and_then(|kind| kind.as_str()) == Some("invite") {
            let id = text("id").unwrap_or_default();
            invitations.push(ListInvite {
                id: id.strip_prefix("invite_").unwrap_or(&id).to_string(),
                list_id: list_id.to_string(),
                email: text("email").unwrap_or_default(),
                role: text("role").unwrap_or_default(),
                token: String::new(),
                created_at: None,
                created_by: None,
            });
        } else if let Some(member) = read_member(list_id, row) {
            members.push(member);
        }
    }
    (members, invitations)
}

/// One member row, flattened or not.
fn read_member(list_id: &str, row: &serde_json::Value) -> Option<ListMember> {
    if row.get("userId").is_some() {
        return serde_json::from_value(row.clone()).ok();
    }
    let user: crate::model::User = serde_json::from_value(row.clone()).ok()?;
    Some(ListMember {
        id: None,
        list_id: Some(list_id.to_string()),
        user_id: user.id.clone(),
        role: row
            .get("role")
            .and_then(|role| role.as_str())
            .unwrap_or("member")
            .to_string(),
        created_at: None,
        updated_at: None,
        user: Some(user),
    })
}

/// Put the server's answer to an invitation in the cached list: the member it made, or the
/// invitation it holds — replacing this device's queued invitation, `queued_id`, when there was
/// one. Shared with the Outbox handler that delivers a queued invitation.
pub(crate) fn record_invite_answer(
    store: &crate::store::Store,
    list_id: &str,
    email: &str,
    role: &str,
    queued_id: Option<&str>,
    answer: &serde_json::Value,
) -> MemberChange {
    let member = answer
        .get("member")
        .filter(|member| !member.is_null())
        .and_then(|member| read_member(list_id, member));
    let invitation = match &member {
        Some(_) => None,
        None => Some(ListInvite {
            id: answer
                .pointer("/invitation/id")
                .and_then(|id| id.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| format!("invite_{email}")),
            list_id: list_id.to_string(),
            email: email.to_string(),
            role: answer
                .pointer("/invitation/role")
                .and_then(|role| role.as_str())
                .unwrap_or(role)
                .to_string(),
            token: String::new(),
            created_at: None,
            created_by: None,
        }),
    };
    if let Ok(Some(mut list)) = store.list(list_id) {
        let invitations = list.invitations.get_or_insert_with(Vec::new);
        invitations.retain(|invite| {
            Some(invite.id.as_str()) != queued_id && !invite.email.eq_ignore_ascii_case(email)
        });
        if let Some(invitation) = &invitation {
            invitations.push(invitation.clone());
        }
        if let Some(member) = &member {
            let members = list.list_members.get_or_insert_with(Vec::new);
            members.retain(|existing| existing.user_id != member.user_id);
            members.push(member.clone());
        }
        let _ = store.upsert_list(&list);
    }
    MemberChange {
        queued: false,
        member,
        invitation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ApiClient, StubTransport, TransportError};
    use crate::platform::{FixedClock, MemorySecureStore};
    use crate::store::Store;
    use std::sync::{Arc, Mutex};

    struct Fixture {
        service: MemberService,
        store: Arc<Store>,
        sent: Arc<Mutex<Vec<crate::api::HttpRequest>>>,
    }

    impl Fixture {
        fn list(&self) -> TaskList {
            self.store.list("l1").expect("reads").expect("cached")
        }
        fn journal(&self) -> Vec<outbox::Entry> {
            journal::all(&self.store).expect("reads")
        }
    }

    fn fixture(transport: StubTransport) -> Fixture {
        let sent = transport.recorded.clone();
        let store = Arc::new(Store::in_memory().expect("opens"));
        let list: TaskList = serde_json::from_value(json!({
            "id": "l1", "name": "Shared", "ownerId": "me",
            "listMembers": [
                { "userId": "me", "role": "owner" },
                { "userId": "dana", "role": "member" }
            ]
        }))
        .expect("a list");
        store.upsert_list(&list).expect("stores");
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
            service: MemberService::new(context),
            store,
            sent,
        }
    }

    fn offline() -> StubTransport {
        StubTransport::new().fallback(Err(TransportError::Unreachable("offline".into())))
    }

    /// Online, the server's answer is the answer: a member it made lands on the list, and nothing
    /// is journalled.
    #[tokio::test]
    async fn an_invitation_online_goes_to_the_server_and_its_member_lands() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/lists/l1/members",
            200,
            json!({ "member": { "userId": "ada", "role": "member", "user": { "id": "ada", "email": "ada@example.com" } } }),
        ));
        let change = fixture
            .service
            .invite("l1", "ada@example.com", "member")
            .await
            .expect("invites");
        assert!(!change.queued);
        assert_eq!(change.member.expect("a member").user_id, "ada");
        assert!(fixture.journal().is_empty());
        assert!(fixture
            .list()
            .list_members
            .expect("members")
            .iter()
            .any(|member| member.user_id == "ada"));
    }

    /// A refusal reaches the person who asked, and nothing changes.
    #[tokio::test]
    async fn a_refusal_online_is_an_error_and_changes_nothing() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/lists/l1/members/dana",
            403,
            json!({ "error": "not allowed" }),
        ));
        assert!(fixture.service.remove("l1", "dana").await.is_err());
        assert_eq!(fixture.list().list_members.expect("members").len(), 2);
        assert!(fixture.journal().is_empty());
    }

    /// Offline an invitation is queued and shown as an invitation — never as a member, which a
    /// permission check would read.
    #[tokio::test]
    async fn an_invitation_offline_is_queued_as_an_invitation_not_a_member() {
        let fixture = fixture(offline());
        let change = fixture
            .service
            .invite("l1", "ada@example.com", "admin")
            .await
            .expect("queues");
        assert!(change.queued);
        let list = fixture.list();
        assert_eq!(
            list.list_members.expect("members").len(),
            2,
            "no member was added"
        );
        let invitations = list.invitations.expect("invitations");
        assert_eq!(invitations[0].email, "ada@example.com");
        assert_eq!(invitations[0].role, "admin");
        let journal = fixture.journal();
        assert_eq!(journal[0].kind, kind::INVITE_TO_LIST);
        assert_eq!(
            journal[0].temp_id.as_deref(),
            Some(invitations[0].id.as_str())
        );
    }

    /// Offline removals and role changes show at once and are queued; and once one change to a
    /// list is queued, the next waits behind it even online — they must not arrive out of order.
    #[tokio::test]
    async fn changes_offline_show_at_once_and_keep_their_order() {
        let fixture = fixture(offline());
        assert!(
            fixture
                .service
                .set_role("l1", "dana", "admin")
                .await
                .expect("queues")
                .queued
        );
        assert_eq!(
            fixture.list().list_members.expect("members")[1].role,
            "admin"
        );

        let before = fixture.sent.lock().expect("lock").len();
        let removed = fixture.service.remove("l1", "dana").await.expect("queues");
        assert!(removed.queued);
        assert_eq!(
            fixture.sent.lock().expect("lock").len(),
            before,
            "queued behind, not sent"
        );
        assert_eq!(fixture.list().list_members.expect("members").len(), 1);
        let kinds: Vec<String> = fixture
            .journal()
            .into_iter()
            .map(|entry| entry.kind)
            .collect();
        assert_eq!(kinds, vec![kind::SET_MEMBER_ROLE, kind::REMOVE_MEMBER]);
    }

    /// Withdrawing an invitation that never left this device withdraws the entry; nothing is sent.
    #[tokio::test]
    async fn cancelling_a_queued_invitation_withdraws_it() {
        let fixture = fixture(offline());
        fixture
            .service
            .invite("l1", "ada@example.com", "member")
            .await
            .expect("queues");
        let before = fixture.sent.lock().expect("lock").len();
        let change = fixture
            .service
            .cancel_invitation("l1", "ADA@example.com")
            .await
            .expect("cancels");
        assert!(!change.queued);
        assert!(fixture.journal().is_empty());
        assert!(fixture.list().invitations.expect("invitations").is_empty());
        assert_eq!(fixture.sent.lock().expect("lock").len(), before);
    }

    /// Removing somebody already gone is the outcome asked for, not an error.
    #[tokio::test]
    async fn removing_somebody_already_gone_succeeds() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/lists/l1/members/dana",
            404,
            json!({ "error": "not found" }),
        ));
        let change = fixture.service.remove("l1", "dana").await.expect("removes");
        assert!(!change.queued);
        assert_eq!(fixture.list().list_members.expect("members").len(), 1);
    }

    /// Delivered later, the queued invitation is replaced by what the server made of it.
    #[test]
    fn a_delivered_invitation_is_replaced_by_the_servers_answer() {
        let fixture = fixture(StubTransport::new());
        let mut list = fixture.list();
        list.invitations = Some(vec![ListInvite {
            id: "temp_i".into(),
            list_id: "l1".into(),
            email: "ada@example.com".into(),
            role: "member".into(),
            token: String::new(),
            created_at: None,
            created_by: None,
        }]);
        fixture.store.upsert_list(&list).expect("stores");

        record_invite_answer(
            &fixture.store,
            "l1",
            "ada@example.com",
            "member",
            Some("temp_i"),
            &json!({ "member": { "userId": "ada", "role": "member" } }),
        );
        let list = fixture.list();
        assert!(list.invitations.expect("invitations").is_empty());
        assert!(list
            .list_members
            .expect("members")
            .iter()
            .any(|member| member.user_id == "ada"));
    }

    /// The web's roster flattens the person into each row and lists pending invitations beside
    /// the members (`app/api/v1/lists/[id]/members/route.ts`). Read as `ListMember`s every row was
    /// dropped and the cached roster wiped; each row now lands where it belongs.
    #[test]
    fn the_webs_roster_rows_become_members_and_invitations() {
        let rows = json!([
            { "id": "me", "name": "Jon", "email": "jon@example.com", "image": null,
              "role": "owner", "isOwner": true, "isAdmin": false, "isAIAgent": false, "type": "member" },
            { "id": "bot", "name": "Claude", "email": "claude@astrid.cc", "role": "member",
              "isOwner": false, "isAdmin": false, "isAIAgent": true, "type": "member" },
            { "id": "invite_i1", "name": null, "email": "ada@example.com", "role": "admin",
              "isOwner": false, "isAIAgent": false, "type": "invite" }
        ]);
        let (members, invitations) = read_roster("l1", &rows);
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].user_id, "me");
        assert_eq!(members[0].role, "owner");
        assert_eq!(
            members[0]
                .user
                .as_ref()
                .and_then(|user| user.name.as_deref()),
            Some("Jon")
        );
        assert!(members[1].user.as_ref().expect("a user").is_agent());
        assert_eq!(invitations.len(), 1);
        assert_eq!(invitations[0].id, "i1");
        assert_eq!(invitations[0].email, "ada@example.com");
        assert_eq!(invitations[0].role, "admin");
    }

    /// The invite answer's `member` is the same flattened row.
    #[tokio::test]
    async fn an_invitation_that_found_an_account_is_read_from_the_webs_row() {
        let fixture = fixture(StubTransport::new().push_json(
            "/api/v1/lists/l1/members",
            200,
            json!({ "message": "added", "member": {
                "id": "ada", "name": "Ada", "email": "ada@example.com", "role": "member",
                "isOwner": false, "isAdmin": false
            } }),
        ));
        let change = fixture
            .service
            .invite("l1", "ada@example.com", "member")
            .await
            .expect("invites");
        let member = change.member.expect("a member, not an invitation");
        assert_eq!(member.user_id, "ada");
        assert!(change.invitation.is_none());
    }
}
