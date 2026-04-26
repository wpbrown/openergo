//! Future spawning helpers.
//!
//! Openergo builds with `panic = "abort"` and never cancels tasks, which makes
//! tokio's `JoinError` impossible to observe in practice. [`oe_spawn`] returns
//! an [`JoinHandle`] that unwraps the underlying join result so callers get the
//! task's output directly.
use futures::FutureExt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Spawn `future` on the current tokio runtime, returning a [`JoinHandle`]
/// that yields the future's output directly.
pub fn oe_spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
{
    JoinHandle {
        inner: tokio::task::spawn_local(future),
    }
}

/// Wrapper around [`tokio::task::JoinHandle`] that asserts the task neither
/// panics nor is cancelled.
pub struct JoinHandle<T> {
    inner: tokio::task::JoinHandle<T>,
}

impl<T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner
            .poll_unpin(cx)
            .map(|res| res.expect("OE future can not panic or be canceled"))
    }
}
