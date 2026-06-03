pub mod drain;
pub mod feishu;
pub mod loader;
pub mod origin;
pub mod outbound;
pub mod poll;
pub mod route;
pub mod subscribe;
pub mod transport;
pub mod trust;

pub use drain::{drain_once, DrainError, DrainOutcome, DrainState, DRAIN_CURSOR_FILE};
pub use feishu::{parse_inbound, FeishuAttachment, FeishuInbound, FeishuParseError};
pub use loader::load_channels_config;
pub use origin::{persist, resolve, thread_for_run, OriginError, ThreadOrigin, ENVELOPES_FILE};
pub use outbound::{format_event, FormatOptions, OutboundReply};
pub use poll::{
    poll_inbound, InboundDecisionRecord, PollError, PollOutcome, PollState, INBOUND_CURSOR_FILE,
    INBOUND_DECISIONS_FILE,
};
pub use route::{
    has_confirm_token, parse_action, route_inbound, RejectReason, RouteDecision, RouteError,
};
pub use subscribe::{handle_event, SubscribeError, OUTBOUND_REPLIES_FILE};
pub use transport::{
    select_transport, BotmuxTransport, FileMockTransport, InboundMessage, Transport, TransportError,
};
pub use trust::resolve_trust;
