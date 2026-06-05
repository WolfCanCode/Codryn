//! Memory pressure management for the indexing pipeline.
//!
//! Monitors process RSS and triggers buffer flushes when memory usage
//! exceeds a configurable threshold (default: 80% of limit).
//! Also provides LRU eviction for the FileCache when entries exceed 10,000.

use std::sync::atomic::{AtomicU64, Ordering};

/// Default memory limit: 2 GB.
const DEFAULT_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Default threshold percentage (80%).
const DEFAULT_THRESHOLD_PCT: f64 = 0.80;

/// Maximum FileCache entries before LRU eviction kicks in.
pub const FILE_CACHE_MAX_ENTRIES: usize = 10_000;

/// Monitors process memory usage and triggers backpressure.
pub struct MemoryMonitor {
    /// Maximum allowed memory in bytes.
    limit_bytes: u64,
    /// Threshold as a fraction (0.0–1.0) of limit_bytes.
    threshold_pct: f64,
    /// Tracks the highest RSS observed during this monitor's lifetime.
    high_water_mark: AtomicU64,
}

impl MemoryMonitor {
    /// Create a new MemoryMonitor with the given memory limit in bytes.
    /// The threshold defaults to 80%.
    pub fn new(limit_bytes: u64) -> Self {
        Self {
            limit_bytes,
            threshold_pct: DEFAULT_THRESHOLD_PCT,
            high_water_mark: AtomicU64::new(0),
        }
    }

    /// Create a MemoryMonitor from an `AppConfig.max_memory_mb` value.
    /// If `None`, uses the default limit of 2048 MB.
    pub fn from_config(max_memory_mb: Option<u64>) -> Self {
        let limit_bytes = max_memory_mb
            .map(|mb| mb * 1024 * 1024)
            .unwrap_or(DEFAULT_LIMIT_BYTES);
        Self::new(limit_bytes)
    }

    /// Get the configured memory limit in bytes.
    pub fn limit_bytes(&self) -> u64 {
        self.limit_bytes
    }

    /// Get the configured threshold percentage.
    pub fn threshold_pct(&self) -> f64 {
        self.threshold_pct
    }

    /// Get current process RSS (Resident Set Size) in bytes.
    ///
    /// Platform-specific:
    /// - Linux: reads `/proc/self/statm` and multiplies by page size
    /// - macOS: uses `mach_task_info` (MACH_TASK_BASIC_INFO)
    /// - Other platforms: returns 0
    pub fn current_rss() -> u64 {
        platform_rss()
    }

    /// Returns `true` if current memory usage exceeds the threshold.
    /// Also updates the high-water mark.
    pub fn should_flush(&self) -> bool {
        let rss = Self::current_rss();
        // Update high-water mark
        self.high_water_mark.fetch_max(rss, Ordering::Relaxed);
        let threshold_bytes = (self.limit_bytes as f64 * self.threshold_pct) as u64;
        rss > threshold_bytes
    }

    /// Log the high-water mark at end of run.
    pub fn log_high_water_mark(&self) {
        let hwm = self.high_water_mark.load(Ordering::Relaxed);
        let hwm_mb = hwm as f64 / (1024.0 * 1024.0);
        let limit_mb = self.limit_bytes as f64 / (1024.0 * 1024.0);
        let pct = if self.limit_bytes > 0 {
            (hwm as f64 / self.limit_bytes as f64) * 100.0
        } else {
            0.0
        };
        tracing::info!(
            high_water_mark_mb = format!("{:.1}", hwm_mb),
            limit_mb = format!("{:.0}", limit_mb),
            usage_pct = format!("{:.1}%", pct),
            "pipeline: memory high-water mark"
        );
    }

    /// Get the current high-water mark value in bytes.
    pub fn high_water_mark(&self) -> u64 {
        self.high_water_mark.load(Ordering::Relaxed)
    }
}

