//! 单进程实时流量事实；不读取完成日志，不参与准入或账号调度。

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const WINDOW_SECONDS: u64 = 60;

/// 单调时钟，不受系统时间校准影响。
pub trait TrafficClock: Send + Sync {
    fn elapsed(&self) -> Duration;
}

struct MonotonicClock(Instant);

impl TrafficClock for MonotonicClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
}

#[derive(Clone, Copy, Default)]
struct Bucket {
    second: u64,
    requests: u64,
}

struct TrafficState {
    buckets: [Bucket; WINDOW_SECONDS as usize],
    preparing: u64,
    executing: u64,
}

/// 当前进程的轻量快照。RPM 使用当前秒及之前 59 秒的桶，精度为 1 秒。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrafficSnapshot {
    pub window_seconds: u64,
    pub uptime_seconds: u64,
    pub ingress_requests_last_minute: u64,
    pub in_flight_requests: u64,
    pub preparing_requests: u64,
    pub executing_requests: u64,
}

/// 所有克隆共享同一份有界计数，重启后清零。
#[derive(Clone)]
pub struct TrafficMonitor {
    state: Arc<Mutex<TrafficState>>,
    clock: Arc<dyn TrafficClock>,
}

impl Default for TrafficMonitor {
    fn default() -> Self {
        Self::with_clock(Arc::new(MonotonicClock(Instant::now())))
    }
}

impl TrafficMonitor {
    #[must_use]
    pub fn with_clock(clock: Arc<dyn TrafficClock>) -> Self {
        Self {
            state: Arc::new(Mutex::new(TrafficState {
                buckets: [Bucket::default(); WINDOW_SECONDS as usize],
                preparing: 0,
                executing: 0,
            })),
            clock,
        }
    }

    fn lock(&self) -> MutexGuard<'_, TrafficState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// API 在每个业务 POST / WebSocket 创建消息的入口调用一次。
    pub fn record_ingress(&self) {
        let mut state = self.lock();
        let second = self.clock.elapsed().as_secs();
        let bucket = &mut state.buckets[(second % WINDOW_SECONDS) as usize];
        if bucket.second != second {
            *bucket = Bucket {
                second,
                requests: 0,
            };
        }
        bucket.requests = bucket.requests.saturating_add(1);
    }

    /// Core 进入逻辑执行时持有；准备失败或 future 被取消也会自动释放。
    pub fn begin_execution(&self) -> TrafficLease {
        self.lock().preparing += 1;
        TrafficLease {
            monitor: self.clone(),
            executing: false,
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> TrafficSnapshot {
        let state = self.lock();
        let second = self.clock.elapsed().as_secs();
        TrafficSnapshot {
            window_seconds: WINDOW_SECONDS,
            uptime_seconds: second,
            ingress_requests_last_minute: state
                .buckets
                .iter()
                .filter(|bucket| second.saturating_sub(bucket.second) < WINDOW_SECONDS)
                .fold(0_u64, |sum, bucket| sum.saturating_add(bucket.requests)),
            in_flight_requests: state.preparing + state.executing,
            preparing_requests: state.preparing,
            executing_requests: state.executing,
        }
    }
}

/// 一次逻辑执行只有一个 lease，上游重试不能创建新 lease。
#[must_use]
pub struct TrafficLease {
    monitor: TrafficMonitor,
    executing: bool,
}

impl TrafficLease {
    /// 首个执行会话准备完成；之后覆盖流式交付和取消收敛。
    pub fn mark_executing(&mut self) {
        if !self.executing {
            let mut state = self.monitor.lock();
            state.preparing -= 1;
            state.executing += 1;
            self.executing = true;
        }
    }
}

impl Drop for TrafficLease {
    fn drop(&mut self) {
        let mut state = self.monitor.lock();
        if self.executing {
            state.executing -= 1;
        } else {
            state.preparing -= 1;
        }
    }
}
