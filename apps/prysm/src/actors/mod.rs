//! Capture → processing → rendering through typed Actix recipients.
//! Data messages are best effort; each full mailbox drops incoming data.
//! Composition owns the thread, terminal results, and ordered shutdown.

pub mod capturer;
#[cfg(feature = "desktop")]
pub mod egui;
pub mod pipeline;
pub mod processor;
pub mod runtime;
pub mod wled;

/// Stop an actor after messages already queued in its mailbox.
#[derive(actix::Message)]
#[rtype(result = "()")]
pub struct Stop;

/// Keep source pixels, detected crop, and computed colors paired at the renderer.
/// Frames are best effort; rendering failures are reported through completion.
#[derive(actix::Message)]
#[rtype(result = "()")]
pub struct RenderFrame {
    pub frame: prysm_capture::Frame,
    pub viewport: Option<prysm_core::Viewport>,
    pub colors: prysm_core::EdgeColors,
}
