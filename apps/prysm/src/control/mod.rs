//! Configuration and lighting operations exposed through the REST API.
mod backend;
mod controller;
pub(crate) mod http;
mod persistence;
mod switching;

pub(crate) use backend::LightingService;
pub(crate) use controller::Controller;

#[cfg(test)]
mod tests;
