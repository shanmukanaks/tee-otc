//! Market Maker Protocol for TEE-OTC
//!
//! This crate defines the protocol messages exchanged between the OTC server
//! and market makers. It contains no networking code - implementations are
//! responsible for their own transport layer.

pub mod errors;
pub mod messages;
pub mod version;

pub use errors::*;
pub use messages::*;
pub use version::*;

// Re-export commonly used types from the otc-models crate
pub use otc_models::{ChainType, TxStatus};
