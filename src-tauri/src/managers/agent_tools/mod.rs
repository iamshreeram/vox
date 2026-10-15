//! Read-only project tools for the voice agent (DESIGN Rev 4, D4 / R2-1..R2-3).
//! Submodules are declared here; the dispatcher lives in `tools`.

pub mod bounded_cmd;
pub mod confined_read;
pub mod git_vet;
pub mod job_status;
pub mod project;
pub mod tauri_backends;
pub mod tools;
pub mod ui_port;
