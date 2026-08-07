//! Automatic patch application engine.
//!
//! Shared by CLI, Axum server, and external patchers. Verification is always
//! read-only and runs before any write. Apply is journaled (not atomic) —
//! recovery is always rollback-to-pristine via `.locust/backup/`.
//!
//! Authoritative rules (design rev 4):
//! - **R1**: rollback restore is driven only by `backup/manifest.json`; the
//!   receipt only nominates deletion candidates, and the backup manifest has
//!   final veto on any path present there.
//! - **R2**: a manifest-less `backup/` may be discarded only when receipt AND
//!   journal are absent AND verify reports Clean at the **strict** tier.
//! - **R3**: forced same-id+version reapply is in-place with R1 carry-forward;
//!   any version/id/file-set change is rollback-then-fresh.

pub mod apply;
pub mod manifest;
pub mod pack;
pub mod rollback;
pub mod store;
pub mod verify;
pub mod zipsec;

pub use apply::{apply, ApplyOptions, ApplyReport, PatchProgress};
pub use manifest::{BackupBaseline, BackupManifest, PatchFileEntry, PatchManifest, Receipt};
pub use pack::{pack_injection_recording, PackOptions, PackReport};
pub use rollback::{rollback, RollbackOptions, RollbackReport};
pub use store::{PatchStatus, PatchStore};
pub use verify::{verify, FileMismatch, VerificationOutcome, VerificationReport};
