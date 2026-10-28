// ─── Handlers ──────────────────────────────────────────────────────────────

pub(crate) mod backups;
pub(crate) mod config_auth;
pub(crate) mod glossary_memory;
pub(crate) mod inject;
pub(crate) mod io;
pub(crate) mod jobs;
pub(crate) mod patch;
pub(crate) mod project;
pub(crate) mod speakers;
pub(crate) mod strings;
pub(crate) mod text_fit;
pub(crate) mod translate;

#[cfg(test)]
pub(crate) mod test_support;
