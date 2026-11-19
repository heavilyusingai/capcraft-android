// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! What an export will cost before it is started: how big the file will be,
//! how long it will take, and how it knows.
//!
//! Two questions every export sheet should answer and almost none does:
//! *will this fit on the phone*, and *how long am I waiting*. The first is
//! arithmetic on the bitrate. The second is arithmetic on a measurement, and
//! the measurement already exists - the engine profiles every export it runs
//! (see [`crate::profile`]) - so the second export of a project can say what
//! it will take from what the first one took.
//!
//! The size is a steady state: a rate-controlled export is the bitrate times
//! the seconds, and a quality-controlled one is that bitrate's equivalent,
//! carried by the rule the export sheet already uses to suggest one. Both
//! are within about ten percent of the file for ordinary footage, and can be
//! further off for a cut of stills - which is what [`Estimate::measured`]
//! and [`Estimate::line`] are for. An estimate that says how it was made can
//! be argued with; a bare number cannot.
//!
//! The time, when nothing has been measured on this machine, is a rule of
//! thumb from the pixels and frames an export pushes: deliberately
//! conservative, and replaced by a real number as soon as one exists.

use crate::ExportRequest;
use crate::profile::ExportProfile;
use concat_media::VideoCodec;

/// What an export is likely to cost.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    /// Frames the cut will render.
    pub frames: i64,
    /// Seconds of timeline, which is the cut's length.
    pub seconds: f64,
    /// The file's size in bytes, near enough to plan a phone's storage by.
    pub bytes: u64,
    /// How long the render is likely to take, in seconds of wall time.
    pub render_seconds: f64,
    /// Where the time came from: an export measured on this machine, or the
    /// rule of thumb.
    pub measured: bool,
}

impl Estimate {
    /// The size for a person: "1.2 GB", "210 MB".
    pub fn size_label(&self) -> String {
        const MB: f64 = 1_000_000.0;
        const GB: f64 = 1_000_000_000.0;
        let bytes = self.bytes as f64;
        if bytes >= GB {
            format!("{:.1} GB", bytes / GB)
        } else {
            format!("{:.0} MB", (bytes / MB).max(1.0))
        }
    }

    /// The time for a person, as a clock: "4:10", "0:38", "1:02:10".
    pub fn time_label(&self) -> String {
        let total = self.render_seconds.max(0.0).round() as u64;
        let (hours, minutes, seconds) = (total / 3600, (total / 60) % 60, total % 60);
        if hours > 0 {
            format!("{hours}:{minutes:02}:{seconds:02}")
        } else {
            format!("{minutes}:{seconds:02}")
        }
    }

    /// One line for the sheet: "≈ 210 MB · ≈ 4:10 (measured)".
    pub fn line(&self) -> String {
        let how = if self.measured {
            "measured"
        } else {
            "estimated"
        };
        format!("≈ {} · ≈ {} ({how})", self.size_label(), self.time_label())
    }
}

/// The part of an export that decides what it costs, taken out of the
/// request so the arithmetic can be reasoned about - and tested - without a
/// timeline behind it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
    /// Frames a second.
    pub fps: f64,
    /// Seconds of timeline the cut covers.
    pub seconds: f64,
    /// The CRF the export asks for, when it is steered by quality.
    pub crf: u8,
    /// The bitrate it asks for, when it is steered by rate; zero otherwise.
    pub bitrate_kbps: u32,
    /// What it encodes to.
    pub codec: VideoCodec,
    /// Ten bits a channel rather than eight.
    pub ten_bit: bool,
    /// Whether the cut carries sound, which is another track in the file.
    pub sound: bool,
}

