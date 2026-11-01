//! Configuration and lighting operations exposed through the REST API.
mod backend;
mod controller;
mod http;
mod persistence;
mod switching;

pub(crate) use backend::LightingService;
pub(crate) use controller::{Controller, Shutdown};
pub(crate) use http::router;

#[cfg(test)]
mod tests;
