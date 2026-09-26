//! Files on a task.
//!
//! Ports the parts of `astrid-ios/Astrid App/Core/Services/AttachmentService.swift` that a client
//! must get right, and none of its caching machinery, which is three overlapping caches there.
//!
//! ## Attachments reach a task through comments
//!
//! There is no "attach to task" endpoint. A file is uploaded, and the id it comes back with is
//! carried by a comment. So the attachments *on* a task are the files on the task itself plus the
//! files on every comment — which is what [`crate::model::Task::all_secure_files`] already
//! gathers, and why the Mac's Attachments section was empty on nearly every task until it started
//! doing the same.
//!
//! ## An upload waits on disk, not in the journal
//!
//! Everything in this app writes through the Outbox, and an upload is no exception — but the
//! journal holds JSON, and a queue row carrying a photograph is a row nobody can read in a
//! database nobody should have. So the bytes are copied into a pending directory beside the
//! downloads and the journal row names the copy.
//!
//! The copy is what makes it honest. Somebody who attaches a file and shuts the lid expects the
//! file to arrive; the original may have been renamed, moved or deleted by then, and a queue that
//! remembered only a path would send whatever is at that path tomorrow, or nothing at all.
//!
//! The file's id is temporary until the server answers, which is what lets the comment carrying it
//! be queued in the same breath: the Outbox rewrites every temporary id in a payload once the
//! write that produces it succeeds, so the comment finds the real file id without knowing it.
//!
//! ## A download is a file on disk
//!
//! The bytes go to a cache directory beside the database and the caller is handed a path, because
//! what somebody wants to do with an attachment is open it in the program that reads that kind of
//! file. Cached by id: a file's contents cannot change without its id changing, so a second open
//! is free.

use std::path::{Path, PathBuf};

use super::{Context, Result, ServiceError};
use crate::api::endpoints;
use crate::model::SecureFile;

pub struct AttachmentService {
    context: Context,
    /// Where downloads land. Beside the cache database, so one directory holds everything this
    /// installation stores.
    cache_dir: PathBuf,
}

impl AttachmentService {
    pub fn new(context: Context, cache_dir: impl AsRef<Path>) -> Self {
        AttachmentService {
            context,
            cache_dir: cache_dir.as_ref().to_path_buf(),
        }
    }

    /// Every file reachable from a task: its own, and its comments'.
    pub fn for_task(&self, task_id: &str) -> Result<Vec<SecureFile>> {
        let mut task = self
            .context
            .store
            .task(task_id)?
            .ok_or_else(|| ServiceError::NotFound {
                kind: "task",
                id: task_id.to_string(),
            })?;
        // The comments are stored separately from the task, so they are folded in here rather than
        // being expected on the record — a task fetched from the list endpoint carries none.
        task.comments = Some(self.context.store.comments_for_task(task_id)?);
        Ok(task.all_secure_files())
    }

    /// Where a file would be if it has been downloaded.
    pub fn cached_path(&self, file: &SecureFile) -> PathBuf {
        cached_path(&self.cache_dir, file)
    }

    pub fn is_cached(&self, file: &SecureFile) -> bool {
        self.cached_path(file).exists()
    }

    /// Where this file's bytes already are on this machine, or `None`.
    ///
    /// Never touches the network. Three places count as in hand, and a thumbnail needs all of
    /// them: a file somebody has opened before is in the download cache under its real id, a file
    /// this device attached moments ago is in the pending directory under its temporary one,
    /// waiting for a connection, and one whose upload has just been delivered is in the download
    /// cache under the id the server gave it while the comment naming it still says `temp_`.
    ///
    /// The last two are the ones worth stating. Posting a screenshot and then watching it load —
    /// from the machine it was taken on, out of bytes this process wrote itself — is the bug the
    /// Mac fixed in AITD-308, and the third case is how it came back on Windows (task 48f72aa7):
    /// the copy left the pending directory the instant the upload answered, and the optimistic
    /// comment goes on naming the file by its temporary id until the thread is fetched again.
    pub fn local_path(&self, file: &SecureFile) -> Option<PathBuf> {
        let cached = self.cached_path(file);
        if cached.exists() {
            return Some(cached);
        }
        let pending = self.pending_dir().join(&file.id);
        if pending.exists() {
            return Some(pending);
        }
        // Delivered, but the row in hand predates the delivery. The mapping is kept for exactly
        // this — a temporary id somebody is still holding — so asking it costs one indexed read.
        let server_id = self.context.store.resolve_id(&file.id).ok()?;
        if server_id == file.id {
            return None;
        }
        let promoted = cached_path_for(&self.cache_dir, &server_id, &file.name);
        promoted.exists().then_some(promoted)
    }