impl Shape {
    /// The shape of `request`: its frame, its rate, its quality, and the
    /// length of its cut - the last frame any clip covers.
    pub fn of(request: &ExportRequest) -> Self {
        let fps = if request.rate_den > 0 {
            request.rate_num as f64 / request.rate_den as f64
        } else {
            30.0
        };
        let seconds = request
            .clips
            .iter()
            .map(|clip| clip.start + clip.duration.max(0.0))
            .fold(0.0f64, f64::max)
            .max(0.0);
        // A layer contributes no sound of its own; anything else unmuted
        // does, and that is what decides whether the file has an audio
        // track at all.
        let sound = request
            .clips
            .iter()
            .any(|clip| !clip.muted && clip.kind != crate::ClipKind::Layer);
        Self {
            width: request.width,
            height: request.height,
            fps,
            seconds,
            crf: request.crf,
            bitrate_kbps: request.bitrate_kbps,
            codec: request.codec,
            ten_bit: request.ten_bit,
            sound,
        }
    }

    /// The frames the cut will render.
    pub fn frames(&self) -> i64 {
        (self.seconds * self.fps).round() as i64
    }

    /// The rate the file will come to, in kbps.
    pub fn kbps(&self) -> u32 {
        implied_kbps(self)
    }
}

/// How many megapixel-frames a second the rule of thumb assumes: the frame
/// size times the rate, so a 4K30 export pushes 249 of them a second and is
/// taken to be about a third of real time. Deliberately slow, because a
/// sheet that promises four minutes and takes eight is worse than one that
/// promises eight and takes four - and because the first real export
/// replaces it with a measured number.
const RULE_OF_THUMB_MEGAPIXEL_FRAMES_A_SECOND: f64 = 75.0;

/// The rate an export not steered by a bitrate comes to, in kbps: 8 Mbps for
/// H.264 at 1080p30 and CRF 20, scaled by the pixels and the rate, doubled
/// about every six steps of CRF below it, and adjusted for the codec.
pub fn implied_kbps(shape: &Shape) -> u32 {
    if shape.bitrate_kbps > 0 {
        return shape.bitrate_kbps;
    }
    let pixels_a_second = f64::from(shape.width) * f64::from(shape.height) * shape.fps.max(1.0);
    let reference = pixels_a_second / (1920.0 * 1080.0 * 30.0);
    // CRF 20 is the reference; every six steps halves or doubles the rate.
    let quality = 2f64.powf((20.0 - f64::from(shape.crf.clamp(1, 51))) / 6.0);
    let codec = match shape.codec {
        VideoCodec::H264 => 1.0,
        VideoCodec::Hevc => 0.62,
        VideoCodec::Av1 => 0.5,
        // The two alpha codecs are intra-only and cost far more.
        _ => 3.0,
    };
    let ten_bit = if shape.ten_bit { 1.05 } else { 1.0 };
    (8_000.0 * reference * quality * codec * ten_bit).clamp(200.0, 400_000.0) as u32
}

/// What `shape` will cost, with `measured` - the profile of an export
/// already run on this machine - deciding the time where there is one worth
/// believing.
pub fn estimate(shape: &Shape, measured: Option<&ExportProfile>) -> Estimate {
    let frames = shape.frames();
    // The video rate, plus a 192 kbps AAC track where there is sound, plus
    // about two percent of container, all over the cut's length.
    let audio_kbps = if shape.sound { 192.0 } else { 0.0 };
    let bytes = (f64::from(shape.kbps()) + audio_kbps) * 1000.0 / 8.0 * shape.seconds * 1.02;
    let measured_seconds = measured.and_then(|profile| measured_seconds(shape, frames, profile));
    Estimate {
        frames,
        seconds: shape.seconds,
        bytes: bytes.max(0.0) as u64,
        render_seconds: measured_seconds.unwrap_or_else(|| rule_of_thumb(shape, frames)),
        measured: measured_seconds.is_some(),
    }
}

/// [`estimate`] for a request, which is what a caller with one to hand
/// wants.
pub fn estimate_request(request: &ExportRequest, measured: Option<&ExportProfile>) -> Estimate {
    estimate(&Shape::of(request), measured)
}

