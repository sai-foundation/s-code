//! Shared skill shop: the domain rules every backend applies (sanitized
//! publication, receipts, the deterministic verification gate) and the
//! registry boundary with its remote HTTP client. A local experience and a
//! shared skill are different artifacts; this crate only ever sees the
//! bounded, sanitized skill.
pub mod domain;
pub mod registry;

pub use domain::*;
pub use registry::{
    DeprecateRequest, EnvironmentTokenSource, MAX_REGISTRY_RESPONSE_BYTES, Published,
    REGISTRY_REQUEST_TIMEOUT, ReceiptAccepted, RegistryError, RegistryTokenSource,
    RemoteSkillRegistryClient, SkillQuery, SkillRegistry, StaticTokenSource, validate_registry_url,
};
