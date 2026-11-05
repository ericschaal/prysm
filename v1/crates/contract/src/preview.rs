//! A subscription owns preview demand and must release it when dropped.
use crate::{
    ControlError,
    identity::{FrameStamp, SettingsRevision},
};
use futures::{Stream, stream::BoxStream};
use prysm_core::ProcessedSnapshot;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Debug, Clone)]
pub struct PreviewFrame {
    pub stamp: FrameStamp,
    pub revision: SettingsRevision,
    pub snapshot: ProcessedSnapshot,
}

/// The backing stream owns the demand guard; dropping the lease drops that guard.
pub struct PreviewLease(BoxStream<'static, Result<PreviewFrame, ControlError>>);

impl PreviewLease {
    pub fn new(stream: BoxStream<'static, Result<PreviewFrame, ControlError>>) -> Self {
        Self(stream)
    }
}

impl Stream for PreviewLease {
    type Item = Result<PreviewFrame, ControlError>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.as_mut().poll_next(cx)
    }
}

pub trait PreviewSource: Clone + Send + Sync + 'static {
    /// Never starts capture or changes lighting state.
    fn subscribe(&self) -> impl Future<Output = Result<PreviewLease, ControlError>> + Send;
}