/// How many bytes the filesystem holding `path` has free for an ordinary
/// user, or `None` where that cannot be asked.
///
/// `statvfs` is the only answer a phone gives: `f_bavail` is the space
/// actually available to this app, which on Android is smaller than the
/// partition - the system keeps a reserve - so a plan made from `f_bfree`
/// would promise room that is not there. Android 10 needs no permission to
/// ask, and the export's own folder is the one being asked about.
pub fn free_bytes(path: &std::path::Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let raw = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `raw` is a valid C string for the call's lifetime and
        // `stats` is a plain old data struct statvfs fills in whole.
        let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(raw.as_ptr(), &mut stats) } != 0 {
            return None;
        }
        let block = stats.f_frsize.max(1) as u64;
        Some(stats.f_bavail as u64 * block)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Whether `estimate` will fit in `free` bytes, with a little headroom: an
/// export that fills the phone exactly leaves nothing to write the index
/// with, and the file that fails at 99 percent is worse than one that never
/// starts.
pub fn fits(estimate: &Estimate, free: u64) -> bool {
    // The estimate is a steady-state average and a busy cut can overshoot it;
    // a tenth of the file is enough room for the overshoot and the mux.
    estimate.bytes.saturating_add(estimate.bytes / 10) <= free
}

/// The time the rule of thumb gives, or zero for a cut with no frames.
fn rule_of_thumb(shape: &Shape, frames: i64) -> f64 {
    if frames <= 0 {
        return 0.0;
    }
    let megapixel_frames =
        frames as f64 * f64::from(shape.width) * f64::from(shape.height) / 1_000_000.0;
    megapixel_frames / RULE_OF_THUMB_MEGAPIXEL_FRAMES_A_SECOND
}

/// The measured time for this shape, from a profile of an export that ran
/// on this machine at some frame size.
///
/// Scaling is by the work, which is pixels times frames: the same cut at
/// twice the pixels takes about twice as long. What does not scale is the
/// fixed cost of opening decoders and the mux, which is small beside a
/// rendered cut, and a profile that measured no frames - a failed export -
/// is not a prediction at all.
fn measured_seconds(shape: &Shape, frames: i64, profile: &ExportProfile) -> Option<f64> {
    if frames <= 0 || profile.frames() == 0 {
        return None;
    }
    let rendered_a_second = profile.frames_per_second();
    if !rendered_a_second.is_finite() || rendered_a_second <= 0.0 {
        return None;
    }
    let (ran_width, ran_height, _) = profile.output();
    if ran_width == 0 || ran_height == 0 {
        return None;
    }
    let ran_pixels = f64::from(ran_width) * f64::from(ran_height);
    let wanted_pixels = f64::from(shape.width) * f64::from(shape.height);
    let per_frame = (1.0 / rendered_a_second) * (wanted_pixels / ran_pixels);
    Some(per_frame * frames as f64)
}

#[cfg(test)]
mod tests {
    use super::{Estimate, Shape, estimate, implied_kbps};
    use crate::profile::{ExportProfile, Stage};
    use concat_media::VideoCodec;
    use std::time::Duration;

    fn shape(width: u32, height: u32, crf: u8) -> Shape {
        Shape {
            width,
            height,
            fps: 30.0,
            seconds: 30.0,
            crf,
            bitrate_kbps: 0,
            codec: VideoCodec::H264,
            ten_bit: false,
            sound: true,
        }
    }

    /// A measured profile: 300 frames rendered in sixty seconds, which is
    /// five frames a second at 1080p.
    fn measured_at_1080p() -> ExportProfile {
        let mut profile = ExportProfile::default();
        profile.output_is(1920, 1080, 30.0);
        profile.encoder_is("c2.qti.hevc.encoder", true);
        for index in 0..300 {
            profile.frame(index, 300, 1, Duration::from_millis(200));
        }
        profile.account(Stage::Composite, Duration::from_secs(60));
        profile
    }

    /// The frames follow from the length and the rate, and a 30-second
    /// 1080p30 cut at CRF 20 is in the tens of megabytes - not a few, and
    /// not hundreds.
    #[test]
    fn a_cut_knows_its_frames_and_its_size() {
        let estimate = estimate(&shape(1920, 1080, 20), None);
        assert_eq!(estimate.frames, 900);
        assert!((estimate.seconds - 30.0).abs() < 1e-6);
        let megabytes = estimate.bytes as f64 / 1_000_000.0;
        assert!((10.0..200.0).contains(&megabytes), "{megabytes} MB");
        assert!(estimate.line().contains("MB"), "{}", estimate.line());
        assert!(estimate.line().contains("estimated"), "{}", estimate.line());
    }

