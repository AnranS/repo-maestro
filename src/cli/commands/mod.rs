//! Per-subcommand handler bodies, split out of the (god-sized) `cli/mod.rs`
//! one heavy command at a time. The CLI's `enum Cmd` and every `*Args`
//! still live in `cli/mod.rs` so clap's derive macros keep working from
//! one place; only the *implementations* moved here.

pub mod bench;
pub mod builtins;
pub mod channels;
pub mod chat;
pub mod chat_tui;
pub mod compare;
pub mod deliberate;
pub mod demo;
pub mod discuss;
pub mod doc;
pub mod doctor;
pub mod learn;
pub mod mailbox;
pub mod memory;
pub mod models;
pub mod plan;
pub mod pr;
pub mod providers;
pub mod review;
pub mod role;
pub mod run;
pub mod runs;
pub mod services;
pub mod skills;
pub mod work;
