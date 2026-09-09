//! What a comment draws: its text, its files, or nothing at all.
//!
//! Ports `MacCommentBubble` from `astrid-ios/Astrid Mac/Views/MacCommentAttachments.swift`
//! (AITD-304), whose header explains the bug it was written for and why the rules are pure.
//!
//! ## Attachments reach a task through comments
//!
//! There is no "attach to task" endpoint: a file is uploaded and the id it comes back with is
//! carried by a comment. So a file somebody attached lives on the *comment*, and a screen that
//! draws only `comment.content` shows an empty row where a screenshot should be.
//!
//! That is how this was reported on the Mac — "attaching is broken (not attaching)". It was
//! attaching. "Never uploaded" and "uploaded, never drawn" are the same picture from the outside,
//! which is the reason the empty-bubble decision is a rule with a test rather than a line in a
//! view.
//!
//! The task-level Attachments section is no help either. It lists the files on the task, and one
//! that arrived on a comment hangs off the comment.

use serde::Serialize;

use crate::model::{Comment, SecureFile};

/// One comment, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentRow {
    pub id: String,
    pub content: String,
    pub created_at: Option<String>,
    /// Who wrote it, or `None` for one the server authored.
    pub author_name: Option<String>,
    /// Still in the Outbox. Not an error — the same meaning it has in a chat transcript.
    pub is_pending: bool,
    /// Whether to draw a text bubble at all.
    pub shows_text: bool,
    /// The files this comment carries.
    pub files: Vec<FileRow>,
    /// Which side of the thread it sits on.
    ///
    /// A chat transcript is unreadable without it: every bubble on the same side is one voice, and
    /// a thread drawn all on one side says the other person never replied.
    pub is_mine: bool,
    /// Nobody wrote it — the server did. Drawn as a centred note rather than as either voice.
    pub is_system: bool,
    /// The comment this one answers, when it answers one (task 97c817dd).
    pub parent_id: Option<String>,
    /// Drawn nested under its parent. False for a reply whose parent is not in the thread, which
    /// is then an ordinary comment rather than an indent under nothing.
    pub is_reply: bool,
    /// Which side a reply is set in from: the parent author's, as the web does it — replies to
    /// my comment step in from the right, replies to yours from the left.
    pub indent_right: bool,
}

/// One file on a comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRow {
    pub id: String,
    pub name: String,
    pub size: i64,
    pub mime_type: String,
    /// Whether it is drawn in the comment or offered as a chip to open.
    pub renders_inline: bool,
    /// Where its bytes are on this machine, when they are already here.
    ///
    /// `None` is not an error — it means "not fetched yet", and the chip is what a screen draws
    /// until they are. Filled in by [`with_local_paths`] rather than by [`rows`], which stays a
    /// pure function of the comments.
    pub local_path: Option<String>,
}

/// The files a comment carries.
pub fn files_of(comment: &Comment) -> Vec<&SecureFile> {
    comment.secure_files.iter().flatten().collect()
}

/// Whether there is a caption to draw.
///
/// A caption-less attachment comment gets **no** text bubble. An empty bubble beside a photo reads
/// as a failed post, and an empty bubble on its own is exactly what the bug looked like.
pub fn shows_text(content: &str) -> bool {
    !content.trim().is_empty()
}

/// Nothing to draw at all — neither text nor a file.
pub fn is_empty(comment: &Comment) -> bool {
    !shows_text(&comment.content) && files_of(comment).is_empty()
}

/// Whether a file is drawn where it sits, or offered as something to open.
///
/// Images are shown; documents, video and audio are a chip. Guessing wider would trade a chip that
/// says what it is for a grey box that does not.
pub fn renders_inline(mime_type: &str) -> bool {
    mime_type.to_ascii_lowercase().starts_with("image/")
}