    /// Fetch a file's bytes and keep them. Answers with the path.
    pub async fn download(&self, file: &SecureFile) -> Result<PathBuf> {
        let path = self.cached_path(file);
        if path.exists() {
            return Ok(path);
        }

        let request = self.context.client.get(endpoints::secure_file(&file.id));
        let response = self.context.client.send_raw(request).await?;

        std::fs::create_dir_all(&self.cache_dir)
            .map_err(|error| ServiceError::LocalFile(error.to_string()))?;
        // Beside and rename, so an interrupted download does not leave a truncated file that looks
        // cached and opens as nothing.
        let temporary = path.with_extension("part");
        std::fs::write(&temporary, &response.body)
            .map_err(|error| ServiceError::LocalFile(error.to_string()))?;
        std::fs::rename(&temporary, &path)
            .map_err(|error| ServiceError::LocalFile(error.to_string()))?;
        Ok(path)
    }

    /// Take a copy of a file and answer with what it will be, once it is sent.
    ///
    /// The copy is the point. Somebody who attaches a file and closes their laptop expects the
    /// file to arrive, and the original may have been renamed, moved or deleted by then — a queue
    /// that remembered only a path would upload whatever is at that path a day later, or nothing.
    ///
    /// The id is a temporary one, so the comment that carries this file can be queued in the same
    /// breath: the Outbox rewrites it to the real id the moment the upload answers.
    pub fn queue(&self, path: &Path) -> Result<(SecureFile, PathBuf)> {
        let bytes =
            std::fs::read(path).map_err(|error| ServiceError::LocalFile(error.to_string()))?;
        let name = file_name(path);
        let id = crate::outbox::new_temp_id();
        let pending = self.pending_dir();
        std::fs::create_dir_all(&pending)
            .map_err(|error| ServiceError::LocalFile(error.to_string()))?;
        let held = pending.join(&id);
        std::fs::write(&held, &bytes)
            .map_err(|error| ServiceError::LocalFile(error.to_string()))?;

        Ok((
            SecureFile {
                id,
                name,
                size: bytes.len() as i64,
                mime_type: mime_for(path),
            },
            held,
        ))
    }

    /// Where files wait for a connection. Beside the downloads, under the same cache directory.
    pub fn pending_dir(&self) -> PathBuf {
        self.cache_dir.join("pending")
    }
}

/// The name to send, or a plain one when the path has none worth sending.
fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment")
        .to_string()
}

/// Where a downloaded file is kept.
///
/// The id names it, and the original name contributes only its extension, so the program that
/// opens it recognises the type. An id is safe as a path component; a name from the server is not —
/// "../../etc/passwd" is a perfectly ordinary string to put in a filename field — and is never used
/// as one.
fn cached_path(cache_dir: &Path, file: &SecureFile) -> PathBuf {
    cached_path_for(cache_dir, &file.id, &file.name)
}

/// The same rule, for a file whose id is not the one on the record in hand — a delivered upload
/// still known to its comment by the temporary id it was queued under.
pub(crate) fn cached_path_for(cache_dir: &Path, id: &str, name: &str) -> PathBuf {
    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("bin");
    cache_dir.join(format!("{id}.{extension}"))
}

