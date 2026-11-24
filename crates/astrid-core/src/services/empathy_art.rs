//! The art an empty board column shows (ADEV-221, split from the Windows task e0692268).
//!
//! The rules — which metaphor today, which state a column is — are [`astrid_rules::empathy`],
//! locked by `contracts/fixtures/empathy-art.json`. This is the rest of what a shell needs:
//!
//! 1. [`EmpathyArtService::sync`] fetches `GET /api/v1/empathy-art` and keeps its frames on disk,
//!    **only when `revision` changes**. A frame's file name carries its content hash, so one that
//!    is already there is never fetched again, and a downloaded body that does not match the
//!    manifest's `sha256` is dropped rather than kept.
//! 2. [`EmpathyArtService::frame_path`] answers a column with a local file path, never touching
//!    the network: `None` for a custom column, for a brand that is not presenting Astrid's own
//!    mascot (a partner's board must not show it), and while the frame is not cached yet.
//!
//! The art is optional for every client. A failed sync leaves whatever was cached, and a shell
//! keeps its built-in character where this answers `None`. Never block a board on it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Context, Result, ServiceError};
use astrid_rules::board::ColumnKind;
use astrid_rules::empathy;

/// The manifest `version` this code understands. A newer one is left alone, not half-read.
pub const MANIFEST_VERSION: u32 = 1;

const MANIFEST_PATH: &str = "/api/v1/empathy-art";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub metaphor: String,
    pub state: String,
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub revision: String,
    pub metaphors: Vec<String>,
    pub states: Vec<String>,
    pub frames: Vec<Frame>,
}

/// What a sync came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutcome {
    /// The server's `revision` is the one already cached.
    Unchanged,
    /// A new manifest is cached; this many frames were fetched for it.
    Updated { fetched: usize },
    /// The server speaks a manifest version this code does not; nothing changed.
    Unsupported { version: u32 },
}

pub struct EmpathyArtService {
    context: Context,
    /// `<cache_dir>/empathy-art/`, beside everything else this installation stores.
    dir: PathBuf,
}

impl EmpathyArtService {
    pub fn new(context: Context, cache_dir: impl AsRef<Path>) -> Self {
        EmpathyArtService {
            context,
            dir: cache_dir.as_ref().join("empathy-art"),
        }
    }

    fn manifest_path(&self) -> PathBuf {
        self.dir.join("manifest.json")
    }

    fn frame_file(&self, frame: &Frame) -> PathBuf {
        let short: String = frame.sha256.chars().take(12).collect();
        self.dir
            .join(format!("{}-{}-{short}.webp", frame.metaphor, frame.state))
    }

