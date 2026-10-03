use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use sysinfo::System;
use tauri::{AppHandle, Emitter};
use tokio::time::{sleep, Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPayload {
    pub total_mb: u64,
    pub used_mb: u64,
    pub percent: f32,
    pub threshold: u32,
    pub is_warning: bool,
}

pub struct MemoryMonitor {
    pub threshold: Arc<AtomicU32>,
    /// 最近一次采样的结果。
    ///
    /// 界面刚打开时先来问一次 —— 否则第一帧只能显示一个编造的占用率
    /// （曾经写死成 16384/7200/44%），在一段时间里看着像真的。
    latest: Arc<RwLock<Option<MemoryPayload>>>,
}

impl MemoryMonitor {
    pub fn new(initial_threshold: u32) -> Self {
        Self {
            threshold: Arc::new(AtomicU32::new(initial_threshold)),
            latest: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set_threshold(&self, val: u32) {
        self.threshold.store(val, Ordering::Relaxed);
    }

    #[allow(dead_code)]
    pub fn get_threshold(&self) -> u32 {
        self.threshold.load(Ordering::Relaxed)
    }

    /// 最近一次采样（主循环每 2 秒刷一次，所以基本总是有值）。
    pub fn latest(&self) -> Option<MemoryPayload> {
        self.latest.read().clone()
    }

    pub fn start_loop(&self, app_handle: AppHandle) {
        let threshold_arc = Arc::clone(&self.threshold);
        let latest = Arc::clone(&self.latest);

        tauri::async_runtime::spawn(async move {
            let mut sys = System::new_all();

            loop {
                sys.refresh_memory();

                let total = sys.total_memory();
                let used = sys.used_memory();

                let percent = if total > 0 {
                    ((used as f64 / total as f64) * 100.0) as f32
                } else {
                    0.0
                };

                let current_thresh = threshold_arc.load(Ordering::Relaxed);
                let is_warning = percent >= current_thresh as f32;

                let payload = MemoryPayload {
                    total_mb: total / 1024 / 1024,
                    used_mb: used / 1024 / 1024,
                    percent,
                    threshold: current_thresh,
                    is_warning,
                };

                *latest.write() = Some(payload.clone());
                let _ = app_handle.emit("memory-update", &payload);

                sleep(Duration::from_secs(2)).await;
            }
        });
    }
}
