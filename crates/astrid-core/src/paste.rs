//! What is on the clipboard, and which of it somebody meant to attach.
//!
//! Ports `MacCommentPaste` from `astrid-ios/Astrid Mac/Views/MacCommentPaste.swift` (AITD-306).
//!
//! Before this, the file picker was the only route a file had into a comment: nothing read the
//! clipboard, so Ctrl+V with a screenshot on the board did nothing at all — the comment box found
//! no text to type, and nobody else was looking.
//!
//! This adds a *source*, not a second pipeline. Everything downstream already works and is shared:
//! the queue in [`crate::services::attachment`], the Outbox upload, the comment that carries the
//! file. A paste fills the same path the Attach button does.
//!
//! The rules live here, pure, so "which of the three things on this clipboard did they mean" is a
//! test rather than something you can only find out by copying a picture and trying it.
//!
//! ## Where this differs from the Mac
//!
//! The Mac also decides *whether* a paste is ours, because its handler is global and has to keep
//! its hands off a paste into the title or the notes. On Windows the handler is bound to the
//! comment box itself, so that question is answered by where it lives rather than by a predicate.
//! A clipboard with nothing attachable on it is still [`Paste::Text`], which is what makes an
//! ordinary paste an ordinary paste.

use chrono::{DateTime, FixedOffset, Utc};

/// How many files one paste may attach.
///
/// The same ceiling the Apple clients put on a staged batch, so a person who pastes twelve files
/// gets the same answer on both.
pub const MAX_FILES: usize = 10;

/// What the shell found on the clipboard. Reading it is the shell's job; deciding what it *means*
/// is this module's, which is why the two are split.
#[derive(Debug, Clone, Default)]
pub struct Clipboard {
    /// Files the board names on disk, in the order it lists them.
    pub files: Vec<String>,
    /// The extension of a raw image rendition with no file behind it — a screenshot, or a copied
    /// region. `None` when there is no image on the board.
    pub image_extension: Option<String>,
    /// Whether there is text. Never on its own a reason to attach anything.
    pub has_text: bool,
}

/// What a paste should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Paste {
    /// Attach these, in this order. Already capped.
    Files(Vec<String>),
    /// Save the clipboard's image under this name, and attach that.
    Image { name: String },
    /// Not ours. Let it type.
    Text,
}

/// The name a nameless rendition gets.
///
/// It carries the moment it was pasted, because two pastes a minute apart must not collide — the
/// same shape Explorer and the Apple clients give a pasted image. In the reader's own time, since
/// this is a filename they will read: a screenshot taken at nine in the morning saying `17.00`
/// would be a small daily confusion.
///
/// The format is fixed rather than localised. It names a file, so a machine set to another region
/// must not sort its staged files differently.
pub fn pasted_image_name(now: DateTime<Utc>, offset: FixedOffset, extension: &str) -> String {
    let local = now.with_timezone(&offset);
    let extension = match extension.trim().trim_start_matches('.') {
        "" => "png",
        given => given,
    };
    format!(
        "Pasted Image {}.{extension}",
        local.format("%Y-%m-%d at %H.%M.%S")
    )
}

/// What this paste should attach, if anything.
///
/// **Files win over the rendition.** Copying a PNG in Explorer puts both on the board; taking both
/// would attach the same picture twice, and the file is the copy that keeps the real name and the
/// original bytes rather than a re-encode.
///
/// **A text-only clipboard is never intercepted.** That is the most common paste in the app, and
/// breaking it to serve the rarest one would be a bad trade.
pub fn decide(board: &Clipboard, now: DateTime<Utc>, offset: FixedOffset) -> Paste {
    let files: Vec<String> = board
        .files
        .iter()
        .filter(|path| !path.trim().is_empty())
        .take(MAX_FILES)
        .cloned()
        .collect();
    if !files.is_empty() {
        return Paste::Files(files);
    }
    match &board.image_extension {
        Some(extension) => Paste::Image {
            name: pasted_image_name(now, offset, extension),
        },
        None => Paste::Text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::date;

    fn now() -> DateTime<Utc> {
        date::parse("2026-09-07T16:04:05Z").expect("an instant")
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).expect("a zone")
    }

    /// Copying a file in Explorer puts the file *and* a rendition of it on the board. Taking both
    /// attaches the same picture twice.
    #[test]
    fn a_file_beats_the_picture_of_it() {
        let board = Clipboard {
            files: vec![r"C:\shots\one.png".into()],
            image_extension: Some("png".into()),
            has_text: false,
        };

        assert_eq!(
            decide(&board, now(), utc()),
            Paste::Files(vec![r"C:\shots\one.png".into()])
        );
    }

    /// A screenshot has no file behind it, so it needs a name — and one that cannot collide with
    /// the paste a minute later.
    #[test]
    fn a_screenshot_is_named_for_the_moment_it_was_pasted() {
        let board = Clipboard {
            image_extension: Some("png".into()),
            ..Default::default()
        };

        assert_eq!(
            decide(&board, now(), utc()),
            Paste::Image {
                name: "Pasted Image 2026-09-07 at 16.04.05.png".into()
            }
        );
    }

    /// The name is read by the person who pasted it, so it is in their time rather than UTC.
    #[test]
    fn the_name_is_in_the_readers_own_time() {
        let behind = FixedOffset::west_opt(7 * 3600).expect("a zone");
        assert_eq!(
            pasted_image_name(now(), behind, "png"),
            "Pasted Image 2026-09-07 at 09.04.05.png"
        );
    }

    /// A board that offers an image with no format named still gets a usable file.
    #[test]
    fn an_unnamed_format_is_a_png() {
        assert!(pasted_image_name(now(), utc(), "").ends_with(".png"));
        assert!(pasted_image_name(now(), utc(), ".jpg").ends_with(".jpg"));
    }

    /// The most common paste in the app. Breaking it to serve the rarest one would be a bad trade.
    #[test]
    fn a_clipboard_with_only_text_is_left_alone() {
        let board = Clipboard {
            has_text: true,
            ..Default::default()
        };

        assert_eq!(decide(&board, now(), utc()), Paste::Text);
        assert_eq!(decide(&Clipboard::default(), now(), utc()), Paste::Text);
    }

    /// The cap is applied here, before anything is read off disk: pasting twelve files at a cap of
    /// ten must not upload the two that can never post.
    #[test]
    fn more_files_than_the_cap_are_cut_before_anything_is_sent() {
        let board = Clipboard {
            files: (0..12).map(|n| format!(r"C:\shots\{n}.png")).collect(),
            ..Default::default()
        };

        let Paste::Files(files) = decide(&board, now(), utc()) else {
            panic!("files");
        };
        assert_eq!(files.len(), MAX_FILES);
        assert_eq!(files[0], r"C:\shots\0.png");
    }

    /// A board that names an empty path is offering nothing, not a file called "".
    #[test]
    fn an_empty_path_is_not_a_file() {
        let board = Clipboard {
            files: vec!["".into(), "   ".into()],
            image_extension: Some("png".into()),
            ..Default::default()
        };

        assert!(matches!(decide(&board, now(), utc()), Paste::Image { .. }));
    }
}