/// Keep the bytes of a delivered upload, under the id the server gave it.
///
/// The Outbox used to delete the pending copy the moment the upload answered. That made a picture
/// this device attached undrawable the instant it was delivered, and it came back only after the
/// thread had been fetched and the file downloaded again — from the machine that had just uploaded
/// it. Task 48f72aa7. Moving it into the download cache is the same cleanup of the pending
/// directory, minus the round trip.
///
/// Failing to move is not worth reporting: the bytes are a cache, and the worst outcome is the
/// download that used to happen every time.
pub(crate) fn promote(pending: &Path, cache_dir: &Path, server_id: &str, name: &str) -> bool {
    if std::fs::create_dir_all(cache_dir).is_err() {
        return false;
    }
    let destination = cached_path_for(cache_dir, server_id, name);
    // Rename first — it is atomic and free on the same volume, which the pending directory is by
    // construction. Copy-then-delete is the fallback for the case where it somehow is not.
    if std::fs::rename(pending, &destination).is_ok() {
        return true;
    }
    match std::fs::copy(pending, &destination) {
        Ok(_) => {
            let _ = std::fs::remove_file(pending);
            true
        }
        Err(_) => false,
    }
}

/// The body of a multipart upload: the file, then the context object.
///
/// Written out rather than pulled from a crate because it is twenty lines and the alternative is a
/// dependency in the core that exists to build a string.
pub(crate) fn multipart(
    boundary: &str,
    file_name: &str,
    mime: &str,
    bytes: &[u8],
    context: &str,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(bytes.len() + 512);
    let mut push = |text: &str| body.extend_from_slice(text.as_bytes());

    push(&format!("--{boundary}\r\n"));
    // The name is quoted and any quote inside it removed: a filename with a quote in it would
    // otherwise end the header early and send a body the server cannot parse.
    push(&format!(
        "Content-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\n",
        file_name.replace(['"', '\r', '\n'], "")
    ));
    push(&format!("Content-Type: {mime}\r\n\r\n"));
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");

    let mut push = |text: &str| body.extend_from_slice(text.as_bytes());
    push(&format!("--{boundary}\r\n"));
    push("Content-Disposition: form-data; name=\"context\"\r\n\r\n");
    push(context);
    push("\r\n");
    push(&format!("--{boundary}--\r\n"));
    body
}

