//! The frames every backend's stream delivers: [`Frame`], the
//! [`FrameRing`] of the newest few, and the [`FrameReader`] that hands
//! them to the recording session (the frame before an event, the frame
//! some time after it), across the runs a pause and resume start.
//!
//! A backend's stream copies each captured image into a tightly packed
//! BGRA buffer (from [`FrameReader::take_buffer`], so evicted frames'
//! buffers are reused), stamps it on the event clock and pushes it with
//! [`FrameReader::push_from`] for its run.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use guiderecorder_core::events::Timestamp;

use crate::frames::FrameGeometry;

/// Spare pixel buffers kept for reuse.
const POOL_MAX: usize = 2;

/// One captured frame: tightly packed BGRA, 4 bytes per pixel, rows top to
/// bottom, `width * 4` bytes per row, no padding.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Presentation time, host monotonic ns (same clock as events).
    pub ts: Timestamp,
    pub width: u32,
    pub height: u32,
    /// Maps global points to this frame's pixels.
    pub geometry: FrameGeometry,
    pub pixels: Arc<Vec<u8>>,
}

impl Frame {
    pub fn bytes_per_row(&self) -> usize {
        self.width as usize * 4
    }

    /// Pixels per point (2.0 on Retina).
    pub fn scale(&self) -> f64 {
        self.geometry.scale()
    }

    /// BGRA of pixel (x, y), if in bounds.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = y as usize * self.bytes_per_row() + x as usize * 4;
        self.pixels.get(i..i + 4)?.try_into().ok()
    }
}

/// Fixed-capacity buffer of the newest frames, ordered by timestamp.
/// Pure selection logic; [`FrameReader`] adds locking and waiting.
#[derive(Debug)]
pub struct FrameRing {
    frames: VecDeque<Frame>,
    capacity: usize,
}

impl FrameRing {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            frames: VecDeque::with_capacity(capacity + 1),
            capacity,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Adds a frame, dropping the oldest if full, and returns the frames
    /// it dropped. Out-of-order frames are inserted in timestamp order.
    pub fn push(&mut self, frame: Frame) -> Vec<Frame> {
        let at = self.frames.partition_point(|f| f.ts <= frame.ts);
        self.frames.insert(at, frame);
        let excess = self.frames.len().saturating_sub(self.capacity);
        self.frames.drain(..excess).collect()
    }

    /// Removes and returns all frames.
    pub fn clear(&mut self) -> Vec<Frame> {
        self.frames.drain(..).collect()
    }

    pub fn latest(&self) -> Option<&Frame> {
        self.frames.back()
    }

    /// Newest frame strictly before `ts`.
    pub fn before(&self, ts: Timestamp) -> Option<&Frame> {
        self.frames.iter().rev().find(|f| f.ts < ts)
    }

    /// Oldest frame at or after `ts`.
    pub fn at_or_after(&self, ts: Timestamp) -> Option<&Frame> {
        self.frames.iter().find(|f| f.ts >= ts)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Frame> {
        self.frames.iter()
    }
}

#[derive(Debug)]
struct ReaderState {
    ring: FrameRing,
    /// False while paused or stopped: no frames will arrive, so waiters
    /// return immediately instead of sleeping until their timeout.
    active: bool,
    /// Identifies the current capture run. Frames and stop notifications
    /// from an earlier run (still queued on its dispatch queue) are ignored.
    generation: u64,
    /// The current run was stopped by the system (display gone, screen
    /// locked, permission revoked, "Stop sharing").
    stopped_by_system: bool,
    /// When the current run was stopped by the system (event clock). No
    /// frame from this run is used for a later time: the screen may have
    /// changed (the window closed) without a frame to show it.
    stopped_at: Option<Timestamp>,
    last_error: Option<String>,
}

/// Called with the error and the stop time when the system stops the
/// stream.
type StopHook = Arc<dyn Fn(&str, Timestamp) + Send + Sync>;

struct Shared {
    state: Mutex<ReaderState>,
    arrived: Condvar,
    /// Spare pixel buffers; `None` when reuse is off.
    pool: Option<Mutex<Vec<Vec<u8>>>>,
    /// See [`FrameReader::on_system_stop`].
    on_stop: Mutex<Option<StopHook>>,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, ReaderState> {
        // A panic while holding the lock cannot leave the ring inconsistent.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_pool(&self) -> Option<MutexGuard<'_, Vec<Vec<u8>>>> {
        self.pool
            .as_ref()
            .map(|p| p.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Cheap, cloneable, thread-safe handle for reading frames. Can outlive or
/// be used concurrently with the [`FrameStream`].
#[derive(Debug, Clone)]
pub struct FrameReader {
    shared: Arc<Shared>,
}

impl FrameReader {
    /// A reader not attached to any stream, fed with [`FrameReader::push`].
    /// Used by tests and by tools that replay frames.
    pub fn detached(capacity: usize) -> Self {
        Self::with_pool(capacity, true)
    }

    pub(crate) fn with_pool(capacity: usize, reuse_buffers: bool) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(ReaderState {
                    ring: FrameRing::new(capacity),
                    active: true,
                    generation: 0,
                    stopped_by_system: false,
                    stopped_at: None,
                    last_error: None,
                }),
                arrived: Condvar::new(),
                pool: reuse_buffers.then(|| Mutex::new(Vec::new())),
                on_stop: Mutex::new(None),
            }),
        }
    }

