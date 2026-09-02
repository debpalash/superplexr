//! Lightweight process telemetry for the in-app performance overlay.

use std::time::Duration;
#[cfg(not(test))]
use std::{thread, time::Instant};

#[cfg(not(test))]
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, get_current_pid};

#[cfg(not(test))]
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PerformanceSample {
    pub(crate) cpu_percent: f32,
    pub(crate) resident_bytes: u64,
    pub(crate) read_bytes_per_second: f64,
    pub(crate) written_bytes_per_second: f64,
    pub(crate) uptime_seconds: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PerformanceStats {
    pub(crate) fps: f32,
    pub(crate) peak_fps: f32,
    pub(crate) benchmark_fps: Option<f32>,
    pub(crate) display_max_fps: Option<f32>,
    pub(crate) frame_p95_ms: f32,
    pub(crate) cpu_percent: f32,
    pub(crate) resident_bytes: u64,
    pub(crate) read_bytes_per_second: f64,
    pub(crate) written_bytes_per_second: f64,
    pub(crate) uptime_seconds: u64,
    pub(crate) ready: bool,
}

/// Snaps a measured vsync rate to a nearby standard display mode. Frame
/// callback probes have small scheduling jitter, so a 120 Hz panel may be
/// observed as 118–119 callbacks per second.
pub(crate) fn normalized_display_fps(measured: f32) -> f32 {
    const COMMON_REFRESH_RATES: [f32; 15] = [
        24.0, 30.0, 48.0, 50.0, 60.0, 72.0, 75.0, 90.0, 100.0, 120.0, 144.0, 165.0, 180.0, 240.0,
        360.0,
    ];
    let nearest = COMMON_REFRESH_RATES
        .into_iter()
        .min_by(|left, right| {
            (measured - *left)
                .abs()
                .total_cmp(&(measured - *right).abs())
        })
        .unwrap_or(measured.round());
    if (measured - nearest).abs() / nearest <= 0.08 {
        nearest
    } else {
        measured.round()
    }
}

/// Moves the render probe from edge to edge twice per second. The motion is
/// time-based so slow frames remain visible instead of slowing the animation.
pub(crate) fn render_probe_progress(elapsed: Duration) -> f32 {
    let half_cycles = elapsed.as_secs_f32() * 2.0;
    let progress = half_cycles % 2.0;
    if progress <= 1.0 {
        progress
    } else {
        2.0 - progress
    }
}

/// Calculates actual completed window draws for the probe interval. Callback
/// cadence alone can report a refresh rate even when no graphics were painted.
pub(crate) fn measured_render_fps(
    starting_frame_count: u64,
    ending_frame_count: u64,
    elapsed: Duration,
) -> Option<f32> {
    let elapsed_seconds = elapsed.as_secs_f32();
    let rendered_frames = ending_frame_count.saturating_sub(starting_frame_count);
    (rendered_frames > 0 && elapsed_seconds > 0.0)
        .then_some(rendered_frames as f32 / elapsed_seconds)
}

impl PerformanceStats {
    pub(crate) fn update_fps(&mut self, fps: f32) {
        self.fps = fps;
        self.peak_fps = self.peak_fps.max(fps);
    }

    pub(crate) fn record_benchmark(&mut self, fps: f32) {
        self.benchmark_fps = Some(fps);
        self.peak_fps = self.peak_fps.max(fps);
    }

    pub(crate) fn update_process(&mut self, sample: PerformanceSample) {
        self.cpu_percent = sample.cpu_percent;
        self.resident_bytes = sample.resident_bytes;
        self.read_bytes_per_second = sample.read_bytes_per_second;
        self.written_bytes_per_second = sample.written_bytes_per_second;
        self.uptime_seconds = sample.uptime_seconds;
        self.ready = true;
    }
}

/// Samples only the current desktop process. The worker exits naturally when
/// the receiving view is dropped and the channel disconnects.
#[cfg(not(test))]
pub(crate) fn spawn_performance_sampler(
    sender: tokio::sync::mpsc::UnboundedSender<PerformanceSample>,
) {
    let spawn = thread::Builder::new()
        .name("termi9ne-performance-monitor".to_owned())
        .spawn(move || {
            let Ok(pid) = get_current_pid() else {
                return;
            };
            let refresh = ProcessRefreshKind::new()
                .with_cpu()
                .with_memory()
                .with_disk_usage();
            let mut system = System::new();
            system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), refresh);
            let mut sampled_at = Instant::now();

            loop {
                thread::sleep(SAMPLE_INTERVAL);
                system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), refresh);
                let now = Instant::now();
                let elapsed = now.duration_since(sampled_at).as_secs_f64().max(0.001);
                sampled_at = now;
                let Some(process) = system.process(pid) else {
                    return;
                };
                let disk = process.disk_usage();
                let sample = PerformanceSample {
                    cpu_percent: process.cpu_usage(),
                    resident_bytes: process.memory(),
                    read_bytes_per_second: disk.read_bytes as f64 / elapsed,
                    written_bytes_per_second: disk.written_bytes as f64 / elapsed,
                    uptime_seconds: process.run_time(),
                };
                if sender.send(sample).is_err() {
                    return;
                }
            }
        });

    if let Err(error) = spawn {
        eprintln!("performance monitor thread failed: {error}");
    }
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    format_byte_value(bytes as f64)
}

