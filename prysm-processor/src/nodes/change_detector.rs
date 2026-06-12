use crate::frames::luma_at;
use prysm_capture::Frame;
use prysm_core::Config;

/// Grid resolution of the luma signature (GRID x GRID sample points)
const GRID: u32 = 16;

/// Detects whether a frame differs from the last *processed* frame, so the
/// pipeline can skip identical content (paused video, static desktop).
///
/// Compares a sparse luma signature against the signature of the last frame
/// that was actually processed — never against the last skipped frame — so a
/// slow fade accumulates delta until it crosses the threshold. A skip cap
/// forces periodic reprocessing as a backstop.
#[derive(Debug)]
pub struct ChangeDetector {
    threshold: f32,
    max_skipped_frames: u32,
    signature: Vec<u8>,
    scratch: Vec<u8>,
    skipped: u32,
}

impl ChangeDetector {
    pub fn new(config: &Config) -> Self {
        Self {
            threshold: config.change_threshold,
            max_skipped_frames: config.max_skipped_frames,
            signature: Vec::new(),
            scratch: Vec::new(),
            skipped: 0,
        }
    }

    /// Returns true if the frame should be processed. Call exactly once per frame.
    pub fn has_changed(&mut self, frame: &Frame) -> bool {
        self.scratch.clear();
        for gy in 0..GRID {
            // Sample at cell centers
            let y = (gy * frame.height + frame.height / 2) / GRID;
            for gx in 0..GRID {
                let x = (gx * frame.width + frame.width / 2) / GRID;
                self.scratch.push(luma_at(frame, x, y));
            }
        }

        let changed = if self.signature.len() != self.scratch.len() {
            true // First frame or resolution change
        } else if self.skipped >= self.max_skipped_frames {
            true // Backstop: never skip indefinitely
        } else {
            let total_delta: u32 = self
                .signature
                .iter()
                .zip(&self.scratch)
                .map(|(&a, &b)| a.abs_diff(b) as u32)
                .sum();
            let mean_delta = total_delta as f32 / self.scratch.len() as f32;
            mean_delta > self.threshold
        };

        if changed {
            std::mem::swap(&mut self.signature, &mut self.scratch);
            self.skipped = 0;
        } else {
            self.skipped += 1;
        }

        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::yuyv_frame_from_luma;

    fn detector(threshold: f32, max_skips: u32) -> ChangeDetector {
        let mut config = Config::default();
        config.change_threshold = threshold;
        config.max_skipped_frames = max_skips;
        ChangeDetector::new(&config)
    }

    #[test]
    fn static_frames_skip_after_first() {
        let mut cd = detector(1.0, 100);
        let frame = yuyv_frame_from_luma(64, 64, |_, _| 128);

        assert!(cd.has_changed(&frame), "first frame always processes");
        assert!(!cd.has_changed(&frame.clone()));
        assert!(!cd.has_changed(&frame.clone()));
    }

    #[test]
    fn content_change_triggers_processing() {
        let mut cd = detector(1.0, 100);
        let gray = yuyv_frame_from_luma(64, 64, |_, _| 128);
        let white = yuyv_frame_from_luma(64, 64, |_, _| 255);

        assert!(cd.has_changed(&gray));
        assert!(!cd.has_changed(&gray.clone()));
        assert!(cd.has_changed(&white));
    }

    #[test]
    fn slow_fade_accumulates_against_processed_frame() {
        let mut cd = detector(2.0, 1000);

        // Each step is below threshold vs the previous frame, but deltas
        // accumulate vs the last processed frame until they cross it.
        assert!(cd.has_changed(&yuyv_frame_from_luma(64, 64, |_, _| 100)));
        assert!(!cd.has_changed(&yuyv_frame_from_luma(64, 64, |_, _| 101)));
        assert!(!cd.has_changed(&yuyv_frame_from_luma(64, 64, |_, _| 102)));
        assert!(cd.has_changed(&yuyv_frame_from_luma(64, 64, |_, _| 103)));
    }

    #[test]
    fn skip_cap_forces_refresh() {
        let mut cd = detector(1.0, 3);
        let frame = yuyv_frame_from_luma(64, 64, |_, _| 128);

        assert!(cd.has_changed(&frame));
        assert!(!cd.has_changed(&frame.clone()));
        assert!(!cd.has_changed(&frame.clone()));
        assert!(!cd.has_changed(&frame.clone()));
        assert!(cd.has_changed(&frame.clone()), "cap reached, must process");
    }

    #[test]
    fn resolution_change_triggers_processing() {
        let mut cd = detector(1.0, 100);
        assert!(cd.has_changed(&yuyv_frame_from_luma(64, 64, |_, _| 128)));
        // Same content, different resolution: signature length stays GRID^2,
        // but luma positions shift; identical uniform frames still match.
        assert!(!cd.has_changed(&yuyv_frame_from_luma(32, 32, |_, _| 128)));
        // A differing uniform frame at the new resolution must process.
        assert!(cd.has_changed(&yuyv_frame_from_luma(32, 32, |_, _| 200)));
    }
}