/// Project a task's comments.
///
/// A comment with neither text nor files is dropped rather than drawn as an empty row — see
/// [`is_empty`]. That is the one this module exists for.
///
/// Replies are nested (task 97c817dd). The server answers a flat list in which a reply is a
/// comment with `parentCommentId`, and the web nests them itself; so does this, putting each
/// reply right after its parent, in the order the replies were written. A reply whose parent is
/// not in the thread — deleted, or beyond the page — is drawn as an ordinary comment rather than
/// as an indent under nothing.
pub fn rows(comments: &[Comment], me: Option<&str>) -> Vec<CommentRow> {
    let drawable: Vec<&Comment> = comments
        .iter()
        .filter(|comment| !is_empty(comment))
        .collect();
    let has = |id: &str| drawable.iter().any(|comment| comment.id == id);
    let is_top_level = |comment: &Comment| {
        comment
            .parent_comment_id
            .as_deref()
            .map(|parent| !has(parent))
            .unwrap_or(true)
    };

    let mut rows = Vec::with_capacity(drawable.len());
    for parent in drawable.iter().filter(|comment| is_top_level(comment)) {
        rows.push(row(parent, me, None));
        let parent_is_mine = me.is_some() && parent.author_id.as_deref() == me;
        for reply in drawable
            .iter()
            .filter(|comment| comment.parent_comment_id.as_deref() == Some(parent.id.as_str()))
        {
            rows.push(row(reply, me, Some((&parent.id, parent_is_mine))));
        }
    }
    rows
}

/// One row. `under` is the parent it nests beneath, and whether that parent is mine.
fn row(comment: &Comment, me: Option<&str>, under: Option<(&String, bool)>) -> CommentRow {
    CommentRow {
        id: comment.id.clone(),
        content: comment.content.clone(),
        created_at: comment.created_at.map(crate::model::date::format),
        author_name: comment
            .author
            .as_ref()
            .map(|author| author.display_name().to_string()),
        is_pending: crate::model::is_temp_id(&comment.id),
        shows_text: shows_text(&comment.content),
        // Signed out, nothing is mine. Comparing `None == None` would otherwise put every
        // system comment on the reader's own side.
        is_mine: me.is_some() && comment.author_id.as_deref() == me,
        is_system: comment.author_id.is_none(),
        parent_id: under.map(|(parent, _)| parent.clone()),
        is_reply: under.is_some(),
        indent_right: under
            .map(|(_, parent_is_mine)| parent_is_mine)
            .unwrap_or(false),
        files: files_of(comment)
            .into_iter()
            .map(|file| FileRow {
                id: file.id.clone(),
                name: file.name.clone(),
                size: file.size,
                mime_type: file.mime_type.clone(),
                renders_inline: renders_inline(&file.mime_type),
                // Filled in afterwards: where the bytes are is a question about this machine,
                // and this function is a question about the comments.
                local_path: None,
            })
            .collect(),
    }
}