pub(crate) fn format_rate(bytes_per_second: f64) -> String {
    format!("{}/s", format_byte_value(bytes_per_second))
}

fn format_byte_value(bytes: f64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.0} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.0} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

pub(crate) fn format_uptime(seconds: u64) -> String {
    let hours = seconds / 3_600;
    let minutes = seconds % 3_600 / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_telemetry_for_a_compact_readout() {
        assert_eq!(format_bytes(192 * 1024 * 1024), "192 MiB");
        assert_eq!(format_rate(1536.0), "2 KiB/s");
        assert_eq!(format_uptime(65), "01:05");
        assert_eq!(format_uptime(3_661), "01:01:01");
    }

    #[test]
    fn process_sample_updates_the_visible_stats() {
        let mut stats = PerformanceStats::default();
        stats.update_process(PerformanceSample {
            cpu_percent: 12.5,
            resident_bytes: 42,
            read_bytes_per_second: 7.0,
            written_bytes_per_second: 9.0,
            uptime_seconds: 11,
        });
        assert!(stats.ready);
        assert_eq!(stats.cpu_percent, 12.5);
        assert_eq!(stats.resident_bytes, 42);
    }

    #[test]
    fn display_probe_jitter_snaps_to_standard_refresh_rates() {
        assert_eq!(normalized_display_fps(58.7), 60.0);
        assert_eq!(normalized_display_fps(118.4), 120.0);
        assert_eq!(normalized_display_fps(143.2), 144.0);
    }

    #[test]
    fn peak_fps_retains_the_highest_observed_rate() {
        let mut stats = PerformanceStats::default();
        stats.update_fps(57.0);
        stats.update_fps(42.0);
        stats.update_fps(119.0);
        assert_eq!(stats.fps, 119.0);
        assert_eq!(stats.peak_fps, 119.0);
    }

    #[test]
    fn benchmark_result_is_retained_and_contributes_to_peak() {
        let mut stats = PerformanceStats::default();
        stats.update_fps(42.0);
        stats.record_benchmark(117.5);
        assert_eq!(stats.benchmark_fps, Some(117.5));
        assert_eq!(stats.peak_fps, 117.5);
    }

    #[test]
    fn render_probe_animates_and_measures_actual_draws() {
        assert_eq!(render_probe_progress(Duration::ZERO), 0.0);
        assert_eq!(render_probe_progress(Duration::from_millis(250)), 0.5);
        assert_eq!(render_probe_progress(Duration::from_millis(500)), 1.0);
        assert_eq!(render_probe_progress(Duration::from_millis(750)), 0.5);
        assert_eq!(render_probe_progress(Duration::from_secs(1)), 0.0);

        assert_eq!(
            measured_render_fps(10, 130, Duration::from_secs(1)),
            Some(120.0)
        );
        assert_eq!(measured_render_fps(10, 10, Duration::from_secs(1)), None);
        assert_eq!(measured_render_fps(10, 11, Duration::ZERO), None);
    }
}
