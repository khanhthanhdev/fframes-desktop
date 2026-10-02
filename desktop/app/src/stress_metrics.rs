use serde::Serialize;
use std::time::Instant;

#[derive(Serialize)]
pub struct MemorySample {
    pub verified_render_requests: usize,
    pub host_rss_bytes: Option<u64>,
    pub worker_rss_bytes: Option<u64>,
}

#[derive(Default, Serialize)]
pub struct StressMetrics {
    pub confirmed_presentations: usize,
    pub verified_render_requests: usize,
    pub coalesced_seeks: usize,
    pub paint_submissions: usize,
    pub release_requests: usize,
    pub release_failures: usize,
    pub queue_high_water: usize,
    pub managed_current_images_high_water: usize,
    pub host_rss_baseline_bytes: Option<u64>,
    pub host_rss_high_water_bytes: Option<u64>,
    pub worker_rss_high_water_bytes: Option<u64>,
    pub memory_samples: Vec<MemorySample>,
    pub native_input_observed: String,
    pub selected_source: Option<crate::selection_spike::SelectedElementInfo>,
    pub heartbeat_samples: usize,
    pub heartbeat_max_gap_ms: f64,
    pub render_transfer_ms: f64,
    pub conversion_ms: f64,
    pub elapsed_ms: f64,
    pub completed: bool,
    pub failure: Option<String>,
    #[serde(skip)]
    pub last_heartbeat: Option<Instant>,
    #[serde(skip)]
    pub last_completion: Option<Instant>,
    #[serde(skip)]
    pub release_baseline: usize,
}

impl StressMetrics {
    pub fn heartbeat(&mut self) {
        let now = Instant::now();
        if let Some(previous) = self.last_heartbeat.replace(now) {
            self.heartbeat_max_gap_ms = self
                .heartbeat_max_gap_ms
                .max(now.duration_since(previous).as_secs_f64() * 1_000.);
        }
        self.heartbeat_samples += 1;
    }
    pub fn sample_memory(&mut self, host: Option<u64>, worker: Option<u64>) {
        if self.verified_render_requests.is_multiple_of(50) {
            self.memory_samples.push(MemorySample {
                verified_render_requests: self.verified_render_requests,
                host_rss_bytes: host,
                worker_rss_bytes: worker,
            });
        }
        if self.host_rss_baseline_bytes.is_none() {
            self.host_rss_baseline_bytes = host;
        }
        if let Some(value) = host {
            self.host_rss_high_water_bytes =
                Some(self.host_rss_high_water_bytes.unwrap_or(0).max(value));
        }
        if let Some(value) = worker {
            self.worker_rss_high_water_bytes =
                Some(self.worker_rss_high_water_bytes.unwrap_or(0).max(value));
        }
    }
}

/// Resident process memory. Unsupported platforms return no measurement.
pub fn resident_bytes(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        status.lines().find_map(|line| {
            line.strip_prefix("VmRSS:")
                .and_then(|value| value.split_whitespace().next()?.parse::<u64>().ok())
                .and_then(|value| value.checked_mul(1024))
        })
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8(output.stdout)
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()?
            .checked_mul(1024)
    }
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::{
                ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
                Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ},
            },
        };
        // The handle is read-only and closed on both success and failure.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
            if process.is_null() {
                return None;
            }
            let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            let success = K32GetProcessMemoryInfo(process, &mut counters, counters.cb);
            CloseHandle(process);
            (success != 0).then_some(counters.WorkingSetSize as u64)
        }
    }
}
