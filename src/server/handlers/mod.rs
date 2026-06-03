//! Per-domain handler bodies for the axum router in `server::serve`.
//! Each submodule exposes its handlers as `pub` items; the router in
//! `server::mod` references them by their full path.

pub mod chat;
pub mod fs;
pub mod misc;
pub mod projects;
pub mod runs;
pub mod skills_memory;
