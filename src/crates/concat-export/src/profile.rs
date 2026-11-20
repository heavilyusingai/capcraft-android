// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! Where an export's time went.
//!
//! An export is four kinds of work one after another: setting the timeline
//! up, decoding each frame's layers, drawing them on the compositor, and
//! handing the picture to the encoder; then the mux that closes the file.
//! When a four-minute export is slower than it should be, the first
//! question is which of those it is - the phone's decoder, the GPU, the
//! encoder, or the disk - and the export itself is the only place that can
//! answer it, because only there is every frame of the cut seen.
//!
//! [`ExportProfile`] is that answer: the wall time each stage took, how
//! many frames and layers went through it, and how the cut's four quarters
//! compare. It is collected only when the caller asks for it ([`Reporter`]
//! carries the slot, `None` by default), so an export nobody is measuring
//! pays one `Option` test per stage and nothing else.
//!
//! [`Reporter`]: crate::Reporter
//!
//! Nothing here needs an Android API: the clock is `std::time`, so the same
//! profile is collected on the phone (Android 10 and later, where CapCraft
//! runs) and on the desktop.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One kind of work an export does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Reading the timeline, opening decoders, opening the compositor and
    /// the encoder: everything before the first frame.
    Setup,
    /// Reading each frame's layers out of their files.
    Decode,
    /// Drawing the decoded layers into the frame on the GPU (or on the
    /// CPU, when the device was lost).
    Composite,
    /// Handing the drawn frame to the encoder (and, on the CPU's encoder
    /// only, encoding it).
    Encode,
    /// Writing the container: the audio mix and the trailer.
    Mux,
}

impl Stage {
    /// Every stage, in the order an export does them.
    pub const ALL: [Stage; 5] = [
        Stage::Setup,
        Stage::Decode,
        Stage::Composite,
        Stage::Encode,
        Stage::Mux,
    ];

    /// The stage's name, as the profile's table and the log spell it.
    pub fn label(self) -> &'static str {
        match self {
            Stage::Setup => "setup",
            Stage::Decode => "decode",
            Stage::Composite => "composite",
            Stage::Encode => "encode",
            Stage::Mux => "mux",
        }
    }
}

/// A profile an export writes into and the caller reads: shared, because
/// the export's reporter and the thread that asked for it are different
/// threads, and an export that ends with an error still leaves what it
/// measured behind.
pub type SharedProfile = Arc<Mutex<ExportProfile>>;

/// A fresh, empty profile for a caller to hand to [`crate::Reporter`].
pub fn shared() -> SharedProfile {
    Arc::new(Mutex::new(ExportProfile::default()))
}

/// Adds `elapsed` to `stage` in `profile`, when the caller keeps one.
///
/// A poisoned lock is taken back rather than propagated: an export whose
/// reporter thread panicked has worse to show than a lost measurement.
pub fn account(profile: Option<&SharedProfile>, stage: Stage, elapsed: Duration) {
    if let Some(profile) = profile {
        profile
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .account(stage, elapsed);
    }
}

/// Runs `work`, adding what it took to `stage` in `profile`.
///
/// The loop's hot path calls this: with no profile the closure runs free of
/// any clock read.
#[inline]
pub fn timed<T>(profile: Option<&SharedProfile>, stage: Stage, work: impl FnOnce() -> T) -> T {
    let Some(profile) = profile else {
        return work();
    };
    let started = std::time::Instant::now();
    let value = work();
    let elapsed = started.elapsed();
    profile
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .account(stage, elapsed);
    value
}

/// What an export measured: the time each stage took, how much went
/// through it, and what it ran on.
#[derive(Clone, Debug, Default)]
pub struct ExportProfile {
    setup: Duration,
    decode: Duration,
    composite: Duration,
    encode: Duration,
    mux: Duration,
    frames: u64,
    layers: u64,
    losses: u64,
    redraws: u64,
    quarters: [Duration; 4],
    encoder: String,
    hardware: bool,
    width: u32,
    height: u32,
    fps: f64,
}

impl ExportProfile {
    /// Adds `elapsed` to one stage.
    pub fn account(&mut self, stage: Stage, elapsed: Duration) {
        match stage {
            Stage::Setup => self.setup += elapsed,
            Stage::Decode => self.decode += elapsed,
            Stage::Composite => self.composite += elapsed,
            Stage::Encode => self.encode += elapsed,
            Stage::Mux => self.mux += elapsed,
        }
    }

    /// What one stage took.
    pub fn stage(&self, stage: Stage) -> Duration {
        match stage {
            Stage::Setup => self.setup,
            Stage::Decode => self.decode,
            Stage::Composite => self.composite,
            Stage::Encode => self.encode,
            Stage::Mux => self.mux,
        }
    }