    /// The manifest last fully cached, or `None` before the first sync.
    pub fn cached_manifest(&self) -> Option<Manifest> {
        let text = std::fs::read_to_string(self.manifest_path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Refetch the manifest and, if its revision moved, the frames not already on disk.
    ///
    /// The manifest is written last, so an interrupted sync leaves the previous revision in place
    /// and the next one finishes the job.
    pub async fn sync(&self) -> Result<SyncOutcome> {
        let cached = self.cached_manifest();
        let client = &self.context.client;
        let etag = cached
            .as_ref()
            .map(|manifest| format!("\"{}\"", manifest.revision));
        let Some(response) = client
            .send_conditional(client.get(MANIFEST_PATH), etag.as_deref())
            .await?
        else {
            return Ok(SyncOutcome::Unchanged);
        };
        let manifest: Manifest = serde_json::from_slice(&response.body)
            .map_err(|error| crate::api::ApiError::Decode(error.to_string()))?;
        if manifest.version != MANIFEST_VERSION {
            return Ok(SyncOutcome::Unsupported {
                version: manifest.version,
            });
        }
        if cached.as_ref().map(|c| c.revision.as_str()) == Some(manifest.revision.as_str())
            && manifest.frames.iter().all(|f| self.frame_file(f).exists())
        {
            return Ok(SyncOutcome::Unchanged);
        }

        std::fs::create_dir_all(&self.dir).map_err(local)?;
        let mut fetched = 0;
        for frame in &manifest.frames {
            let path = self.frame_file(frame);
            if path.exists() {
                continue;
            }
            let body = client.fetch_public(&frame.url).await?.body;
            let digest = format!("{:x}", Sha256::digest(&body));
            if !digest.eq_ignore_ascii_case(&frame.sha256) {
                return Err(ServiceError::LocalFile(format!(
                    "{} does not match the manifest's sha256",
                    frame.url
                )));
            }
            // Beside and rename, so an interrupted download never looks cached.
            let temporary = path.with_extension("part");
            std::fs::write(&temporary, &body).map_err(local)?;
            std::fs::rename(&temporary, &path).map_err(local)?;
            fetched += 1;
        }

        let text = serde_json::to_string(&manifest)
            .map_err(|error| ServiceError::LocalFile(error.to_string()))?;
        let temporary = self.manifest_path().with_extension("part");
        std::fs::write(&temporary, text).map_err(local)?;
        std::fs::rename(&temporary, self.manifest_path()).map_err(local)?;
        self.prune(&manifest);
        Ok(SyncOutcome::Updated { fetched })
    }

    /// Drop frames the current manifest no longer names. Best effort: a file that will not go is
    /// only disk, never a wrong answer, because [`Self::frame_path`] reads the manifest.
    fn prune(&self, manifest: &Manifest) {
        let keep: Vec<PathBuf> = manifest.frames.iter().map(|f| self.frame_file(f)).collect();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "webp") && !keep.contains(&path) {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// The local frame an empty column shows today, or `None`.
    ///
    /// `presents_astrid_mascot` is the brand gate (web's `brandIcon === ASTRID_ICON`): a shell
    /// that is not presenting Astrid's own mascot passes `false` and gets `None`.
    pub fn frame_path(
        &self,
        presents_astrid_mascot: bool,
        list_id: &str,
        column_id: &str,
        kind: ColumnKind,
        now_epoch_ms: i64,
    ) -> Option<PathBuf> {
        if !presents_astrid_mascot || list_id.is_empty() {
            return None;
        }
        let state = empathy::state_for_board_column(column_id, kind)?;
        let manifest = self.cached_manifest()?;
        if manifest.version != MANIFEST_VERSION || manifest.metaphors.is_empty() {
            return None;
        }
        let metaphor = &manifest.metaphors
            [empathy::metaphor_index(manifest.metaphors.len(), list_id, now_epoch_ms)];
        let frame = manifest
            .frames
            .iter()
            .find(|f| &f.metaphor == metaphor && f.state == state)?;
        let path = self.frame_file(frame);
        path.exists().then_some(path)
    }
}

fn local(error: std::io::Error) -> ServiceError {
    ServiceError::LocalFile(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::api::transport::{HttpResponse, StubTransport};
    use crate::api::ApiClient;
    use crate::platform::{FixedClock, MemorySecureStore};

    const LIST: &str = "a623f322-4c3c-49b5-8a94-d2d9f00c82ba";
    /// 2026-11-20T00:00:00Z.
    const NOV_20: i64 = 20_777 * 86_400_000;

    /// The list's metaphor that day, from the rule the contract fixture locks.
    fn today() -> &'static str {
        empathy::metaphor_for(LIST, NOV_20)
    }

    fn a_cache_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("astrid-test-{}", crate::outbox::new_temp_id()));
        std::fs::create_dir_all(&dir).expect("creates");
        dir
    }

    fn bytes_for(metaphor: &str, state: &str) -> Vec<u8> {
        format!("webp {metaphor} {state}").into_bytes()
    }

    fn manifest(revision: &str) -> Manifest {
        let metaphors: Vec<String> = empathy::METAPHORS.iter().map(|m| m.to_string()).collect();
        let states: Vec<String> = empathy::STATES.iter().map(|s| s.to_string()).collect();
        let mut frames = Vec::new();
        for metaphor in &metaphors {
            for state in &states {
                let body = bytes_for(metaphor, state);
                frames.push(Frame {
                    metaphor: metaphor.clone(),
                    state: state.clone(),
                    url: format!("https://astrid.cc/images/empathy-art/{metaphor}-{state}.webp"),
                    sha256: format!("{:x}", Sha256::digest(&body)),
                    bytes: body.len() as u64,
                });
            }
        }
        Manifest {
            version: MANIFEST_VERSION,
            revision: revision.to_string(),
            metaphors,
            states,
            frames,
        }
    }

    fn stub(manifest: &Manifest) -> StubTransport {
        let mut stub = StubTransport::new().push_json(
            MANIFEST_PATH,
            200,
            serde_json::to_value(manifest).expect("serialises"),
        );
        for frame in &manifest.frames {
            stub = stub.push(
                &frame.url,
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: bytes_for(&frame.metaphor, &frame.state),
                }),
            );
        }
        stub
    }

    fn service(cache: &Path, transport: StubTransport) -> (EmpathyArtService, Arc<StubTransport>) {
        let transport = Arc::new(transport);
        let context = Context::new(
            Arc::new(ApiClient::new(
                "https://astrid.cc",
                transport.clone(),
                Arc::new(MemorySecureStore::new()),
            )),
            Arc::new(crate::store::Store::in_memory().expect("opens")),
            Arc::new(FixedClock::parsed("2026-11-20T12:00:00Z")),
        );
        (EmpathyArtService::new(context, cache), transport)
    }

    #[tokio::test]
    async fn a_column_gets_the_cached_frame_for_the_lists_metaphor_today() {
        let cache = a_cache_dir();
        let (service, _) = service(&cache, stub(&manifest("r1")));
        assert_eq!(
            service.frame_path(true, LIST, "ready", ColumnKind::Status, NOV_20),
            None,
            "nothing is cached before the first sync"
        );
        assert_eq!(
            service.sync().await.expect("syncs"),
            SyncOutcome::Updated { fetched: 25 }
        );

        let path = service
            .frame_path(true, LIST, "ready", ColumnKind::Status, NOV_20)
            .expect("cached");
        assert_eq!(
            std::fs::read(path).expect("reads"),
            bytes_for(today(), "ready")
        );
        let done = service
            .frame_path(true, LIST, "__virtual_done__", ColumnKind::Done, NOV_20)
            .expect("cached");
        assert_eq!(
            std::fs::read(done).expect("reads"),
            bytes_for(today(), "done")
        );
        std::fs::remove_dir_all(&cache).expect("removes");
    }

    #[tokio::test]
    async fn a_custom_column_and_a_partner_brand_get_no_frame() {
        let cache = a_cache_dir();
        let (service, _) = service(&cache, stub(&manifest("r1")));
        service.sync().await.expect("syncs");
        assert_eq!(
            service.frame_path(true, LIST, "review", ColumnKind::Status, NOV_20),
            None
        );
        assert_eq!(
            service.frame_path(false, LIST, "ready", ColumnKind::Status, NOV_20),
            None
        );
        assert_eq!(
            service.frame_path(true, "", "ready", ColumnKind::Status, NOV_20),
            None
        );
        std::fs::remove_dir_all(&cache).expect("removes");
    }

    #[tokio::test]
    async fn an_unchanged_revision_fetches_nothing() {
        let cache = a_cache_dir();
        let first = manifest("r1");
        let (service, _) = service(&cache, stub(&first));
        service.sync().await.expect("syncs");

        let (again, transport) = {
            let transport = StubTransport::new().push(
                MANIFEST_PATH,
                Ok(HttpResponse {
                    status: 304,
                    headers: vec![],
                    body: vec![],
                }),
            );
            self::service(&cache, transport)
        };
        assert_eq!(again.sync().await.expect("syncs"), SyncOutcome::Unchanged);
        let requests = transport.requests();
        assert_eq!(requests.len(), 1, "only the manifest was asked for");
        assert_eq!(requests[0].header("if-none-match"), Some("\"r1\""));
        std::fs::remove_dir_all(&cache).expect("removes");
    }

    #[tokio::test]
    async fn a_new_revision_refetches_and_forgets_the_old_frames() {
        let cache = a_cache_dir();
        let (service, _) = service(&cache, stub(&manifest("r1")));
        service.sync().await.expect("syncs");

        // The next revision re-draws one frame: only that one is fetched.
        let mut next = manifest("r2");
        let changed = bytes_for(today(), "ready-v2");
        let frame = next
            .frames
            .iter_mut()
            .find(|f| f.metaphor == today() && f.state == "ready")
            .expect("present");
        frame.sha256 = format!("{:x}", Sha256::digest(&changed));
        let url = frame.url.clone();
        let transport = StubTransport::new()
            .push_json(MANIFEST_PATH, 200, serde_json::to_value(&next).expect("ok"))
            .push(
                &url,
                Ok(HttpResponse {
                    status: 200,
                    headers: vec![],
                    body: changed.clone(),
                }),
            );
        let (service, transport) = self::service(&cache, transport);
        assert_eq!(
            service.sync().await.expect("syncs"),
            SyncOutcome::Updated { fetched: 1 }
        );
        assert_eq!(transport.requests().len(), 2);
        let path = service
            .frame_path(true, LIST, "ready", ColumnKind::Status, NOV_20)
            .expect("cached");
        assert_eq!(std::fs::read(path).expect("reads"), changed);
        let files = std::fs::read_dir(cache.join("empathy-art"))
            .expect("lists")
            .count();
        assert_eq!(
            files, 26,
            "25 frames and the manifest — the old frame is gone"
        );
        std::fs::remove_dir_all(&cache).expect("removes");
    }

    #[tokio::test]
    async fn a_body_that_does_not_match_its_hash_is_never_kept() {
        let cache = a_cache_dir();
        let good = manifest("r1");
        let mut bad = good.clone();
        bad.frames[0].sha256 = format!("{:x}", Sha256::digest(b"something else"));
        let (service, _) = service(&cache, stub(&bad));
        assert!(service.sync().await.is_err());
        assert_eq!(
            service.cached_manifest(),
            None,
            "the manifest is written last"
        );
        assert!(!service.frame_file(&bad.frames[0]).exists());
        std::fs::remove_dir_all(&cache).expect("removes");
    }

    #[tokio::test]
    async fn a_frame_url_that_is_not_https_is_refused() {
        let cache = a_cache_dir();
        let mut insecure = manifest("r1");
        insecure.frames[0].url = "http://evil.example/x.webp".into();
        let (service, _) = service(&cache, stub(&insecure));
        assert!(service.sync().await.is_err());
        std::fs::remove_dir_all(&cache).expect("removes");
    }
}
