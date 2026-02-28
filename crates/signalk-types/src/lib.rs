/// SignalK protocol version implemented by this library
pub const SIGNALK_VERSION: &str = "1.7.0";

pub mod ais;
pub mod delta;
pub mod full;
pub mod geo;
pub mod meta;
pub mod notification;
pub mod path;
pub mod resources;
pub mod source;
pub mod v2;
pub mod ws;

// Re-exports for convenience
pub use ais::{TargetClass, classify_mmsi};
pub use delta::{Delta, PathValue, PutRequest, PutResponse, PutSpec, PutState, Update};
pub use full::{
    DiscoveryResponse, EndpointInfo, FullModel, LoginRequest, LoginResponse, ServerInfo,
    SignalKValue, SourceValue, VesselData,
};
pub use meta::{Metadata, Zone, ZoneState};
pub use notification::{Notification, NotificationMethod, NotificationState};
pub use path::{matches_pattern, normalize_context, resolve_self, split};
pub use resources::{ActiveRoute, CoursePoint, CourseState, PointType, Position, ResourceType};
pub use source::{Source, SourceRef};
pub use v2::{
    ActiveRouteRequest, DestinationRequest, FeatureInfo, FeaturesResponse, PointAdvanceRequest,
    PointIndexRequest, ResourceQueryParams, ResourceResponse,
};
pub use ws::{
    HelloMessage, InboundMessage, SubscribeMessage, SubscribeMode, Subscription,
    SubscriptionPolicy, UnsubscribeMessage, UnsubscribeSpec,
};
