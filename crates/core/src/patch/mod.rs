//! Automatic patch application engine.
//!
//! Shared by CLI, Axum server, and external patchers. Verification never writes
//! the game and runs before any asset write. Mutations and archive verification
//! hold a cross-process game lock in per-user OS data storage; receipt status
//! checks are read-only and do not acquire it. Apply is journaled —
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
mod lock;
pub mod manifest;
pub mod pack;
pub mod rollback;
pub mod store;
pub mod stream;
pub mod verify;
pub mod zipsec;

pub use apply::{apply, apply_cancellable, ApplyOptions, ApplyReport, PatchProgress};
pub use lock::GameLock;
pub use manifest::{BackupBaseline, BackupManifest, PatchFileEntry, PatchManifest, Receipt};
pub use pack::{
    ensure_pack_output_outside, pack_injection_recording, pack_recorded_generation,
    pack_with_pristine_backup, PackOptions, PackReport,
};
pub use rollback::{preview_rollback, rollback, RollbackOptions, RollbackReport};
pub use store::{PatchStatus, PatchStore};
pub use verify::{
    verify, verify_receipt_files, FileMismatch, ReceiptVerificationMode, ReceiptVerificationReport,
    VerificationOutcome, VerificationReport,
};
