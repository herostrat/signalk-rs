/// SignalK protocol version implemented by this library
pub const SIGNALK_VERSION: &str = "1.7.0";

pub mod delta;
pub mod full;
pub mod meta;
pub mod path;
pub mod source;
pub mod ws;

// Re-exports for convenience
pub use delta::{Delta, PathValue, PutRequest, PutResponse, PutSpec, PutState, Update};
pub use full::{
    DiscoveryResponse, EndpointInfo, FullModel, LoginRequest, LoginResponse, ServerInfo,
    SignalKValue, VesselData,
};
pub use meta::{Metadata, Zone, ZoneState};
pub use path::{matches_pattern, normalize_context, resolve_self, split};
pub use source::{Source, SourceRef};
pub use ws::{
    HelloMessage, InboundMessage, SubscribeMessage, SubscribeMode, Subscription,
    SubscriptionPolicy, UnsubscribeMessage, UnsubscribeSpec,
};
