//! Local history services shared by desktop, CLI and MCP adapters.
//! This crate never initializes a webview, starts agents or contacts a server.

pub mod brand;
pub mod db;
pub mod git;
pub mod indexing;
pub mod jsonl;
pub mod library_revision;
pub mod live;
pub mod locking;
pub mod observation;
pub mod projects;
pub mod search;
pub mod service;
pub mod sessions;
pub mod source;
pub mod sql;
pub mod text_read;
