//! Wire representations and schema roots. Domain ports never depend on this crate.
pub mod application;
pub mod client;
#[cfg(feature = "schema")]
pub mod schema;
pub mod wire;