/// Say where each file's bytes already are, for the ones that are here.
///
/// Split from [`rows`] so the projection stays pure: which comments draw, and how, is a rule with
/// tests; where a file landed on this disk is not. `resolve` is given a file id and answers with a
/// path when the bytes are in hand — see `AttachmentService::local_path`.
///
/// Only the files that would be drawn are asked about. A resolver call for a PDF that renders as a
/// chip either way is work done to change nothing.
pub fn with_local_paths(rows: &mut [CommentRow], resolve: impl Fn(&str) -> Option<String>) {
    for row in rows.iter_mut() {
        for file in row.files.iter_mut().filter(|file| file.renders_inline) {
            file.local_path = resolve(&file.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn comment(value: serde_json::Value) -> Comment {
        serde_json::from_value(value).expect("a comment")
    }

    fn a_file(mime: &str) -> serde_json::Value {
        json!({ "id": "f1", "originalName": "shot.png", "fileSize": 1024, "mimeType": mime })
    }

    /// A reply draws under the comment it answers, set in from the parent author's side, in the
    /// order it was written — and never as a top-level comment with no sign of what it answers
    /// (task 97c817dd).
    #[test]
    fn replies_nest_under_their_parent_on_the_parent_author_s_side_task_97c817dd() {
        let rows = rows(
            &[
                comment(json!({ "id": "c1", "content": "first", "authorId": "me" })),
                comment(
                    json!({ "id": "r2", "content": "later reply to c1", "authorId": "dana", "parentCommentId": "c1", "createdAt": "2026-09-07T12:05:00Z" }),
                ),
                comment(json!({ "id": "c3", "content": "second", "authorId": "dana" })),
                comment(
                    json!({ "id": "r1", "content": "first reply to c1", "authorId": "me", "parentCommentId": "c1", "createdAt": "2026-09-07T12:01:00Z" }),
                ),
                comment(
                    json!({ "id": "r3", "content": "reply to c3", "authorId": "me", "parentCommentId": "c3" }),
                ),
            ],
            Some("me"),
        );

        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["c1", "r2", "r1", "c3", "r3"],
            "each reply follows its parent, in the order given"
        );
        assert!(!rows[0].is_reply);
        assert!(rows[1].is_reply);
        assert_eq!(rows[1].parent_id.as_deref(), Some("c1"));
        assert!(
            rows[1].indent_right,
            "c1 is mine, so its replies step in from the right"
        );
        assert!(
            !rows[4].indent_right,
            "c3 is Dana's, so its replies step in from the left"
        );
        assert!(
            !rows[1].is_mine && rows[2].is_mine,
            "a reply keeps its own author's side"
        );
    }

    /// A reply whose parent is not in the thread is an ordinary comment, not an indent under
    /// nothing.
    #[test]
    fn a_reply_without_its_parent_is_drawn_as_a_comment() {
        let rows = rows(
            &[comment(
                json!({ "id": "r1", "content": "orphan", "parentCommentId": "gone" }),
            )],
            None,
        );
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].is_reply);
        assert!(rows[0].parent_id.is_none());
    }

    /// An empty reply is dropped like an empty comment, and takes no place under its parent.
    #[test]
    fn an_empty_reply_is_not_drawn() {
        let rows = rows(
            &[
                comment(json!({ "id": "c1", "content": "first" })),
                comment(json!({ "id": "r1", "content": "  ", "parentCommentId": "c1" })),
            ],
            None,
        );
        assert_eq!(rows.len(), 1);
    }

    /// The bug in one test: a file posted with no caption was an empty row, and an empty row is
    /// what "it did not attach" looks like.
    #[test]
    fn a_file_posted_without_a_caption_draws_the_file_and_no_text() {
        let rows = rows(
            &[comment(json!({
                "id": "c1",
                "content": "",
                "secureFiles": [a_file("image/png")],
            }))],
            None,
        );

        assert_eq!(rows.len(), 1);
        assert!(!rows[0].shows_text, "no empty bubble beside the picture");
        assert_eq!(rows[0].files.len(), 1);
        assert_eq!(rows[0].files[0].name, "shot.png");
    }

    #[test]
    fn a_caption_and_a_file_draw_both() {
        let rows = rows(
            &[comment(json!({
                "id": "c1",
                "content": "here it is",
                "secureFiles": [a_file("image/png")],
            }))],
            None,
        );

        assert!(rows[0].shows_text);
        assert_eq!(rows[0].files.len(), 1);
    }

    /// Whitespace is not a caption. A comment carrying a file and a stray newline would otherwise
    /// draw a bubble with nothing in it.
    #[test]
    fn whitespace_is_not_a_caption() {
        assert!(!shows_text("   \n "));
        assert!(shows_text("x"));
    }

    /// Neither text nor a file is nothing to say. Drawing it puts an empty row in the thread for
    /// something the server sent that this build has no way to show.
    #[test]
    fn a_comment_with_nothing_in_it_is_not_drawn() {
        let rows = rows(
            &[
                comment(json!({ "id": "c1", "content": "" })),
                comment(json!({ "id": "c2", "content": "said something" })),
            ],
            None,
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "c2");
    }

    /// A picture is worth showing; a spreadsheet is worth naming. Guessing wider trades a chip
    /// that says what it is for a grey box that does not.
    #[test]
    fn images_are_drawn_and_everything_else_is_offered() {
        assert!(renders_inline("image/png"));
        assert!(
            renders_inline("IMAGE/JPEG"),
            "the header's case is not ours"
        );
        assert!(!renders_inline("video/quicktime"));
        assert!(!renders_inline("application/pdf"));
        assert!(!renders_inline(""));
    }

    /// A thread drawn all on one side says the other person never replied.
    #[test]
    fn a_comment_sits_on_the_side_of_whoever_wrote_it() {
        let rows = rows(
            &[
                comment(json!({ "id": "c1", "content": "mine", "authorId": "u1" })),
                comment(json!({ "id": "c2", "content": "theirs", "authorId": "u2" })),
                comment(json!({ "id": "c3", "content": "the server's" })),
            ],
            Some("u1"),
        );

        assert!(rows[0].is_mine);
        assert!(!rows[1].is_mine);
        assert!(!rows[2].is_mine, "nobody's is not mine");
        assert!(rows[2].is_system);
        assert!(!rows[0].is_system);
    }

    /// Signed out, nothing is mine. `None == None` would otherwise claim every system comment.
    #[test]
    fn signed_out_nothing_is_mine() {
        let rows = rows(&[comment(json!({ "id": "c1", "content": "x" }))], None);
        assert!(!rows[0].is_mine);
    }

    /// The bug AITD-308 names on the Mac: a screenshot posted from this machine, drawn from bytes
    /// this process wrote itself rather than fetched back.
    #[test]
    fn a_file_whose_bytes_are_in_hand_says_where_they_are() {
        let mut rows = rows(
            &[comment(
                json!({ "id": "c1", "content": "", "secureFiles": [a_file("image/png")] }),
            )],
            None,
        );

        with_local_paths(&mut rows, |id| {
            (id == "f1").then(|| "C:/cache/pending/f1".to_string())
        });

        assert_eq!(
            rows[0].files[0].local_path.as_deref(),
            Some("C:/cache/pending/f1")
        );
    }

    /// Not fetched yet is not an error. The chip is what a screen draws until the bytes arrive.
    #[test]
    fn a_file_that_is_not_here_yet_simply_has_no_path() {
        let mut rows = rows(
            &[comment(
                json!({ "id": "c1", "content": "", "secureFiles": [a_file("image/png")] }),
            )],
            None,
        );

        with_local_paths(&mut rows, |_| None);

        assert!(rows[0].files[0].local_path.is_none());
    }

    /// A chip looks the same whether or not its bytes are here, so asking is work done to change
    /// nothing.
    #[test]
    fn only_the_files_that_would_be_drawn_are_asked_about() {
        let mut rows = rows(
            &[comment(json!({
                "id": "c1",
                "content": "",
                "secureFiles": [a_file("image/png"), a_file("application/pdf")],
            }))],
            None,
        );

        let asked = std::cell::RefCell::new(Vec::new());
        with_local_paths(&mut rows, |id| {
            asked.borrow_mut().push(id.to_string());
            Some("somewhere".to_string())
        });

        assert_eq!(asked.borrow().len(), 1, "the picture, not the document");
        assert!(rows[0].files[0].local_path.is_some());
        assert!(rows[0].files[1].local_path.is_none());
    }

    /// The same meaning it has in a chat transcript: queued, not failed.
    #[test]
    fn a_comment_still_in_the_outbox_says_so() {
        let rows = rows(
            &[comment(json!({ "id": "temp_abc", "content": "just said" }))],
            None,
        );
        assert!(rows[0].is_pending);
    }
}
