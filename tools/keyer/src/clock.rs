use std::{
    hint, thread,
    time::{Duration, Instant},
};

/// How long before a deadline the wait stops sleeping and starts spinning.
const SPIN: Duration = Duration::from_millis(1);

/// The time base keying is laid out against.
pub trait Clock: Send {
    /// How long since the clock started.
    fn now(&self) -> Duration;
    /// Returns once [`now`](Self::now) has reached `deadline`.
    fn sleep_until(&self, deadline: Duration);
}

/// The host's monotonic clock, waited on as tightly as the platform allows.
pub struct SystemClock {
    origin: Instant,
    #[cfg(windows)]
    timer: Option<windows::Timer>,
}

impl SystemClock {
    /// Starts the clock now.
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            #[cfg(windows)]
            timer: windows::Timer::new(),
        }
    }

    fn sleep(&self, duration: Duration) {
        #[cfg(windows)]
        if let Some(timer) = &self.timer {
            timer.sleep(duration);
            return;
        }
        thread::sleep(duration);
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn sleep_until(&self, deadline: Duration) {
        let now = self.now();
        if let Some(remaining) = deadline.checked_sub(now + SPIN) {
            self.sleep(remaining);
        }
        while self.now() < deadline {
            hint::spin_loop();
        }
    }
}

/// Raises the calling thread to the priority keying needs, where the platform
/// lets an unprivileged process ask for it.
pub fn raise_thread_priority() {
    #[cfg(windows)]
    windows::raise_thread_priority();
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows {
    use std::{ptr, time::Duration};

    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::Threading::{
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, GetCurrentThread, INFINITE,
            SetThreadPriority, SetWaitableTimer, THREAD_PRIORITY_TIME_CRITICAL, TIMER_ALL_ACCESS, WaitForSingleObject,
        },
    };

    pub struct Timer(HANDLE);

    unsafe impl Send for Timer {}

    impl Timer {
        pub fn new() -> Option<Self> {
            let handle = unsafe {
                CreateWaitableTimerExW(
                    ptr::null(),
                    ptr::null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                    TIMER_ALL_ACCESS,
                )
            };
            (!handle.is_null()).then_some(Self(handle))
        }

        pub fn sleep(&self, duration: Duration) {
            let intervals = i64::try_from(duration.as_nanos() / 100).unwrap_or(i64::MAX);
            if intervals == 0 {
                return;
            }
            let due = -intervals;
            unsafe {
                if SetWaitableTimer(self.0, &due, 0, None, ptr::null(), 0) != 0 {
                    WaitForSingleObject(self.0, INFINITE);
                }
            }
        }
    }

    impl Drop for Timer {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub fn raise_thread_priority() {
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_never_returns_before_its_deadline() {
        let clock = SystemClock::new();
        for step in 1..=20 {
            let deadline = Duration::from_micros(2_500 * step);
            clock.sleep_until(deadline);
            assert!(clock.now() >= deadline);
        }
    }
}
