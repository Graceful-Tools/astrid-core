//! A question to a person is a recorded state, not a marker in a comment (AWTD-1687).
//!
//! Until this, an agent asked by writing a Decision-format comment ending in
//! `AGENT-HANDBACK: <mailbox>` and reassigning the task; the server recognised the answer by
//! parsing that line. It worked, but only Astrid's own loops knew the convention, and the board
//! could not say "these tasks are waiting on your answer" as anything but "Inbox, assigned to you".
//!
//! **Canonical here**, the way [`crate::waiting`] is. The contract is
//! `contracts/fixtures/questions.json`, generated from this module (`cargo xtask
//! gen-core-contracts`).
//!
//! ## Q1 — an open question is a fact on the task
//!
//! [`ask`] records who asked, who must answer, the question comment, and what the asker needs to
//! resume (`resume_status_role`: Ready or Doing). It answers the [`Question`] to store and the
//! hand-back write ([`crate::waiting::hand_back`]: Inbox, assigned to the answerer — R4).
//! A task holds at most one open question; a finished task cannot ask; nobody asks themselves.
//!
//! ## Q2 — an answer closes it and returns the task to the asker
//!
//! [`answer`]: a reply (or a tapped Decision card) *by the answerer* closes the question and
//! writes the task back to the asker in the role it asked to resume in. Anyone else's reply is
//! refused, so a comment from a bystander does not wake the asker. A reply is never approval to
//! migrate or release; that is the orchestration policy's job.
//!
//! ## Q3 — asker and answerer are people or agents
//!
//! The rules take ids, not kinds: one agent can ask another, a person can ask an agent. Only
//! `asker_is_agent` is carried, so an answer says whether it resumes an agent's run ([`Resume`]).
//!
//! ## Q4 — questions waiting on me
//!
//! [`waiting_on`]: the open questions a person must answer, across every list, oldest first.
//!
//! ## Q5 — the text marker, for one release
//!
//! [`marker_mailbox`] reads `AGENT-HANDBACK: <mailbox>` from a comment, so a server can keep
//! accepting a question asked the old way while it moves to the recorded one.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::board::{DOING_STATUS_ROLE, READY_STATUS_ROLE};
use crate::model::date;
use crate::waiting::{hand_back, HandBack};

/// A question is already open on this task (Q1).
pub const QUESTION_ALREADY_OPEN: &str = "QUESTION_ALREADY_OPEN";
/// A finished task cannot ask (Q1).
pub const QUESTION_TASK_DONE: &str = "QUESTION_TASK_DONE";
/// Nobody asks themselves, and a question names both people (Q1).
pub const QUESTION_NEEDS_ANOTHER_PERSON: &str = "QUESTION_NEEDS_ANOTHER_PERSON";
/// An asker resumes in Ready or Doing, nothing else (Q1).
pub const QUESTION_BAD_RESUME_ROLE: &str = "QUESTION_BAD_RESUME_ROLE";
/// The question is already answered (Q2).
pub const QUESTION_CLOSED: &str = "QUESTION_CLOSED";
/// Only the person asked can answer (Q2).
pub const QUESTION_NOT_ANSWERER: &str = "QUESTION_NOT_ANSWERER";
/// The reply is older than the question (Q2).
pub const QUESTION_REPLY_BEFORE_ASK: &str = "QUESTION_REPLY_BEFORE_ASK";

/// Why a question was refused; each is `refused` with the code as `code`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionError {
    AlreadyOpen,
    TaskDone,
    NeedsAnotherPerson,
    BadResumeRole,
    Closed,
    NotAnswerer,
    ReplyBeforeAsk,
}

impl QuestionError {
    pub fn code(self) -> &'static str {
        match self {
            QuestionError::AlreadyOpen => QUESTION_ALREADY_OPEN,
            QuestionError::TaskDone => QUESTION_TASK_DONE,
            QuestionError::NeedsAnotherPerson => QUESTION_NEEDS_ANOTHER_PERSON,
            QuestionError::BadResumeRole => QUESTION_BAD_RESUME_ROLE,
            QuestionError::Closed => QUESTION_CLOSED,
            QuestionError::NotAnswerer => QUESTION_NOT_ANSWERER,
            QuestionError::ReplyBeforeAsk => QUESTION_REPLY_BEFORE_ASK,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            QuestionError::AlreadyOpen => "This task already has an open question",
            QuestionError::TaskDone => "A finished task cannot ask a question",
            QuestionError::NeedsAnotherPerson => "A question needs someone else to answer it",
            QuestionError::BadResumeRole => "A question resumes in Ready or Doing",
            QuestionError::Closed => "This question is already answered",
            QuestionError::NotAnswerer => "Only the person asked can answer this question",
            QuestionError::ReplyBeforeAsk => "A reply cannot come before the question",
        }
    }
}

/// Whether a question is still waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub enum QuestionState {
    Open,
    Answered,
}

