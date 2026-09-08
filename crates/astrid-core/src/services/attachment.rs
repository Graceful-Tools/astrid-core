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
//! ## Uploading needs a connection, and says so
//!
//! Everything else in this app writes through the Outbox and works on a train. An upload cannot:
//! the journal holds JSON, and a queue entry carrying a photo would put megabytes into the write
//! journal and still not be an attachment anybody else could see. Apple keeps a parallel disk
//! queue for this; that is a bigger machine than it looks, and until it exists here an upload
//! offline fails honestly rather than appearing to have worked.
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

    /// Upload a file from disk and answer with the id the server gave it.
    ///
    /// `context` is the JSON the server wants beside the file — `{"listId": "…"}` — which is how it
    /// decides who may read it afterwards.
    pub async fn upload(
        &self,
        path: &Path,
        context: serde_json::Value,
    ) -> Result<crate::model::SecureFile> {
        let bytes =
            std::fs::read(path).map_err(|error| ServiceError::LocalFile(error.to_string()))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("attachment")
            .to_string();
        let mime = mime_for(path);

        let boundary = format!("astrid-{}", crate::outbox::new_temp_id());
        let body = multipart(&boundary, &name, &mime, &bytes, &context.to_string());

        let request = self
            .context
            .client
            .post(endpoints::REQUEST_UPLOAD)
            .bytes(format!("multipart/form-data; boundary={boundary}"), body);
        let answer = self.context.client.send(request).await?;

        let id = answer
            .get("fileId")
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                ServiceError::Api(crate::api::ApiError::Decode(
                    "the upload answered without a file id".into(),
                ))
            })?;
        Ok(SecureFile {
            id: id.to_string(),
            name,
            size: bytes.len() as i64,
            mime_type: mime,
        })
    }
}

/// Where a downloaded file is kept.
///
/// The id names it, and the original name contributes only its extension, so the program that
/// opens it recognises the type. An id is safe as a path component; a name from the server is not —
/// "../../etc/passwd" is a perfectly ordinary string to put in a filename field — and is never used
/// as one.
fn cached_path(cache_dir: &Path, file: &SecureFile) -> PathBuf {
    let extension = Path::new(&file.name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("bin");
    cache_dir.join(format!("{}.{}", file.id, extension))
}

/// The body of a multipart upload: the file, then the context object.
///
/// Written out rather than pulled from a crate because it is twenty lines and the alternative is a
/// dependency in the core that exists to build a string.
fn multipart(boundary: &str, file_name: &str, mime: &str, bytes: &[u8], context: &str) -> Vec<u8> {
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
fn mime_for(path: &Path) -> String {
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
