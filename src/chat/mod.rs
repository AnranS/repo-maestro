//! Multi-session orchestrator chat.
//!
//! Storage layout under `.maestro/chat/`:
//!   current.txt           — id of the current session
//!   sessions/<id>.json    — one file per session (messages + cursor_chat_id)
//!
//! Each message can carry parsed `maestro-action` blocks that the UI can ask the
//! user to confirm before they actually run via the CLI surface.

pub mod actions;
pub mod compact;
pub mod continuation;
pub mod control;
pub mod providers;
pub mod sessions;
pub mod signature;
pub mod stream;
pub mod tagger;
pub mod turn;

pub use actions::{execute_action, parse_actions, Action, ActionStatus, ActionVerb};
pub use compact::compact_session;
pub use continuation::{render_brief as render_continuation_brief, write_brief};
pub use sessions::{Message, Role, Session, SessionMeta};
pub use stream::StreamEvent;
pub use tagger::auto_tag;