    /// Six steps of CRF is about half the file, an explicit bitrate wins
    /// over any rule, and HEVC at the same quality is smaller than H.264.
    #[test]
    fn the_size_follows_the_quality_asked_for() {
        let fine = implied_kbps(&shape(1920, 1080, 20));
        let coarse = implied_kbps(&shape(1920, 1080, 26));
        let ratio = f64::from(coarse) / f64::from(fine);
        assert!((0.45..0.55).contains(&ratio), "{ratio}");
        let mut steered = shape(1920, 1080, 20);
        steered.bitrate_kbps = 12_000;
        assert_eq!(implied_kbps(&steered), 12_000);
        let mut hevc = shape(1920, 1080, 20);
        hevc.codec = VideoCodec::Hevc;
        assert!(implied_kbps(&hevc) < fine);
        // A bigger frame at the same quality is a bigger file.
        assert!(implied_kbps(&shape(3840, 2160, 20)) > 3 * fine);
    }

    /// Once an export has been measured, the next one's time comes from it,
    /// and a bigger frame is a longer export in proportion.
    #[test]
    fn a_measured_export_predicts_the_next() {
        let profile = measured_at_1080p();
        let measured = estimate(&shape(1920, 1080, 20), Some(&profile));
        assert!(measured.measured);
        // 900 frames at five a second: three minutes.
        assert!(
            (measured.render_seconds - 180.0).abs() < 1.0,
            "{measured:?}"
        );
        let bigger = estimate(&shape(3840, 2160, 20), Some(&profile));
        let ratio = bigger.render_seconds / measured.render_seconds;
        assert!((3.9..4.1).contains(&ratio), "{ratio}");
        // Without a measurement the rule of thumb answers, and says so.
        let guessed = estimate(&shape(1920, 1080, 20), None);
        assert!(!guessed.measured);
        assert!(guessed.render_seconds > 0.0);
    }

    /// A profile that measured nothing (a failed export) does not become a
    /// prediction of zero seconds.
    #[test]
    fn an_empty_profile_falls_back_to_the_rule() {
        let empty = ExportProfile::default();
        let estimate = estimate(&shape(1280, 720, 23), Some(&empty));
        assert!(!estimate.measured);
        assert!(estimate.render_seconds > 0.0);
    }

    /// A file that would fill the phone is refused before it starts, and the
    /// space free is the space this app can have, asked of a folder that
    /// exists.
    #[test]
    fn a_file_that_would_fill_the_phone_is_refused() {
        let estimate = estimate(&shape(1920, 1080, 20), None);
        let headroom = estimate.bytes + estimate.bytes / 10;
        assert!(super::fits(&estimate, headroom));
        // Room for the file but nothing to write it with is still a refusal.
        assert!(!super::fits(&estimate, estimate.bytes));
        assert!(!super::fits(&estimate, estimate.bytes / 2));
        let free =
            super::free_bytes(std::path::Path::new(".")).expect("a size for the working folder");
        assert!(free > 0);
        // A folder that is not there has no size to give, which is not zero.
        assert!(super::free_bytes(std::path::Path::new("/no/such/folder")).is_none());
    }

    /// The labels are the ones a person reads: sizes in MB and GB, times as
    /// clocks.
    #[test]
    fn labels_read_like_a_person_wrote_them() {
        let small = Estimate {
            frames: 30,
            seconds: 1.0,
            bytes: 210_000_000,
            render_seconds: 250.0,
            measured: true,
        };
        assert_eq!(small.size_label(), "210 MB");
        assert_eq!(small.time_label(), "4:10");
        assert!(small.line().contains("measured"), "{}", small.line());
        let large = Estimate {
            frames: 30,
            seconds: 1.0,
            bytes: 1_240_000_000,
            render_seconds: 3_730.0,
            measured: false,
        };
        assert_eq!(large.size_label(), "1.2 GB");
        assert_eq!(large.time_label(), "1:02:10");
    }
}
