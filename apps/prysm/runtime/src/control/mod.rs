//! Configuration and lighting operations exposed through the REST API.
mod backend;
mod calibration;
mod controller;
pub(crate) mod http;
mod observation;
mod ready;
mod switching;

pub(crate) use controller::{Controller, ControllerHandle};

#[cfg(all(test, feature = "host-input"))]
mod tests;