    /// Adds a frame and wakes [`FrameReader::frame_after`] waiters.
    pub fn push(&self, frame: Frame) {
        let generation = self.shared.lock().generation;
        self.push_from(generation, frame);
    }

    /// Adds a frame captured by run `generation`; dropped if that run is
    /// over.
    pub(crate) fn push_from(&self, generation: u64, frame: Frame) {
        let evicted = {
            let mut state = self.shared.lock();
            if state.generation != generation {
                vec![frame]
            } else {
                state.ring.push(frame)
            }
        };
        self.shared.arrived.notify_all();
        self.recycle(evicted);
    }

    /// Returns uniquely owned pixel buffers to the pool.
    fn recycle(&self, frames: Vec<Frame>) {
        let Some(mut pool) = self.shared.lock_pool() else {
            return;
        };
        for f in frames {
            if pool.len() >= POOL_MAX {
                break;
            }
            if let Ok(buf) = Arc::try_unwrap(f.pixels) {
                pool.push(buf);
            }
        }
    }

    /// An empty buffer with room for `len` bytes, reused if possible.
    pub(crate) fn take_buffer(&self, len: usize) -> Vec<u8> {
        let reused = self.shared.lock_pool().and_then(|mut pool| {
            let i = pool.iter().position(|b| b.capacity() >= len)?;
            Some(pool.swap_remove(i))
        });
        match reused {
            Some(mut buf) => {
                buf.clear();
                buf
            }
            None => Vec::with_capacity(len),
        }
    }

    /// Starts a new capture run: clears the ring (the screen may have
    /// changed since the last run) and returns the run's generation.
    pub(crate) fn begin_run(&self) -> u64 {
        let (generation, old) = {
            let mut state = self.shared.lock();
            state.generation += 1;
            state.active = true;
            state.stopped_by_system = false;
            state.stopped_at = None;
            (state.generation, state.ring.clear())
        };
        self.recycle(old);
        generation
    }

    /// Ends the current run. Buffered frames stay readable; frames still
    /// queued by the old stream are dropped. Frees the buffer pool.
    pub(crate) fn end_run(&self) {
        {
            let mut state = self.shared.lock();
            state.generation += 1;
            state.active = false;
        }
        self.shared.arrived.notify_all();
        if let Some(mut pool) = self.shared.lock_pool() {
            pool.clear();
        }
    }

    /// The system stopped run `generation`, now.
    pub(crate) fn system_stopped(&self, generation: u64, error: String) {
        self.system_stopped_at(generation, error, crate::backend::event_tap::now());
    }

    /// The system stopped run `generation` at `at` (event clock). Calls
    /// the [`FrameReader::on_system_stop`] hook, outside the lock.
    pub(crate) fn system_stopped_at(&self, generation: u64, error: String, at: Timestamp) {
        {
            let mut state = self.shared.lock();
            if state.generation != generation || state.stopped_by_system {
                return;
            }
            state.stopped_by_system = true;
            state.stopped_at = Some(at);
            state.active = false;
            state.last_error = Some(error.clone());
        }
        self.shared.arrived.notify_all();
        let hook = self
            .shared
            .on_stop
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(hook) = hook {
            hook(&error, at);
        }
    }

