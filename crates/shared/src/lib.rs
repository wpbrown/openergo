pub mod codec;
pub mod label;
pub mod model;
pub mod protocol;
pub mod spawn;
pub mod time;
pub mod tracing_fmt;

pub mod socket {
    use std::path::PathBuf;

    /// Default path to the system-wide server IPC socket. Used by the
    /// server itself when binding and by clients when connecting.
    pub const DEFAULT_SERVER_SOCKET_PATH: &str = "/run/openergo.sock";

    /// Default path to the per-user client IPC socket.
    ///
    /// Prefers `$XDG_RUNTIME_DIR/openergo-client.sock` and falls back to
    /// `$HOME/.openergo-client.sock` when `XDG_RUNTIME_DIR` is unset.
    /// As a last resort returns `./openergo-client.sock` so the value is
    /// always a usable `PathBuf` (suitable for `clap`'s `default_value_t`).
    pub fn default_client_socket_path() -> PathBuf {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
            let mut path = PathBuf::from(dir);
            path.push("openergo-client.sock");
            return path;
        }
        if let Some(home) = std::env::var_os("HOME") {
            let mut path = PathBuf::from(home);
            path.push(".openergo-client.sock");
            return path;
        }
        PathBuf::from("openergo-client.sock")
    }
}

pub mod select_small {
    use core::pin::Pin;
    use core::task::{Context, Poll};
    use futures::FutureExt;
    use futures::future::Future;
    use smallvec::SmallVec;

    /// Future that resolves once any of its inner futures is ready,
    /// returning `(output, index)`. Inner futures are dropped on
    /// completion; unlike [`futures::future::select_all`], the remaining
    /// futures are not returned. Suited for one-shot waits where the
    /// caller rebuilds the set on the next iteration.
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    pub struct SelectSmallOnce<Fut, const N: usize> {
        inner: SmallVec<[Fut; N]>,
    }

    impl<Fut: Unpin, const N: usize> Unpin for SelectSmallOnce<Fut, N> {}

    /// Wait for any future in `iter` to complete. Stores futures inline
    /// when `iter` yields at most `N` items, spilling to the heap
    /// otherwise.
    ///
    /// # Panics
    ///
    /// Panics if `iter` is empty (matching `futures::future::select_all`).
    pub fn select_small_once<I, const N: usize>(iter: I) -> SelectSmallOnce<I::Item, N>
    where
        I: IntoIterator,
        I::Item: Future + Unpin,
    {
        let inner: SmallVec<[I::Item; N]> = iter.into_iter().collect();
        assert!(!inner.is_empty());
        SelectSmallOnce { inner }
    }

    impl<Fut: Future + Unpin, const N: usize> Future for SelectSmallOnce<Fut, N> {
        type Output = (Fut::Output, usize);

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let item =
                self.inner
                    .iter_mut()
                    .enumerate()
                    .find_map(|(i, f)| match f.poll_unpin(cx) {
                        Poll::Pending => None,
                        Poll::Ready(e) => Some((i, e)),
                    });
            match item {
                Some((idx, res)) => Poll::Ready((res, idx)),
                None => Poll::Pending,
            }
        }
    }
}

pub mod shutdown {
    use crate::oe_spawn;
    use bachelor::signal::mpmc_latched::{
        self, MpmcLatchedSignalConsumer, MpmcLatchedSignalProducer, MpmcLatchedSignalSource, Wait,
    };
    use rootcause::prelude::*;
    use std::pin::pin;
    use tokio::signal::unix::{SignalKind, signal};
    use tracing::info;

    /// A clonable handle for triggering shutdown and subscribing new
    /// shutdown signals. All clones share the same underlying signal,
    /// so notifying any of them wakes every outstanding [`ShutdownSignal`].
    pub struct ShutdownSource {
        producer: MpmcLatchedSignalProducer,
        source: MpmcLatchedSignalSource,
    }

    impl ShutdownSource {
        /// Create a new shutdown source and spawn a task that triggers
        /// shutdown when the process receives `SIGINT` or `SIGTERM`.
        ///
        /// Signal handlers are installed synchronously before the task
        /// is spawned so signals delivered immediately after this call
        /// are observed.
        pub fn new() -> Result<Self, Report> {
            let (producer, source) = mpmc_latched::signal();
            let this = Self { producer, source };

            let mut sigint =
                signal(SignalKind::interrupt()).context("Failed to install SIGINT handler")?;
            let mut sigterm =
                signal(SignalKind::terminate()).context("Failed to install SIGTERM handler")?;
            let producer = this.producer.clone();
            oe_spawn!("shutdown-signal", async move {
                match futures::future::select(pin!(sigint.recv()), pin!(sigterm.recv())).await {
                    futures::future::Either::Left(_) => {
                        info!("SIGINT received, broadcasting shutdown")
                    }
                    futures::future::Either::Right(_) => {
                        info!("SIGTERM received, broadcasting shutdown")
                    }
                }
                producer.notify();
            });

            Ok(this)
        }

        /// Create a shutdown source that is triggered manually by tests
        /// without installing OS signal handlers or spawning a watcher task.
        pub fn new_manual() -> Self {
            let (producer, source) = mpmc_latched::signal();
            Self { producer, source }
        }

        /// Trigger shutdown, waking every outstanding [`ShutdownSignal`].
        pub fn trigger(&self) {
            self.producer.notify();
        }

        /// Subscribe a new shutdown signal that callers can await.
        pub fn signal(&self) -> ShutdownSignal {
            ShutdownSignal {
                consumer: self.source.subscribe(),
            }
        }
    }

    /// A subscriber to a [`ShutdownSource`]. Await [`Self::wait`] to
    /// be woken when shutdown is requested.
    pub struct ShutdownSignal {
        consumer: MpmcLatchedSignalConsumer,
    }

    impl ShutdownSignal {
        pub fn wait(&mut self) -> Wait<'_, '_> {
            self.consumer.observe()
        }
    }
}