    /// The frames written.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The layers decoded: a cut of stills with a title over it decodes
    /// two a frame, and tells a decode-bound export from a composite-bound
    /// one.
    pub fn layers(&self) -> u64 {
        self.layers
    }

    /// How many times the GPU device was lost under the export and the
    /// frame drawn again on the next one; see `recovered` in the render.
    pub fn losses(&self) -> u64 {
        self.losses
    }

    /// Frames drawn a second time because the device died under the first
    /// drawing.
    pub fn redraws(&self) -> u64 {
        self.redraws
    }

    /// The encoder that wrote the file, by the name the platform gave it
    /// (`c2.qti.hevc.encoder`, `h264_qsv`, ...).
    pub fn encoder(&self) -> &str {
        &self.encoder
    }

    /// Whether that encoder was the platform's hardware one.
    pub fn hardware(&self) -> bool {
        self.hardware
    }

    /// What the frame size and rate were written as.
    pub fn output(&self) -> (u32, u32, f64) {
        (self.width, self.height, self.fps)
    }

    /// Notes the encoder the file is being written with.
    pub fn encoder_is(&mut self, name: impl Into<String>, hardware: bool) {
        self.encoder = name.into();
        self.hardware = hardware;
    }

    /// Notes the size and rate the file is written as.
    pub fn output_is(&mut self, width: u32, height: u32, fps: f64) {
        self.width = width;
        self.height = height;
        self.fps = fps;
    }

    /// Counts one frame: its layers, and the time it took in the quarter of
    /// the cut it belongs to.
    pub fn frame(&mut self, index: i64, total: i64, layers: u64, elapsed: Duration) {
        self.frames += 1;
        self.layers += layers;
        let quarter = if total > 0 {
            ((index.max(0) as u128 * 4) / total.max(1) as u128).min(3) as usize
        } else {
            0
        };
        self.quarters[quarter] += elapsed;
    }

    /// Counts a lost device and the frame drawn again after it.
    pub fn lost(&mut self) {
        self.losses += 1;
        self.redraws += 1;
    }

    /// The four quarters of the cut, by the time their frames took to
    /// decode, draw and encode. The first quarter of a cut that opens on
    /// the phone's own footage can be twice the last one's - the decoders
    /// warming up - and a profile that shows it says where to look.
    pub fn quarters(&self) -> [Duration; 4] {
        self.quarters
    }

    /// Decoding, drawing and encoding - everything but the setup and the
    /// mux. Dividing by [`frames`](Self::frames) gives the per-frame cost,
    /// which is what a faster export has to fall.
    pub fn rendering(&self) -> Duration {
        self.decode + self.composite + self.encode
    }

    /// Everything measured, the setup and the mux included.
    pub fn total(&self) -> Duration {
        self.setup + self.rendering() + self.mux
    }

    /// How many frames a second the export rendered, setup and mux aside.
    pub fn frames_per_second(&self) -> f64 {
        let seconds = self.rendering().as_secs_f64();
        if seconds <= 0.0 {
            0.0
        } else {
            self.frames as f64 / seconds
        }
    }

    /// One line for a log or a notification: the per-frame cost, the fastest
    /// stage's share, and the encoder.
    pub fn summary(&self) -> String {
        let frames = self.frames.max(1) as f64;
        let per_frame = self.rendering().as_secs_f64() * 1000.0 / frames;
        format!(
            "{} frames in {:.2}s ({:.2} ms a frame, {:.1} fps), encoder {} ({})",
            self.frames,
            self.total().as_secs_f64(),
            per_frame,
            self.frames_per_second(),
            if self.encoder.is_empty() {
                "unknown"
            } else {
                self.encoder.as_str()
            },
            if self.hardware {
                "hardware"
            } else {
                "software"
            },
        )
    }