/// The recorded question: what a server stores on the task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub id: String,
    pub task_id: String,
    pub list_id: String,
    pub asker_id: String,
    pub asker_is_agent: bool,
    pub answerer_id: String,
    /// The comment that states the question.
    pub question_comment_id: String,
    /// What the asker needs to resume: `ready` or `doing`.
    pub resume_status_role: String,
    #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
    #[serde(with = "date::required")]
    pub asked_at: DateTime<Utc>,
    pub state: QuestionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_comment_id: Option<String>,
    #[cfg_attr(feature = "schema", schemars(with = "Option<date::Timestamp>"))]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "date::optional"
    )]
    pub answered_at: Option<DateTime<Utc>>,
}

/// What asking writes: the question to store, the hand-back, and who to notify.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asked {
    pub question: Question,
    pub hand_back: HandBack,
    pub notify: Vec<String>,
}

/// What a person's reply is, for [`answer`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Reply {
    pub comment_id: String,
    pub author_id: String,
    #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
    #[serde(with = "date::required")]
    pub at: DateTime<Utc>,
}

/// The task's write when its question is answered: back to the asker, open, in the role it
/// asked to resume in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReturnWrite {
    pub status_role: String,
    pub assignee_id: String,
    pub completed: bool,
}

/// What an agent needs to resume its run after the answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resume {
    pub question_id: String,
    pub question_comment_id: String,
    pub reply_comment_id: String,
}

/// What answering writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answered {
    pub question: Question,
    pub write: ReturnWrite,
    pub notify: Vec<String>,
    /// Present when the asker is an agent: the attention item its next queue read carries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume: Option<Resume>,
}

/// Q1: everything [`ask`] reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct AskInput {
    pub question_id: String,
    pub task_id: String,
    pub list_id: String,
    #[serde(default)]
    pub task_completed: bool,
    /// The id of the question already open on the task, if any.
    #[serde(default)]
    pub open_question_id: Option<String>,
    pub asker_id: String,
    #[serde(default)]
    pub asker_is_agent: bool,
    pub answerer_id: String,
    pub question_comment_id: String,
    pub resume_status_role: String,
    #[cfg_attr(feature = "schema", schemars(with = "date::Timestamp"))]
    #[serde(with = "date::required")]
    pub now: DateTime<Utc>,
}

/// Q1: record a question and hand the task to the person asked.
pub fn ask(input: &AskInput) -> Result<Asked, QuestionError> {
    let asker = input.asker_id.trim();
    let answerer = input.answerer_id.trim();
    if asker.is_empty() || answerer.is_empty() || asker == answerer {
        return Err(QuestionError::NeedsAnotherPerson);
    }
    if input.task_completed {
        return Err(QuestionError::TaskDone);
    }
    if input.open_question_id.is_some() {
        return Err(QuestionError::AlreadyOpen);
    }
    let role = input.resume_status_role.as_str();
    if role != READY_STATUS_ROLE && role != DOING_STATUS_ROLE {
        return Err(QuestionError::BadResumeRole);
    }
    Ok(Asked {
        question: Question {
            id: input.question_id.clone(),
            task_id: input.task_id.clone(),
            list_id: input.list_id.clone(),
            asker_id: asker.to_string(),
            asker_is_agent: input.asker_is_agent,
            answerer_id: answerer.to_string(),
            question_comment_id: input.question_comment_id.clone(),
            resume_status_role: role.to_string(),
            asked_at: input.now,
            state: QuestionState::Open,
            answer_comment_id: None,
            answered_at: None,
        },
        hand_back: hand_back(answerer),
        notify: vec![answerer.to_string()],
    })
}

/// Q2: close a question with the answerer's reply and return the task to the asker.
pub fn answer(question: &Question, reply: &Reply) -> Result<Answered, QuestionError> {
    if question.state != QuestionState::Open {
        return Err(QuestionError::Closed);
    }
    if reply.author_id != question.answerer_id {
        return Err(QuestionError::NotAnswerer);
    }
    if reply.at < question.asked_at {
        return Err(QuestionError::ReplyBeforeAsk);
    }
    let closed = Question {
        state: QuestionState::Answered,
        answer_comment_id: Some(reply.comment_id.clone()),
        answered_at: Some(reply.at),
        ..question.clone()
    };
    Ok(Answered {
        write: ReturnWrite {
            status_role: question.resume_status_role.clone(),
            assignee_id: question.asker_id.clone(),
            completed: false,
        },
        notify: vec![question.asker_id.clone()],
        resume: question.asker_is_agent.then(|| Resume {
            question_id: question.id.clone(),
            question_comment_id: question.question_comment_id.clone(),
            reply_comment_id: reply.comment_id.clone(),
        }),
        question: closed,
    })
}

/// Q4: the open questions `person_id` must answer, oldest first, across every list.
pub fn waiting_on<'a>(questions: &'a [Question], person_id: &str) -> Vec<&'a Question> {
    let mut mine: Vec<&Question> = questions
        .iter()
        .filter(|q| q.state == QuestionState::Open && q.answerer_id == person_id)
        .collect();
    mine.sort_by(|a, b| a.asked_at.cmp(&b.asked_at).then_with(|| a.id.cmp(&b.id)));
    mine
}

