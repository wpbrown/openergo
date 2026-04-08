pub mod boot_instant {
    use nix::{
        sys::time::TimeSpec,
        time::{ClockId, clock_gettime},
    };
    use std::{fmt, ops::Sub, time::Duration};

    /// A monotonic instant that includes time spent in system suspend.
    ///
    /// Unlike `std::time::Instant` (which uses `CLOCK_MONOTONIC`), this uses
    /// `CLOCK_BOOTTIME` which continues to tick during suspend. This is useful
    /// for tracking "real" elapsed time including sleep periods.
    #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct BootInstant(TimeSpec);

    impl BootInstant {
        /// Returns the current boot instant.
        #[must_use]
        pub fn now() -> Self {
            Self(
                clock_gettime(ClockId::CLOCK_BOOTTIME)
                    .expect("can always get time from CLOCK_BOOTTIME"),
            )
        }

        /// Returns a zero instant (time since boot = 0).
        pub const fn zero() -> Self {
            Self(TimeSpec::from_duration(Duration::ZERO))
        }

        /// Returns the duration since an earlier instant.
        ///
        /// Returns `Duration::ZERO` if `earlier` is after `self`.
        #[must_use]
        pub fn duration_since(&self, earlier: BootInstant) -> Duration {
            self.checked_duration_since(earlier).unwrap_or_default()
        }

        /// Returns the duration since an earlier instant, or `None` if
        /// `earlier` is after `self`.
        #[must_use]
        pub fn checked_duration_since(&self, earlier: BootInstant) -> Option<Duration> {
            if self.0 >= earlier.0 {
                Some(Duration::from(self.0 - earlier.0))
            } else {
                None
            }
        }

        /// Returns the duration since an earlier instant, saturating at zero.
        #[must_use]
        pub fn saturating_duration_since(&self, earlier: BootInstant) -> Duration {
            self.checked_duration_since(earlier).unwrap_or_default()
        }

        /// Returns the inner `TimeSpec` for use with timerfd.
        pub(super) fn as_timespec(&self) -> TimeSpec {
            self.0
        }
    }

    impl fmt::Debug for BootInstant {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.fmt(f)
        }
    }

    impl Sub<BootInstant> for BootInstant {
        type Output = Duration;

        fn sub(self, other: BootInstant) -> Duration {
            self.duration_since(other)
        }
    }

    impl std::ops::Add<Duration> for BootInstant {
        type Output = BootInstant;

        fn add(self, duration: Duration) -> BootInstant {
            let new_timespec = self.0 + TimeSpec::from(duration);
            BootInstant(new_timespec)
        }
    }

    impl std::ops::Sub<Duration> for BootInstant {
        type Output = BootInstant;

        fn sub(self, duration: Duration) -> BootInstant {
            let new_timespec = self.0 - TimeSpec::from(duration);
            BootInstant(new_timespec)
        }
    }
}

pub mod timer {
    use super::boot_instant::BootInstant;
    use jiff::Timestamp;
    use nix::{
        libc,
        sys::timerfd::{ClockId, TimerFd, TimerFlags, TimerSetTimeFlags},
        unistd::read,
    };
    use std::{io, os::fd::AsFd, time::Duration};
    use tokio::io::unix::AsyncFd;

    /// Timer based on `CLOCK_REALTIME` for wall-clock time operations.
    ///
    /// Useful for scheduling events at specific times of day. Supports
    /// cancellation when the system clock is adjusted (e.g., NTP).
    pub struct RealtimeTimer {
        timer_fd: TimerFd,
    }

    pub enum RealtimeSleepEnd {
        Completed,
        Cancelled,
    }

    impl RealtimeTimer {
        pub fn new() -> io::Result<Self> {
            Ok(Self {
                timer_fd: TimerFd::new(ClockId::CLOCK_REALTIME, TimerFlags::TFD_NONBLOCK)?,
            })
        }

        async fn wait_for_timer(&mut self) -> Result<RealtimeSleepEnd, io::Error> {
            let fd = self.timer_fd.as_fd();
            let async_fd = AsyncFd::new(fd)?;

            loop {
                let mut guard = async_fd.readable().await?;

                let mut buffer = [0u8; 8];
                match guard.try_io(|inner| Ok(read(inner.get_ref(), &mut buffer)?)) {
                    Ok(result) => {
                        return match result {
                            Ok(_) => Ok(RealtimeSleepEnd::Completed),
                            Err(e) if e.raw_os_error() == Some(libc::ECANCELED) => {
                                Ok(RealtimeSleepEnd::Cancelled)
                            }
                            Err(e) => Err(e),
                        };
                    }
                    Err(_) => continue,
                }
            }
        }

        pub async fn sleep(&mut self, duration: Duration) -> Result<RealtimeSleepEnd, io::Error> {
            self.timer_fd.set(
                nix::sys::timerfd::Expiration::OneShot(duration.into()),
                TimerSetTimeFlags::TFD_TIMER_CANCEL_ON_SET,
            )?;
            self.wait_for_timer().await
        }

        pub async fn sleep_until(
            &mut self,
            target: Timestamp,
        ) -> Result<RealtimeSleepEnd, io::Error> {
            use nix::sys::time::TimeSpec;

            let secs = target.as_second();
            let nanos = target.subsec_nanosecond();
            let timespec = TimeSpec::new(secs, nanos as i64);

            self.timer_fd.set(
                nix::sys::timerfd::Expiration::OneShot(timespec),
                TimerSetTimeFlags::TFD_TIMER_ABSTIME | TimerSetTimeFlags::TFD_TIMER_CANCEL_ON_SET,
            )?;
            self.wait_for_timer().await
        }
    }

    /// Timer based on `CLOCK_BOOTTIME` for suspend-aware timing.
    ///
    /// Unlike `tokio::time::sleep` (which uses `CLOCK_MONOTONIC`), this timer
    /// continues to track time during system suspend. When the system resumes,
    /// the timer will fire immediately if its deadline has passed.
    ///
    /// This is useful for rest/break timers where we want "5 minutes of real
    /// time" including any time the machine was asleep.
    pub struct BoottimeTimer {
        timer_fd: TimerFd,
    }

    impl BoottimeTimer {
        pub fn new() -> io::Result<Self> {
            Ok(Self {
                timer_fd: TimerFd::new(ClockId::CLOCK_BOOTTIME, TimerFlags::TFD_NONBLOCK)?,
            })
        }

        async fn wait_for_timer(&mut self) -> io::Result<()> {
            let fd = self.timer_fd.as_fd();
            let async_fd = AsyncFd::new(fd)?;

            loop {
                let mut guard = async_fd.readable().await?;

                let mut buffer = [0u8; 8];
                match guard.try_io(|inner| Ok(read(inner.get_ref(), &mut buffer)?)) {
                    Ok(result) => {
                        result?;
                        return Ok(());
                    }
                    Err(_) => continue,
                }
            }
        }

        /// Sleep for a duration. Includes time spent in system suspend.
        pub async fn sleep(&mut self, duration: Duration) -> io::Result<()> {
            self.timer_fd.set(
                nix::sys::timerfd::Expiration::OneShot(duration.into()),
                TimerSetTimeFlags::empty(),
            )?;
            self.wait_for_timer().await
        }

        /// Sleep until a specific boot instant. Includes time spent in system suspend.
        pub async fn sleep_until(&mut self, target: BootInstant) -> io::Result<()> {
            self.timer_fd.set(
                nix::sys::timerfd::Expiration::OneShot(target.as_timespec()),
                TimerSetTimeFlags::TFD_TIMER_ABSTIME,
            )?;
            self.wait_for_timer().await
        }
    }
}
