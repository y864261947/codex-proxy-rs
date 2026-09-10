use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use gateway_core::engine::traffic::{TrafficClock, TrafficMonitor};

#[derive(Default)]
struct Clock(AtomicU64);

impl TrafficClock for Clock {
    fn elapsed(&self) -> Duration {
        Duration::from_secs(self.0.load(Ordering::SeqCst))
    }
}

#[test]
fn ingress_expires_without_new_requests_and_survives_ring_wrap() {
    let clock = Arc::new(Clock::default());
    let traffic = TrafficMonitor::with_clock(clock.clone());
    traffic.record_ingress();
    clock.0.store(59, Ordering::SeqCst);
    traffic.record_ingress();
    assert_eq!(traffic.snapshot().ingress_requests_last_minute, 2);
    clock.0.store(60, Ordering::SeqCst);
    assert_eq!(traffic.snapshot().ingress_requests_last_minute, 1);
    traffic.record_ingress();
    assert_eq!(traffic.snapshot().ingress_requests_last_minute, 2);
    clock.0.store(180, Ordering::SeqCst);
    assert_eq!(traffic.snapshot().ingress_requests_last_minute, 0);
    traffic.record_ingress();
    assert_eq!(traffic.snapshot().ingress_requests_last_minute, 1);
}

#[test]
fn logical_execution_is_shared_and_released_in_both_phases() {
    let traffic = TrafficMonitor::default();
    let other = traffic.clone();
    let preparing = traffic.begin_execution();
    let mut running = other.begin_execution();
    running.mark_executing();
    running.mark_executing();
    let snapshot = traffic.snapshot();
    assert_eq!(snapshot.in_flight_requests, 2);
    assert_eq!(snapshot.preparing_requests, 1);
    assert_eq!(snapshot.executing_requests, 1);
    // Execution tracking cannot accidentally increment ingress RPM.
    assert_eq!(snapshot.ingress_requests_last_minute, 0);
    drop(preparing);
    assert_eq!(other.snapshot().in_flight_requests, 1);
    drop(running);
    assert_eq!(traffic.snapshot().in_flight_requests, 0);
}

#[test]
fn concurrent_recorders_do_not_lose_counts() {
    let traffic = TrafficMonitor::with_clock(Arc::new(Clock::default()));
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let traffic = &traffic;
            scope.spawn(move || {
                for _ in 0..1000 {
                    traffic.record_ingress();
                    let mut lease = traffic.begin_execution();
                    lease.mark_executing();
                }
            });
        }
    });
    assert_eq!(traffic.snapshot().ingress_requests_last_minute, 8000);
    assert_eq!(traffic.snapshot().in_flight_requests, 0);
}