/// Q5: the mailbox a comment's `AGENT-HANDBACK: <mailbox>` line names, if it has one.
pub fn marker_mailbox(comment: &str) -> Option<&str> {
    comment.lines().find_map(|line| {
        line.trim()
            .strip_prefix("AGENT-HANDBACK:")
            .map(str::trim)
            .filter(|mailbox| !mailbox.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        date::parse(text).expect("an instant")
    }

    fn input() -> AskInput {
        AskInput {
            question_id: "q1".into(),
            task_id: "t1".into(),
            list_id: "l1".into(),
            task_completed: false,
            open_question_id: None,
            asker_id: "claude".into(),
            asker_is_agent: true,
            answerer_id: "jon".into(),
            question_comment_id: "c1".into(),
            resume_status_role: "doing".into(),
            now: at("2026-11-16T10:00:00.000Z"),
        }
    }

    #[test]
    fn asking_hands_the_task_to_the_answerer_awtd_1687() {
        let asked = ask(&input()).unwrap();
        assert_eq!(asked.hand_back, hand_back("jon"));
        assert_eq!(asked.question.state, QuestionState::Open);
        assert_eq!(asked.notify, vec!["jon"]);
    }

    #[test]
    fn asking_is_refused_when_it_cannot_be_a_question_awtd_1687() {
        let refuse = |edit: fn(&mut AskInput)| {
            let mut i = input();
            edit(&mut i);
            ask(&i).unwrap_err()
        };
        assert_eq!(
            refuse(|i| i.answerer_id = "claude".into()),
            QuestionError::NeedsAnotherPerson
        );
        assert_eq!(refuse(|i| i.task_completed = true), QuestionError::TaskDone);
        assert_eq!(
            refuse(|i| i.open_question_id = Some("q0".into())),
            QuestionError::AlreadyOpen
        );
        assert_eq!(
            refuse(|i| i.resume_status_role = "waiting".into()),
            QuestionError::BadResumeRole
        );
    }

    fn reply(author: &str, when: &str) -> Reply {
        Reply {
            comment_id: "c2".into(),
            author_id: author.into(),
            at: at(when),
        }
    }

    #[test]
    fn the_answer_returns_the_task_to_the_asker_awtd_1687() {
        let q = ask(&input()).unwrap().question;
        let done = answer(&q, &reply("jon", "2026-11-16T11:00:00.000Z")).unwrap();
        assert_eq!(done.write.assignee_id, "claude");
        assert_eq!(done.write.status_role, "doing");
        assert_eq!(done.question.state, QuestionState::Answered);
        assert_eq!(done.resume.unwrap().reply_comment_id, "c2");
        assert_eq!(
            answer(&done.question, &reply("jon", "2026-11-16T12:00:00.000Z")).unwrap_err(),
            QuestionError::Closed
        );
    }

    #[test]
    fn only_the_answerer_closes_it_and_never_backwards_awtd_1687() {
        let q = ask(&input()).unwrap().question;
        assert_eq!(
            answer(&q, &reply("bystander", "2026-11-16T11:00:00.000Z")).unwrap_err(),
            QuestionError::NotAnswerer
        );
        assert_eq!(
            answer(&q, &reply("jon", "2026-11-16T09:00:00.000Z")).unwrap_err(),
            QuestionError::ReplyBeforeAsk
        );
    }

    #[test]
    fn a_person_asking_an_agent_resumes_no_agent_run_awtd_1687() {
        let mut i = input();
        (i.asker_id, i.asker_is_agent, i.answerer_id) = ("jon".into(), false, "claude".into());
        let q = ask(&i).unwrap().question;
        assert!(answer(&q, &reply("claude", "2026-11-16T11:00:00.000Z"))
            .unwrap()
            .resume
            .is_none());
    }

    #[test]
    fn waiting_on_lists_only_my_open_questions_oldest_first_awtd_1687() {
        let mk = |id: &str, who: &str, when: &str| {
            let mut i = input();
            (i.question_id, i.answerer_id, i.now) = (id.into(), who.into(), at(when));
            ask(&i).unwrap().question
        };
        let mut closed = mk("q0", "jon", "2026-11-15T10:00:00.000Z");
        closed.state = QuestionState::Answered;
        let all = vec![
            mk("q2", "jon", "2026-11-16T10:00:00.000Z"),
            mk("q3", "ana", "2026-11-16T09:00:00.000Z"),
            mk("q1", "jon", "2026-11-14T10:00:00.000Z"),
            closed,
        ];
        let ids: Vec<&str> = waiting_on(&all, "jon")
            .iter()
            .map(|q| q.id.as_str())
            .collect();
        assert_eq!(ids, vec!["q1", "q2"]);
    }

    #[test]
    fn the_text_marker_is_still_read_awtd_1687() {
        assert_eq!(
            marker_mailbox("Which?\n\nAGENT-HANDBACK: claude\n"),
            Some("claude")
        );
        assert_eq!(marker_mailbox("no marker"), None);
        assert_eq!(marker_mailbox("AGENT-HANDBACK:  "), None);
    }
}