    /// The profile as the table the log and the app's export report show:
    /// a line a stage, its time, its share of the export, and its cost a
    /// frame, then the quarters and what it ran on.
    pub fn table(&self) -> String {
        let total = self.total().as_secs_f64();
        let frames = self.frames.max(1) as f64;
        let mut out = String::new();
        out.push_str(&format!(
            "export profile: {}x{} at {:.3} fps\n",
            self.width, self.height, self.fps
        ));
        out.push_str("stage        time      share   per frame\n");
        for stage in Stage::ALL {
            let took = self.stage(stage).as_secs_f64();
            let share = if total > 0.0 {
                took / total * 100.0
            } else {
                0.0
            };
            let per_frame = if matches!(stage, Stage::Setup | Stage::Mux) {
                "-".to_owned()
            } else {
                format!("{:.2} ms", took * 1000.0 / frames)
            };
            out.push_str(&format!(
                "{:<10} {:>8.3}s {:>6.1}% {:>11}\n",
                stage.label(),
                took,
                share,
                per_frame
            ));
        }
        out.push_str(&format!(
            "{} frames, {} layers, {} rewound after {} device {}\n",
            self.frames,
            self.layers,
            self.redraws,
            self.losses,
            if self.losses == 1 { "loss" } else { "losses" },
        ));
        let quarters: Vec<String> = self
            .quarters
            .iter()
            .map(|quarter| format!("{:.2}s", quarter.as_secs_f64()))
            .collect();
        out.push_str(&format!("quarters: {}\n", quarters.join("  ")));
        out.push_str(&format!(
            "encoder: {} ({})\n",
            if self.encoder.is_empty() {
                "unknown"
            } else {
                self.encoder.as_str()
            },
            if self.hardware {
                "hardware"
            } else {
                "software"
            },
        ));
        out
    }
}

impl fmt::Display for ExportProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.table())
    }
}

#[cfg(test)]
mod tests {
    use super::{ExportProfile, Stage, account, shared, timed};
    use std::time::Duration;

    fn millis(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Each stage keeps its own time, and the rendering total is the three
    /// that draw and write the frames.
    #[test]
    fn stages_add_up() {
        let mut profile = ExportProfile::default();
        profile.account(Stage::Setup, millis(500));
        profile.account(Stage::Decode, millis(2000));
        profile.account(Stage::Composite, millis(1000));
        profile.account(Stage::Encode, millis(500));
        profile.account(Stage::Mux, millis(100));
        assert_eq!(profile.stage(Stage::Decode), millis(2000));
        assert_eq!(profile.rendering(), millis(3500));
        assert_eq!(profile.total(), millis(4100));
    }

    /// A frame is counted in the quarter of the cut it belongs to, and the
    /// last frame of the cut is in the last quarter rather than past it.
    #[test]
    fn quarters_hold_every_frame() {
        let mut profile = ExportProfile::default();
        for index in 0..400 {
            profile.frame(index, 400, 1, millis(10));
        }
        assert_eq!(profile.frames(), 400);
        assert_eq!(profile.layers(), 400);
        assert_eq!(profile.quarters(), [millis(1000); 4]);
        let mut last = ExportProfile::default();
        last.frame(399, 400, 2, millis(10));
        assert_eq!(last.quarters()[3], millis(10));
        assert_eq!(last.layers(), 2);
    }

    /// The shared slot the reporter carries is added to, and a caller who
    /// keeps no profile pays nothing for the clock.
    #[test]
    fn the_shared_slot_is_measured_through() {
        let profile = shared();
        // The work the clock is around takes a moment, and the moment is
        // what the stage keeps.
        timed(Some(&profile), Stage::Composite, || {
            std::thread::sleep(millis(5))
        });
        account(Some(&profile), Stage::Encode, millis(2));
        let read = profile
            .lock()
            .map(|profile| profile.clone())
            .unwrap_or_default();
        assert!(read.stage(Stage::Composite) >= millis(5));
        assert_eq!(read.stage(Stage::Encode), millis(2));
        // With no profile kept, the work runs and nothing is measured.
        assert_eq!(timed(None, Stage::Decode, || 7), 7);
    }

    /// The table names every stage, the frames, and the encoder.
    #[test]
    fn the_table_says_where_the_time_went() {
        let mut profile = ExportProfile::default();
        profile.output_is(3840, 2160, 30.0);
        profile.encoder_is("c2.qti.hevc.encoder", true);
        profile.account(Stage::Decode, millis(120_000));
        profile.account(Stage::Composite, millis(60_000));
        profile.account(Stage::Encode, millis(70_000));
        for index in 0..300 {
            profile.frame(index, 300, 1, millis(833));
        }
        let table = profile.table();
        for stage in Stage::ALL {
            assert!(table.contains(stage.label()), "{table}");
        }
        assert!(table.contains("3840x2160 at 30.000 fps"), "{table}");
        assert!(table.contains("c2.qti.hevc.encoder (hardware)"), "{table}");
        assert!(table.contains("300 frames, 300 layers"), "{table}");
        assert!(table.contains("quarters:"), "{table}");
        // Decoding is the largest share of this export, and the per-frame
        // cost is the rendering time over the frames written.
        assert!(profile.frames_per_second() > 0.0);
        assert!(
            profile.summary().contains("hardware"),
            "{}",
            profile.summary()
        );
    }
}
