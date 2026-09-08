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
pub fn rows(comments: &[Comment], me: Option<&str>) -> Vec<CommentRow> {
    comments
        .iter()
        .filter(|comment| !is_empty(comment))
        .map(|comment| CommentRow {
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
            files: files_of(comment)
                .into_iter()
                .map(|file| FileRow {
                    id: file.id.clone(),
                    name: file.name.clone(),
                    size: file.size,
                    mime_type: file.mime_type.clone(),
                    renders_inline: renders_inline(&file.mime_type),
                })
                .collect(),
        })
        .collect()
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