    /// The current run's generation, for [`FrameReader::system_stopped_at`]
    /// in tests.
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.shared.lock().generation
    }

    /// Calls `f` with the error and the stop time, once per run, when the
    /// system stops the
    /// stream (window closed, display unplugged, permission revoked). It
    /// runs on ScreenCaptureKit's queue: keep it quick. Replaces any
    /// earlier hook.
    pub fn on_system_stop(&self, f: impl Fn(&str, Timestamp) + Send + Sync + 'static) {
        *self
            .shared
            .on_stop
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
    }

    /// When the system stopped the current run (event clock); `None` while
    /// it runs, is paused or was stopped by us.
    pub fn stopped_at(&self) -> Option<Timestamp> {
        self.shared.lock().stopped_at
    }

    pub(crate) fn stopped_by_system(&self) -> bool {
        self.shared.lock().stopped_by_system
    }

    pub(crate) fn clear_error(&self) {
        self.shared.lock().last_error = None;
    }

    /// Newest frame taken strictly before `ts` (the pre-click frame).
    /// `None` for a `ts` at or after the time the system stopped the
    /// stream ([`FrameReader::stopped_at`]): the last frame may show a
    /// window that has closed since.
    pub fn frame_before(&self, ts: Timestamp) -> Option<Frame> {
        let state = self.shared.lock();
        if state.stopped_at.is_some_and(|at| ts >= at) {
            return None;
        }
        state.ring.before(ts).cloned()
    }

    /// First frame taken at least `min_delay_ms` after `ts` (the "after"
    /// frame). Waits up to `timeout` for one to arrive.
    ///
    /// ScreenCaptureKit sends no frames while the screen is static, so a
    /// click that changes nothing produces no later frame. On timeout, or
    /// immediately if the stream is paused or stopped, this therefore
    /// falls back to the latest frame if it was taken at or after `ts`
    /// (ignoring `min_delay_ms`): the screen has not changed since then.
    /// Returns `None` only if there is no frame at or after `ts` at all.
    pub fn frame_after(
        &self,
        ts: Timestamp,
        min_delay_ms: u64,
        timeout: Duration,
    ) -> Option<Frame> {
        let target = Timestamp(ts.0.saturating_add(min_delay_ms.saturating_mul(1_000_000)));
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock();
        loop {
            if let Some(f) = state.ring.at_or_after(target) {
                return Some(f.clone());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if !state.active || left.is_zero() {
                if state.stopped_at.is_some_and(|at| ts >= at) {
                    return None;
                }
                return state.ring.latest().filter(|f| f.ts >= ts).cloned();
            }
            state = self
                .shared
                .arrived
                .wait_timeout(state, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    pub fn latest(&self) -> Option<Frame> {
        self.shared.lock().ring.latest().cloned()
    }

    /// Number of frames currently buffered.
    pub fn len(&self) -> usize {
        self.shared.lock().ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The error with which ScreenCaptureKit last stopped the stream (e.g.
    /// the display disappeared). Cleared by a successful
    /// [`FrameStream::resume`].
    pub fn last_error(&self) -> Option<String> {
        self.shared.lock().last_error.clone()
    }
}

#[cfg(test)]
mod tests {
    use guiderecorder_core::model::Rect;

    use super::*;

    fn fake(ts: u64) -> Frame {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        };
        Frame {
            ts: Timestamp(ts),
            width: 2,
            height: 2,
            geometry: FrameGeometry::from_rect(rect, 2, 2),
            pixels: Arc::new((0..16).collect()),
        }
    }

    fn ts_of(f: Option<Frame>) -> Option<u64> {
        f.map(|f| f.ts.0)
    }

    #[test]
    fn ring_evicts_oldest() {
        let mut ring = FrameRing::new(3);
        for ts in [10, 20, 30] {
            assert!(ring.push(fake(ts)).is_empty());
        }
        let evicted = ring.push(fake(40));
        assert_eq!(evicted.iter().map(|f| f.ts.0).collect::<Vec<_>>(), [10]);
        let kept: Vec<u64> = ring.iter().map(|f| f.ts.0).collect();
        assert_eq!(kept, [20, 30, 40]);
        assert_eq!(ring.latest().map(|f| f.ts.0), Some(40));
    }

    #[test]
    fn ring_capacity_at_least_one() {
        let mut ring = FrameRing::new(0);
        ring.push(fake(1));
        ring.push(fake(2));
        assert_eq!(ring.capacity(), 1);
        assert_eq!(ring.len(), 1);
        assert_eq!(ring.latest().map(|f| f.ts.0), Some(2));
    }

    #[test]
    fn ring_orders_out_of_order_frames() {
        let mut ring = FrameRing::new(3);
        ring.push(fake(30));
        ring.push(fake(10));
        ring.push(fake(20));
        let kept: Vec<u64> = ring.iter().map(|f| f.ts.0).collect();
        assert_eq!(kept, [10, 20, 30]);
        // A late frame older than everything is evicted straight away.
        let evicted = ring.push(fake(5));
        assert_eq!(evicted[0].ts.0, 5);
        let kept: Vec<u64> = ring.iter().map(|f| f.ts.0).collect();
        assert_eq!(kept, [10, 20, 30]);
    }

    #[test]
    fn before_is_strict_and_newest() {
        let mut ring = FrameRing::new(3);
        for ts in [10, 20, 30] {
            ring.push(fake(ts));
        }
        assert_eq!(ring.before(Timestamp(25)).map(|f| f.ts.0), Some(20));
        assert_eq!(ring.before(Timestamp(30)).map(|f| f.ts.0), Some(20));
        assert_eq!(ring.before(Timestamp(31)).map(|f| f.ts.0), Some(30));
        assert_eq!(ring.before(Timestamp(10)).map(|f| f.ts.0), None);
    }

    #[test]
    fn at_or_after_is_oldest_match() {
        let mut ring = FrameRing::new(3);
        for ts in [10, 20, 30] {
            ring.push(fake(ts));
        }
        assert_eq!(ring.at_or_after(Timestamp(20)).map(|f| f.ts.0), Some(20));
        assert_eq!(ring.at_or_after(Timestamp(21)).map(|f| f.ts.0), Some(30));
        assert_eq!(ring.at_or_after(Timestamp(31)).map(|f| f.ts.0), None);
    }

    #[test]
    fn reader_frame_after_applies_min_delay() {
        let reader = FrameReader::detached(3);
        let ms = 1_000_000;
        for ts in [100 * ms, 300 * ms, 600 * ms] {
            reader.push(fake(ts));
        }
        let click = Timestamp(100 * ms);
        let got = reader.frame_after(click, 400, Duration::ZERO);
        assert_eq!(ts_of(got), Some(600 * ms));
        let got = reader.frame_after(click, 0, Duration::ZERO);
        assert_eq!(ts_of(got), Some(100 * ms));
    }

    #[test]
    fn reader_frame_after_falls_back_to_latest() {
        let reader = FrameReader::detached(3);
        let ms = 1_000_000;
        reader.push(fake(50 * ms));
        reader.push(fake(150 * ms));
        // Nothing 600 ms after the click; the screen has been static since
        // the 150 ms frame, which is after the click.
        let got = reader.frame_after(Timestamp(100 * ms), 600, Duration::ZERO);
        assert_eq!(ts_of(got), Some(150 * ms));
        // No frame at or after the click at all.
        assert!(reader
            .frame_after(Timestamp(200 * ms), 0, Duration::ZERO)
            .is_none());
    }

    #[test]
    fn reader_frame_after_waits_for_new_frame() {
        let reader = FrameReader::detached(3);
        reader.push(fake(10));
        let producer = reader.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            producer.push(fake(2_000_000));
        });
        let got = reader.frame_after(Timestamp(0), 1, Duration::from_secs(5));
        t.join().unwrap();
        assert_eq!(ts_of(got), Some(2_000_000));
    }

    #[test]
    fn reader_frame_after_times_out() {
        let reader = FrameReader::detached(3);
        reader.push(fake(10));
        let start = Instant::now();
        assert!(reader
            .frame_after(Timestamp(11), 1, Duration::from_millis(20))
            .is_none());
        assert!(start.elapsed() >= Duration::from_millis(20));
    }

    #[test]
    fn reader_inactive_returns_immediately() {
        let reader = FrameReader::detached(3);
        reader.end_run();
        let start = Instant::now();
        assert!(reader
            .frame_after(Timestamp(0), 0, Duration::from_secs(5))
            .is_none());
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn reader_frame_before() {
        let reader = FrameReader::detached(3);
        assert!(reader.frame_before(Timestamp(5)).is_none());
        reader.push(fake(1));
        reader.push(fake(4));
        assert_eq!(ts_of(reader.frame_before(Timestamp(5))), Some(4));
        assert_eq!(reader.len(), 2);
    }

    #[test]
    fn stale_run_frames_are_dropped() {
        let reader = FrameReader::detached(3);
        let old = reader.begin_run();
        reader.push_from(old, fake(1));
        reader.end_run();
        // Pause keeps buffered frames, but drops late ones from the old run.
        reader.push_from(old, fake(2));
        assert_eq!(ts_of(reader.latest()), Some(1));
        // Resume clears the ring; old-run frames still do not get in.
        let new = reader.begin_run();
        assert!(reader.is_empty());
        reader.push_from(old, fake(3));
        assert!(reader.is_empty());
        reader.push_from(new, fake(4));
        assert_eq!(ts_of(reader.latest()), Some(4));
    }

    #[test]
    fn system_stop_only_affects_its_own_run() {
        let reader = FrameReader::detached(3);
        let old = reader.begin_run();
        let new = reader.begin_run();
        reader.system_stopped(old, "stale".into());
        assert!(!reader.stopped_by_system());
        assert!(reader.last_error().is_none());
        reader.system_stopped(new, "display gone".into());
        assert!(reader.stopped_by_system());
        assert_eq!(reader.last_error().as_deref(), Some("display gone"));
        reader.begin_run();
        assert!(!reader.stopped_by_system());
        reader.clear_error();
        assert!(reader.last_error().is_none());
    }

    #[test]
    fn no_frame_after_a_system_stop() {
        let reader = FrameReader::detached(4);
        let run = reader.begin_run();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let hook_seen = Arc::clone(&seen);
        reader.on_system_stop(move |e, at| hook_seen.lock().unwrap().push((e.to_owned(), at)));
        reader.push_from(run, fake(10));
        reader.push_from(run, fake(20));
        reader.system_stopped_at(run, "window gone".into(), Timestamp(30));
        // A second notification for the same run is ignored.
        reader.system_stopped_at(run, "again".into(), Timestamp(40));
        assert_eq!(
            *seen.lock().unwrap(),
            [("window gone".to_owned(), Timestamp(30))]
        );
        assert_eq!(reader.stopped_at(), Some(Timestamp(30)));
        // Before the stop: the frames still serve.
        assert_eq!(ts_of(reader.frame_before(Timestamp(25))), Some(20));
        assert_eq!(ts_of(reader.frame_before(Timestamp(15))), Some(10));
        // At or after it: nothing, however old the last frame is.
        assert!(reader.frame_before(Timestamp(30)).is_none());
        assert!(reader.frame_before(Timestamp(1_000)).is_none());
        assert!(reader
            .frame_after(Timestamp(30), 0, Duration::from_secs(5))
            .is_none());
        assert_eq!(
            ts_of(reader.frame_after(Timestamp(15), 0, Duration::from_secs(5))),
            Some(20)
        );
        // A new run lifts the cutoff.
        let next = reader.begin_run();
        assert!(reader.stopped_at().is_none());
        reader.push_from(next, fake(50));
        assert_eq!(ts_of(reader.frame_before(Timestamp(60))), Some(50));
    }

    #[test]
    fn stale_run_stop_has_no_cutoff() {
        let reader = FrameReader::detached(4);
        let old = reader.begin_run();
        let run = reader.begin_run();
        reader.push_from(run, fake(10));
        reader.system_stopped_at(old, "old".into(), Timestamp(5));
        assert!(reader.stopped_at().is_none());
        assert_eq!(ts_of(reader.frame_before(Timestamp(100))), Some(10));
    }

    #[test]
    fn evicted_buffers_are_reused() {
        let reader = FrameReader::detached(1);
        reader.push(fake(1));
        let evicted_ptr = reader.latest().unwrap().pixels.as_ptr();
        reader.push(fake(2)); // evicts frame 1, whose buffer is unique
        let buf = reader.take_buffer(16);
        assert_eq!(buf.as_ptr(), evicted_ptr);
        assert!(buf.is_empty());
        // A frame someone still holds is not recycled.
        let held = reader.latest().unwrap();
        reader.push(fake(3));
        let fresh = reader.take_buffer(16);
        assert_ne!(fresh.as_ptr(), held.pixels.as_ptr());
    }

    #[test]
    fn pool_can_be_disabled() {
        let reader = FrameReader::with_pool(1, false);
        reader.push(fake(1));
        reader.push(fake(2));
        assert!(reader.shared.lock_pool().is_none());
        assert_eq!(reader.take_buffer(16).capacity(), 16);
    }

    #[test]
    fn frame_pixel_access() {
        let f = fake(0);
        assert_eq!(f.bytes_per_row(), 8);
        assert_eq!(f.pixel(1, 1), Some([12, 13, 14, 15]));
        assert_eq!(f.pixel(2, 0), None);
    }
}
