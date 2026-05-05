//! Per-route page components extracted from `app.rs`.
//!
//! Each submodule owns a single page (or a small cluster of tightly coupled
//! components). Shared rendering helpers and storage utilities currently live
//! in `app.rs` and are imported here via `crate::app::*`.

pub(crate) mod admin;
pub(crate) mod cards;
pub(crate) mod docs;
pub(crate) mod download;
pub(crate) mod flock;
pub(crate) mod home;
pub(crate) mod index_page;
pub(crate) mod not_found;
pub(crate) mod repo;
pub(crate) mod repos_list;
pub(crate) mod skill;
pub(crate) mod skills_list;
pub(crate) mod user;
pub(crate) mod widgets;
