use bachelor::error::Closed;
use bachelor::watch::{MpmcWatchRefConsumer, MpmcWatchRefProducer, MpmcWatchRefSource, mpmc_watch};
use shared::model::CreditLimit;
use std::future::Future;

/// Per-tracker credit limits. Each field is the budget for one of the
/// per-source credit accumulators (rest window, break window, daily window).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CreditLimitState {
    pub rest: CreditLimit,
    pub breaks: CreditLimit,
    pub day: CreditLimit,
}

/// Source-side handle to the credit-limit watch. Cloneable so multiple
/// consumers (and a future tuning driver) can subscribe.
#[derive(Clone)]
pub struct CreditLimitSource {
    inner: MpmcWatchRefSource<CreditLimitState>,
}

impl CreditLimitSource {
    pub fn subscribe_forward(&self) -> CreditLimitConsumer {
        CreditLimitConsumer {
            inner: self.inner.subscribe_forward(),
        }
    }
}

/// Producer-side handle. Reserved for the future pain-driven tuner; today
/// the only writer is `create` (the initial value).
#[derive(Clone)]
pub struct CreditLimitProducer {
    inner: MpmcWatchRefProducer<CreditLimitState>,
}

#[cfg_attr(not(test), expect(unused, reason = "pending the future tuner"))]
impl CreditLimitProducer {
    /// Replace the current limit state. Errors from a closed watch are
    /// silently dropped, mirroring the convention used by other producers
    /// in this codebase.
    pub fn update(&self, f: impl FnOnce(&mut CreditLimitState)) {
        let _ = self.inner.update(f);
    }
}

pub struct CreditLimitConsumer {
    inner: MpmcWatchRefConsumer<CreditLimitState>,
}

impl CreditLimitConsumer {
    pub fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin {
        self.inner.changed()
    }

    pub fn view<R>(&self, f: impl FnOnce(&CreditLimitState) -> R) -> R {
        self.inner.view(f)
    }
}

impl crate::watch_mux::FiniteChanges for CreditLimitConsumer {
    fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin + '_ {
        CreditLimitConsumer::changed(self)
    }
}

/// Construct the credit-limit watch with the given initial state. There is
/// no driver future today; a future automatic tuner will own the returned
/// [`CreditLimitProducer`] and update it from pain telemetry.
pub fn create(initial: CreditLimitState) -> (CreditLimitSource, CreditLimitProducer) {
    let (producer, source) = mpmc_watch(initial);
    (
        CreditLimitSource { inner: source },
        CreditLimitProducer { inner: producer },
    )
}
