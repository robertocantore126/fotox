//! Frame statistics shared by the render thread (writer) and the engine
//! thread (reader, for the ~2 Hz `status` message) — M1-T11.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The window the frame statistics cover.
const WINDOW: Duration = Duration::from_secs(2);

/// What the render thread reports.
#[derive(Debug, Default)]
pub struct RenderStats {
	/// When each frame of the last [`WINDOW`] finished, and how long it took (ms).
	frames: VecDeque<(Instant, f32)>,
	/// Source tiles uploaded by the last frame.
	pub uploads: u32,
	/// Tiles being loaded from warm/cold storage.
	pub pending_loads: u32,
	/// VRAM the compositor holds (atlas + composite cache), in bytes.
	pub gpu_bytes: u64,
	/// Input → pixels latency of the frames that showed stroke pixels
	/// (M5-T11): when, and how long (ms).
	inputs: VecDeque<(Instant, f32)>,
}

/// A summary of the last [`WINDOW`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameSummary {
	pub fps: f32,
	pub p50_ms: f32,
	pub p99_ms: f32,
}

impl RenderStats {
	/// Record a frame that took `ms` and finished at `now`.
	pub fn record(&mut self, now: Instant, ms: f32) {
		self.frames.push_back((now, ms));
		self.prune(now);
	}

	/// Record the input → pixels latency of a frame that showed stroke pixels.
	pub fn record_input(&mut self, now: Instant, ms: f32) {
		self.inputs.push_back((now, ms));
		while self.inputs.front().is_some_and(|&(t, _)| now.saturating_duration_since(t) > WINDOW) {
			self.inputs.pop_front();
		}
	}

	/// The input latency's median and 99th percentile over the last 2 s
	/// (0 when nothing was painted).
	pub fn input_latency(&mut self, now: Instant) -> (f32, f32) {
		while self.inputs.front().is_some_and(|&(t, _)| now.saturating_duration_since(t) > WINDOW) {
			self.inputs.pop_front();
		}
		if self.inputs.is_empty() {
			return (0.0, 0.0);
		}
		let mut times: Vec<f32> = self.inputs.iter().map(|&(_, ms)| ms).collect();
		times.sort_by(f32::total_cmp);
		let at = |q: f32| times[((times.len() - 1) as f32 * q).round() as usize];
		(at(0.5), at(0.99))
	}

	/// Frames per second and frame-time percentiles over the last 2 s.
	pub fn summary(&mut self, now: Instant) -> FrameSummary {
		self.prune(now);
		if self.frames.is_empty() {
			return FrameSummary::default();
		}
		let mut times: Vec<f32> = self.frames.iter().map(|&(_, ms)| ms).collect();
		times.sort_by(f32::total_cmp);
		let at = |q: f32| times[((times.len() - 1) as f32 * q).round() as usize];
		FrameSummary {
			fps: self.frames.len() as f32 / WINDOW.as_secs_f32(),
			p50_ms: at(0.5),
			p99_ms: at(0.99),
		}
	}

	fn prune(&mut self, now: Instant) {
		while self.frames.front().is_some_and(|&(t, _)| now.saturating_duration_since(t) > WINDOW) {
			self.frames.pop_front();
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn summary_covers_the_last_two_seconds() {
		let mut stats = RenderStats::default();
		let t0 = Instant::now();
		for i in 0..120u32 {
			// 60 fps for 2 s, frame times 1..=120 ms
			stats.record(t0 + Duration::from_millis(u64::from(i) * 1000 / 60), (i + 1) as f32);
		}
		let s = stats.summary(t0 + Duration::from_millis(1990));
		assert_eq!(s.fps, 60.0);
		assert_eq!(s.p50_ms, 61.0);
		assert_eq!(s.p99_ms, 119.0);
		// Idle for a while: nothing left.
		assert_eq!(stats.summary(t0 + Duration::from_secs(10)), FrameSummary::default());
	}
}
