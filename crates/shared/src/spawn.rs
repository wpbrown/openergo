use futures::FutureExt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tracing::Instrument;

/// Spawn a local task instrumented with `span`. Used by [`oe_spawn!`] when
/// the `tokio-console` feature is disabled. Not intended to be called
/// directly.
#[doc(hidden)]
#[track_caller]
pub fn __oe_spawn<F>(name: &'static str, span: tracing::Span, future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
{
    JoinHandle {
        name,
        inner: tokio::task::spawn_local(future.instrument(span)),
    }
}

/// Spawn a named local task via `tokio::task::Builder`, instrumented with
/// `span`. Used by [`oe_spawn!`] when the `tokio-console` feature is
/// enabled. Not intended to be called directly.
#[cfg(feature = "tokio-console")]
#[doc(hidden)]
#[track_caller]
pub fn __oe_spawn_named<N, F>(
    task_name: N,
    name: &'static str,
    span: tracing::Span,
    future: F,
) -> JoinHandle<F::Output>
where
    N: AsRef<str>,
    F: Future + 'static,
{
    JoinHandle {
        name,
        inner: tokio::task::Builder::new()
            .name(task_name.as_ref())
            .spawn_local(future.instrument(span))
            .expect("oe_spawn requires a LocalSet/LocalRuntime context"),
    }
}

/// Spawn `future` on the current tokio runtime, instrumenting it with a
/// tracing span and (when `tokio-console` is enabled) a tokio task name.
///
/// Openergo builds with `panic = "abort"` and never cancels tasks, so
/// [`tokio::task::JoinError`] is impossible in practice. The returned
/// [`JoinHandle`] unwraps the join result so callers get the task's output
/// directly.
///
/// Every spawned future is wrapped in a [`tracing::Span`] so log records and
/// events emitted while it runs carry consistent context. Each call site also
/// supplies a tokio task name that is forwarded to [`tokio::task::Builder`]
/// when the `tokio-console` feature is enabled and discarded otherwise, so
/// a `format!(...)` task name is free in production builds.
///
/// # Forms
///
/// ```ignore
/// // Simple form: one static name is used for *both* the tracing span and
/// // the tokio task name.
/// oe_spawn!("client-ipc-listener", future);
///
/// // Extended form: a (potentially dynamic) tokio task name plus an
/// // explicit span. Use this when you want a static span name with
/// // structured fields and a dynamic task name. The `task:` expression is
/// // only evaluated when `tokio-console` is enabled.
/// oe_spawn!(
///     task: format!("device:{label_str}"),
///     span: tracing::info_span!("device", label = %label_str),
///     future,
/// );
/// ```
#[macro_export]
macro_rules! oe_spawn {
    // Simple form: a single static name used for both the tracing span and
    // the tokio task name. `:literal` (rather than `:expr`) deliberately
    // rejects dynamic names so callers route them through the extended
    // form below.
    ($name:literal, $future:expr $(,)?) => {{
        #[cfg(feature = "tokio-console")]
        {
            $crate::spawn::__oe_spawn_named($name, $name, ::tracing::info_span!($name), $future)
        }
        #[cfg(not(feature = "tokio-console"))]
        {
            $crate::spawn::__oe_spawn($name, ::tracing::info_span!($name), $future)
        }
    }};
    // Extended form: explicit (potentially dynamic) tokio task name plus a
    // caller-built span. The `$task` expression is only evaluated when
    // `tokio-console` is enabled, so `format!(...)` is free otherwise. In
    // debug builds we still type-check it against `AsRef<str>` so typos
    // surface without the feature.
    (task: $task:expr, span: $span:expr, $future:expr $(,)?) => {{
        #[cfg(feature = "tokio-console")]
        {
            let span = $span;
            let name = span
                .metadata()
                .map(::tracing::Metadata::name)
                .unwrap_or("unknown");
            $crate::spawn::__oe_spawn_named($task, name, span, $future)
        }
        #[cfg(not(feature = "tokio-console"))]
        {
            #[cfg(debug_assertions)]
            let _ = || {
                let _: &dyn ::core::convert::AsRef<str> = &$task;
            };
            let span = $span;
            let name = span
                .metadata()
                .map(::tracing::Metadata::name)
                .unwrap_or("unknown");
            $crate::spawn::__oe_spawn(name, span, $future)
        }
    }};
}

/// Wrapper around [`tokio::task::JoinHandle`] that asserts the task neither
/// panics nor is cancelled.
pub struct JoinHandle<T> {
    name: &'static str,
    inner: tokio::task::JoinHandle<T>,
}

pub struct JoinedTask<T> {
    pub name: &'static str,
    pub result: T,
}

impl<T> JoinHandle<T> {
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub async fn join(self) -> JoinedTask<T> {
        let name = self.name;
        let result = self.await;
        JoinedTask { name, result }
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.inner
            .poll_unpin(cx)
            .map(|res| res.expect("OE future can not panic or be canceled"))
    }
}
