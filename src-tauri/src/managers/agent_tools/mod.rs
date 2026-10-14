// M2 stub: bodies arrive in M2-1..M2-3, this allow is removed in M2-3
#![allow(dead_code)]

//! Read-only project tools for the voice agent (DESIGN Rev 4, D4 / R2-1..R2-3).
//! Submodules are declared here; the dispatcher lives in `tools`.

pub mod bounded_cmd;
pub mod confined_read;
pub mod git_vet;
pub mod project;
pub mod tools;
pub mod ui_port;