/// A content type from the file's extension.
///
/// A short table rather than a crate: the server re-checks the type anyway, and what this affects
/// is whether a browser shows an image inline. Anything unrecognised is bytes.
pub(crate) fn mime_for(path: &Path) -> String {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_lowercase();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "zip" => "application/zip",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: &str, name: &str) -> SecureFile {
        SecureFile {
            id: id.into(),
            name: name.into(),
            size: 12,
            mime_type: "image/png".into(),
        }
    }

    /// The id names the cached file. A name from the server is never used as a path component —
    /// "../../etc/passwd" is a perfectly ordinary string to put in a filename field.
    #[test]
    fn a_cached_file_is_named_by_its_id() {
        let cache = Path::new("cache");
        let path = cached_path(cache, &file("f1", "holiday.png"));
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("f1.png"));

        let nasty = cached_path(cache, &file("f2", "../../etc/passwd"));
        assert_eq!(nasty.parent(), Some(cache));
    }

    /// A file with no extension still opens as something rather than as nothing.
    #[test]
    fn a_file_with_no_extension_gets_one() {
        let path = cached_path(Path::new("cache"), &file("f1", "notes"));
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("f1.bin"));
    }

    #[test]
    fn the_content_type_comes_from_the_extension() {
        assert_eq!(mime_for(Path::new("a/b/holiday.PNG")), "image/png");
        assert_eq!(mime_for(Path::new("notes.md")), "text/plain");
        assert_eq!(
            mime_for(Path::new("archive.tar.gz")),
            "application/octet-stream"
        );
        assert_eq!(
            mime_for(Path::new("noextension")),
            "application/octet-stream"
        );
    }

    /// A filename with a quote in it would end the header early and send a body the server cannot
    /// parse.
    #[test]
    fn a_quote_in_a_filename_cannot_break_the_headers() {
        let body = multipart("B", "ho\"li\nday.png", "image/png", b"bytes", "{}");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("filename=\"holiday.png\""));
        assert_eq!(text.matches("filename=").count(), 1);
    }

    /// A temporary directory of this test's own, so two of them cannot see each other's files.
    fn a_cache_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("astrid-test-{}", crate::outbox::new_temp_id()));
        std::fs::create_dir_all(&dir).expect("creates");
        dir
    }

    fn service(cache_dir: &Path) -> (AttachmentService, std::sync::Arc<crate::store::Store>) {
        use crate::api::{ApiClient, StubTransport};
        use crate::platform::{FixedClock, MemorySecureStore};
        use std::sync::Arc;

        let store = Arc::new(crate::store::Store::in_memory().expect("opens"));
        let context = Context::new(
            Arc::new(ApiClient::new(
                "https://astrid.cc",
                Arc::new(StubTransport::new()),
                Arc::new(MemorySecureStore::new()),
            )),
            store.clone(),
            Arc::new(FixedClock::parsed("2026-09-26T12:00:00Z")),
        );
        (AttachmentService::new(context, cache_dir), store)
    }

    /// A file waiting for a connection draws from the copy the Outbox wrote. AITD-308 on the Mac.
    #[test]
    fn a_file_still_waiting_to_be_sent_is_already_in_hand() {
        let cache = a_cache_dir();
        let (service, _store) = service(&cache);
        std::fs::create_dir_all(service.pending_dir()).expect("creates");
        std::fs::write(service.pending_dir().join("temp_abc"), b"bytes").expect("writes");

        assert_eq!(
            service.local_path(&file("temp_abc", "shot.png")),
            Some(service.pending_dir().join("temp_abc")),
        );
        std::fs::remove_dir_all(&cache).expect("removes");
    }

    /// Task 48f72aa7. The comment on screen names the file by the temporary id it was queued
    /// under, and goes on doing so until the thread is fetched again — so a delivered upload has to
    /// be findable by that id, or the picture disappears the moment it is successfully sent.
    #[test]
    fn a_delivered_file_is_in_hand_under_the_temporary_id_its_comment_still_uses() {
        let cache = a_cache_dir();
        let (service, store) = service(&cache);
        // What the Outbox leaves behind: the bytes under the server's id, and the mapping.
        std::fs::write(cache.join("file_real.png"), b"bytes").expect("writes");
        store
            .record_id_mapping(
                "temp_abc",
                "file_real",
                crate::model::date::parse("2026-09-26T12:00:00Z").expect("an instant"),
            )
            .expect("records");

        assert_eq!(
            service.local_path(&file("temp_abc", "shot.png")),
            Some(cache.join("file_real.png")),
        );
        std::fs::remove_dir_all(&cache).expect("removes");
    }

    /// Nothing anywhere is still nothing — the chip is what a screen draws until the bytes arrive,
    /// and an unmapped id must not be reported as a path that does not exist.
    #[test]
    fn a_file_nobody_has_the_bytes_for_is_not_in_hand() {
        let cache = a_cache_dir();
        let (service, _store) = service(&cache);

        assert_eq!(service.local_path(&file("f1", "shot.png")), None);
        std::fs::remove_dir_all(&cache).expect("removes");
    }

    /// Promoting is the same cleanup as deleting, so far as the pending directory is concerned.
    #[test]
    fn promoting_moves_the_bytes_rather_than_copying_them() {
        let cache = a_cache_dir();
        let pending = cache.join("pending");
        std::fs::create_dir_all(&pending).expect("creates");
        let held = pending.join("temp_abc");
        std::fs::write(&held, b"bytes").expect("writes");

        assert!(promote(&held, &cache, "file_real", "shot.png"));
        assert!(!held.exists());
        assert_eq!(
            std::fs::read(cache.join("file_real.png")).expect("reads"),
            b"bytes",
        );
        std::fs::remove_dir_all(&cache).expect("removes");
    }

    #[test]
    fn the_body_carries_the_file_and_the_context() {
        let body = multipart("B", "a.png", "image/png", b"bytes", r#"{"listId":"l1"}"#);
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("--B\r\n"));
        assert!(text.contains("name=\"file\""));
        assert!(text.contains("bytes"));
        assert!(text.contains("name=\"context\""));
        assert!(text.contains(r#"{"listId":"l1"}"#));
        assert!(text.ends_with("--B--\r\n"));
    }
}