/// Platform-specific RSS reading.
#[cfg(target_os = "linux")]
fn platform_rss() -> u64 {
    // Read /proc/self/statm: fields are in pages
    // Field 1 (index 1) is RSS in pages
    match std::fs::read_to_string("/proc/self/statm") {
        Ok(content) => {
            let fields: Vec<&str> = content.split_whitespace().collect();
            if fields.len() >= 2 {
                if let Ok(pages) = fields[1].parse::<u64>() {
                    let page_size = page_size_bytes();
                    return pages * page_size;
                }
            }
            0
        }
        Err(_) => 0,
    }
}

#[cfg(target_os = "linux")]
fn page_size_bytes() -> u64 {
    // SAFETY: sysconf(_SC_PAGESIZE) is always safe to call
    unsafe {
        let ps = libc::sysconf(libc::_SC_PAGESIZE);
        if ps > 0 {
            ps as u64
        } else {
            4096 // fallback
        }
    }
}

#[cfg(target_os = "macos")]
fn platform_rss() -> u64 {
    use std::mem;

    // Mach kernel types and constants for task_info
    #[allow(non_camel_case_types)]
    type mach_port_t = u32;
    #[allow(non_camel_case_types)]
    type kern_return_t = i32;
    #[allow(non_camel_case_types)]
    type task_flavor_t = u32;
    #[allow(non_camel_case_types)]
    type task_info_t = *mut i32;
    #[allow(non_camel_case_types)]
    type mach_msg_type_number_t = u32;

    const MACH_TASK_BASIC_INFO: task_flavor_t = 20;
    const KERN_SUCCESS: kern_return_t = 0;

    #[repr(C)]
    #[allow(non_camel_case_types)]
    struct mach_task_basic_info {
        virtual_size: u64,
        resident_size: u64,
        resident_size_max: u64,
        user_time: [u32; 2],   // time_value_t
        system_time: [u32; 2], // time_value_t
        policy: i32,
        suspend_count: i32,
    }

    extern "C" {
        fn mach_task_self() -> mach_port_t;
        fn task_info(
            target_task: mach_port_t,
            flavor: task_flavor_t,
            task_info_out: task_info_t,
            task_info_outCnt: *mut mach_msg_type_number_t,
        ) -> kern_return_t;
    }

    unsafe {
        let task = mach_task_self();
        let mut info: mach_task_basic_info = mem::zeroed();
        let mut count = (mem::size_of::<mach_task_basic_info>() / mem::size_of::<u32>())
            as mach_msg_type_number_t;

        let kr = task_info(
            task,
            MACH_TASK_BASIC_INFO,
            &mut info as *mut _ as task_info_t,
            &mut count,
        );

        if kr == KERN_SUCCESS {
            info.resident_size
        } else {
            0
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn platform_rss() -> u64 {
    0
}

/// Returns the peak RSS (Resident Set Size) of the current process in megabytes.
///
/// Platform-specific:
/// - Linux: reads VmHWM from `/proc/self/status` (kernel-tracked high-water mark)
/// - macOS: uses `mach_task_info` (MACH_TASK_BASIC_INFO) `resident_size_max`
/// - Other platforms: returns `None`
pub fn peak_rss_mb() -> Option<u64> {
    let bytes = platform_peak_rss();
    if bytes > 0 {
        Some(bytes / (1024 * 1024))
    } else {
        None
    }
}

/// Returns a memory warning message if peak RSS exceeds 80% of the configured limit.
/// Returns `None` if usage is within acceptable bounds or if the platform is unsupported.
pub fn memory_warning(limit_mb: Option<u64>) -> Option<String> {
    let peak = peak_rss_mb()?;
    let limit = limit_mb.unwrap_or(2048); // default 2 GB
    let threshold = (limit as f64 * DEFAULT_THRESHOLD_PCT) as u64;
    if peak > threshold {
        let pct = (peak as f64 / limit as f64) * 100.0;
        Some(format!(
            "Peak memory usage ({peak} MB) exceeds 80% of configured limit ({limit} MB) — {pct:.1}% used"
        ))
    } else {
        None
    }
}

/// Platform-specific peak RSS reading (in bytes).
#[cfg(target_os = "linux")]
fn platform_peak_rss() -> u64 {
    // Read VmHWM (high-water mark) from /proc/self/status
    match std::fs::read_to_string("/proc/self/status") {
        Ok(content) => {
            for line in content.lines() {
                if line.starts_with("VmHWM:") {
                    // Format: "VmHWM:    12345 kB"
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(kb) = parts[1].parse::<u64>() {
                            return kb * 1024; // Convert kB to bytes
                        }
                    }
                }
            }
            0
        }
        Err(_) => 0,
    }
}

#[cfg(target_os = "macos")]
fn platform_peak_rss() -> u64 {
    use std::mem;

    // Reuse the same Mach kernel types from platform_rss
    #[allow(non_camel_case_types)]
    type mach_port_t = u32;
    #[allow(non_camel_case_types)]
    type kern_return_t = i32;
    #[allow(non_camel_case_types)]
    type task_flavor_t = u32;
    #[allow(non_camel_case_types)]
    type task_info_t = *mut i32;
    #[allow(non_camel_case_types)]
    type mach_msg_type_number_t = u32;

    const MACH_TASK_BASIC_INFO: task_flavor_t = 20;
    const KERN_SUCCESS: kern_return_t = 0;

    #[repr(C)]
    #[allow(non_camel_case_types)]
    struct mach_task_basic_info {
        virtual_size: u64,
        resident_size: u64,
        resident_size_max: u64,
        user_time: [u32; 2],
        system_time: [u32; 2],
        policy: i32,
        suspend_count: i32,
    }

    extern "C" {
        fn mach_task_self() -> mach_port_t;
        fn task_info(
            target_task: mach_port_t,
            flavor: task_flavor_t,
            task_info_out: task_info_t,
            task_info_outCnt: *mut mach_msg_type_number_t,
        ) -> kern_return_t;
    }

    unsafe {
        let task = mach_task_self();
        let mut info: mach_task_basic_info = mem::zeroed();
        let mut count = (mem::size_of::<mach_task_basic_info>() / mem::size_of::<u32>())
            as mach_msg_type_number_t;

        let kr = task_info(
            task,
            MACH_TASK_BASIC_INFO,
            &mut info as *mut _ as task_info_t,
            &mut count,
        );

        if kr == KERN_SUCCESS {
            info.resident_size_max
        } else {
            0
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn platform_peak_rss() -> u64 {
    0
}

/// LRU-aware eviction for the FileCache.
/// When the cache exceeds `FILE_CACHE_MAX_ENTRIES`, this removes
/// approximately half the entries (the oldest inserted ones).
///
/// Since `FileCache` uses a `HashMap` without access-time tracking,
/// we implement a simple size-based eviction that clears the oldest
/// half of entries by draining and re-inserting the newest half.
pub fn evict_file_cache_if_needed(cache: &mut super::FileCache) {
    if cache.len() > FILE_CACHE_MAX_ENTRIES {
        let target_size = FILE_CACHE_MAX_ENTRIES / 2;
        let to_remove = cache.len() - target_size;
        tracing::info!(
            entries = cache.len(),
            evicting = to_remove,
            "pipeline: FileCache LRU eviction triggered"
        );
        cache.evict_oldest(to_remove);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_with_defaults() {
        let monitor = MemoryMonitor::new(DEFAULT_LIMIT_BYTES);
        assert_eq!(monitor.limit_bytes(), DEFAULT_LIMIT_BYTES);
        assert_eq!(monitor.threshold_pct(), DEFAULT_THRESHOLD_PCT);
        assert_eq!(monitor.high_water_mark(), 0);
    }

    #[test]
    fn test_from_config_with_value() {
        let monitor = MemoryMonitor::from_config(Some(4096));
        assert_eq!(monitor.limit_bytes(), 4096 * 1024 * 1024);
    }

    #[test]
    fn test_from_config_none_uses_default() {
        let monitor = MemoryMonitor::from_config(None);
        assert_eq!(monitor.limit_bytes(), DEFAULT_LIMIT_BYTES);
    }

    #[test]
    fn test_current_rss_returns_nonzero() {
        // On supported platforms (Linux/macOS), RSS should be > 0
        let rss = MemoryMonitor::current_rss();
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(rss > 0, "RSS should be non-zero on supported platforms");
        }
    }

    #[test]
    fn test_should_flush_below_threshold() {
        // Set a very high limit so we're always below threshold
        let monitor = MemoryMonitor::new(u64::MAX);
        assert!(!monitor.should_flush());
    }

    #[test]
    fn test_should_flush_above_threshold() {
        // Set a very low limit so we're always above threshold
        let monitor = MemoryMonitor::new(1); // 1 byte limit
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(monitor.should_flush());
        }
    }

    #[test]
    fn test_high_water_mark_tracking() {
        let monitor = MemoryMonitor::new(u64::MAX);
        // Call should_flush to trigger high-water-mark update
        monitor.should_flush();
        let hwm = monitor.high_water_mark();
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(
                hwm > 0,
                "High-water mark should be updated after should_flush()"
            );
        }
    }

    #[test]
    fn test_high_water_mark_only_increases() {
        let monitor = MemoryMonitor::new(u64::MAX);
        monitor.should_flush();
        let hwm1 = monitor.high_water_mark();
        monitor.should_flush();
        let hwm2 = monitor.high_water_mark();
        // High-water mark should never decrease
        assert!(hwm2 >= hwm1);
    }

    #[test]
    fn test_evict_file_cache_below_limit() {
        let mut cache = super::super::FileCache::new();
        // Add fewer than FILE_CACHE_MAX_ENTRIES
        for i in 0..100 {
            cache.insert(
                std::path::PathBuf::from(format!("/tmp/file_{}.rs", i)),
                std::sync::Arc::new(format!("content {}", i)),
            );
        }
        evict_file_cache_if_needed(&mut cache);
        // Should not evict anything
        assert_eq!(cache.len(), 100);
    }

    #[test]
    fn test_evict_file_cache_above_limit() {
        let mut cache = super::super::FileCache::new();
        // Add more than FILE_CACHE_MAX_ENTRIES
        let count = FILE_CACHE_MAX_ENTRIES + 500;
        for i in 0..count {
            cache.insert(
                std::path::PathBuf::from(format!("/tmp/file_{}.rs", i)),
                std::sync::Arc::new(format!("content {}", i)),
            );
        }
        assert_eq!(cache.len(), count);
        evict_file_cache_if_needed(&mut cache);
        // Should evict down to FILE_CACHE_MAX_ENTRIES / 2
        assert_eq!(cache.len(), FILE_CACHE_MAX_ENTRIES / 2);
    }

    #[test]
    fn test_peak_rss_mb_returns_value_on_supported_platforms() {
        let peak = peak_rss_mb();
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(
                peak.is_some(),
                "peak_rss_mb should return Some on supported platforms"
            );
            assert!(peak.unwrap() > 0, "peak RSS should be > 0 MB");
        } else {
            assert!(
                peak.is_none(),
                "peak_rss_mb should return None on unsupported platforms"
            );
        }
    }

    #[test]
    fn test_memory_warning_below_threshold() {
        // With a very high limit, no warning should be generated
        let warning = memory_warning(Some(1_000_000)); // 1 TB limit
        assert!(
            warning.is_none(),
            "No warning expected when well below threshold"
        );
    }

    #[test]
    fn test_memory_warning_above_threshold() {
        // With a very low limit (1 MB), the process should exceed 80%
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            let warning = memory_warning(Some(1)); // 1 MB limit
            assert!(warning.is_some(), "Warning expected when above threshold");
            let msg = warning.unwrap();
            assert!(
                msg.contains("exceeds 80%"),
                "Warning should mention 80% threshold"
            );
        }
    }

    #[test]
    fn test_memory_warning_default_limit() {
        // With default limit (None = 2048 MB), typical test process should be below threshold
        let warning = memory_warning(None);
        // This test just verifies it doesn't panic; actual result depends on process size
        let _ = warning;
    }
}
