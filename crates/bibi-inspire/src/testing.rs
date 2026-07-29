//! A clock that never sleeps, for asserting on schedules rather than on time.

use crate::rate_limit::Clock;
use std::{
    future::Future,
    pin::Pin,
    sync::Mutex,
    time::{Duration, Instant},
};

/// A clock that records the waits it was asked for and returns immediately.
///
/// Pacing puts sleeps on the success path, so a suite using the real clock
/// would take minutes to assert something that is really about arithmetic. This
/// advances a virtual instant by exactly the duration requested, which makes
/// the rate limiter's window roll forward deterministically.
#[derive(Debug)]
pub struct TestClock {
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    now: Instant,
    waits: Vec<Duration>,
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new()
    }
}

impl TestClock {
    /// A clock starting now, having waited for nothing.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                now: Instant::now(),
                waits: Vec::new(),
            }),
        }
    }

    /// Every wait this clock was asked for, in order.
    pub fn waits(&self) -> Vec<Duration> {
        self.state.lock().expect("test clock").waits.clone()
    }

    /// How far the virtual clock has moved.
    pub fn elapsed(&self) -> Duration {
        let state = self.state.lock().expect("test clock");
        state.waits.iter().sum()
    }

    /// Move the clock forward without recording a wait.
    pub fn advance(&self, duration: Duration) {
        let mut state = self.state.lock().expect("test clock");
        state.now += duration;
    }
}

impl Clock for TestClock {
    fn now(&self) -> Instant {
        self.state.lock().expect("test clock").now
    }

    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        {
            let mut state = self.state.lock().expect("test clock");
            state.waits.push(duration);
            state.now += duration;
        }
        Box::pin(async {})
    }
}
