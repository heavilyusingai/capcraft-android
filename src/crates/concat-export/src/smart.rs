// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! Whether an export can hand the frames it already has straight to the
//! file instead of decoding, compositing and encoding them again.
//!
//! A cut that changes nothing about a clip - no effect, no transform, no
//! retime, the file's own codec and size and rate - is a copy of bytes
//! that are already compressed. Every editor that is fast at this does the
//! same thing and calls it smart rendering: copy the groups of pictures the
//! cut does not touch, and re-encode only the few frames between a cut and
//! the next place a decoder can start.
//!
//! The rules here are the ones the mature implementations converged on, and
//! they are all about *when not to*. A copy is only legal if the file being
//! written is the file being read: same codec, same profile and bit depth,
//! same size, same rate, same colour tags. And a copy can only start where a
//! decoder can start - on an IDR, not on a recovery point; the leading
//! pictures of an open group of pictures are part of the window that must be
//! re-encoded, not part of the copy. Reporting a cut that a copy cannot make
//! is worse than a slow export: the file plays with a smear at the join, or
//! not at all.
//!
//! So this module answers three questions and no more:
//! *can this clip be copied*, *from where to where*, and *what would have to
//! be re-encoded instead*. It is the plan the export executor follows; the
//! copying itself is a muxing job, not arithmetic, and lives with the
//! muxer.
//!
//! [`ExportClip`]: crate::ExportClip

use crate::{ClipKind, ExportClip, ExportRequest};
use concat_media::VideoCodec;

/// Seconds said the way a report says them: "45s", "1m 20s".
fn seconds_label(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (minutes, seconds) = (total / 60, total % 60);
    if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

/// What a source file is, as far as copying it is concerned.
///
/// The caller fills this from the project's own media record and from a
/// keyframe-only read of the file; nothing here is guessed.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// The file's video codec, named as the project names it: "h264",
    /// "hevc", "av1", "vp9", or anything else, which is uncopyable.
    pub codec: String,
    /// The frame's size in pixels.
    pub width: u32,
    /// The frame's height in pixels.
    pub height: u32,
    /// Frames a second, as the file's own fraction. `None` for a file whose
    /// rate is not known - which is not copyable, because the output's rate
    /// has to be identical.
    pub fps: Option<f64>,
    /// Ten bits a channel rather than eight.
    pub ten_bit: bool,
    /// Where a decoder can start, in seconds into the file, ascending:
    /// IDR pictures, or a group of pictures whose leading pictures are all
    /// present. A recovery point is *not* one of these.
    pub starts: Vec<f64>,
    /// Whether the file's groups of pictures are closed - no picture refers
    /// back before its keyframe. An open file can still be copied, but only
    /// by re-encoding the leading pictures at each join, and the honest
    /// answer for a phone that cannot see them is to decline.
    pub closed_gop: bool,
}

impl Source {
    /// A source about which nothing is known beyond its codec and size.
    pub fn new(codec: &str, width: u32, height: u32) -> Self {
        Self {
            codec: codec.to_owned(),
            width,
            height,
            fps: None,
            ten_bit: false,
            starts: Vec::new(),
            closed_gop: true,
        }
    }

    /// The codec, as the engine's own [`VideoCodec`], when it is one the
    /// engine writes.
    pub fn engine_codec(&self) -> Option<VideoCodec> {
        VideoCodec::parse(&self.codec)
    }

    /// The first place at or after `at` that a decoder can start on.
    pub fn start_at_or_after(&self, at: f64) -> Option<f64> {
        self.starts
            .iter()
            .copied()
            .find(|start| *start >= at - START_SLACK)
    }

    /// The last place at or before `at` that a decoder can start on.
    pub fn start_at_or_before(&self, at: f64) -> Option<f64> {
        self.starts
            .iter()
            .copied()
            .rev()
            .find(|start| *start <= at + START_SLACK)
    }
}

/// How close a cut has to be to a start before it counts as on it: a frame's
/// worth of rounding at 60 fps, and no more, so a cut a frame late is
/// re-encoded rather than copied from the wrong picture.
const START_SLACK: f64 = 0.0084;

/// Why a clip cannot be copied, in the words the report shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// An effect, a filter chain, or an adjustment is applied to it.
    Effected,
    /// It is moved, scaled, turned, stretched, cropped or flipped.
    Transformed,
    /// It is blended with what is under it, or faded.
    Blended,
    /// It is retimed: a speed other than one, a speed curve, or reversed.
    Retimed,
    /// It is keyframed or animated.
    Animated,
    /// It has a mask, a cutout, a stabiliser or a motion map.
    Masked,
    /// It is not footage: a still, sound, or a treatment with no pixels.
    NotFootage,
    /// Its picture is hidden, so the frame is the layer beneath.
    Hidden,
    /// It is a transition's outgoing or incoming half.
    InTransition,
    /// The file is not the codec the export writes.
    WrongCodec,
    /// The file is not the size the export writes.
    WrongSize,
    /// The file is not the rate the export writes, or its rate is unknown.
    WrongRate,
    /// The file is not the bit depth the export writes.
    WrongDepth,
    /// The file's groups of pictures are open, so a cut inside one would
    /// need its leading pictures re-encoded and this engine cannot see them.
    OpenGop,
    /// The file does not say where a decoder can start.
    NoStarts,
    /// The export writes HDR and the file is not, or the other way round.
    WrongColour,
}

impl Why {
    /// The reason in a sentence, for a report or a log line.
    pub fn sentence(self) -> &'static str {
        match self {
            Why::Effected => "an effect is applied to it",
            Why::Transformed => "it is moved, scaled or cropped",
            Why::Blended => "it is blended or faded",
            Why::Retimed => "it is retimed",
            Why::Animated => "it is animated",
            Why::Masked => "it is masked, cut out or stabilised",
            Why::NotFootage => "it is not footage",
            Why::Hidden => "its picture is hidden",
            Why::InTransition => "it is part of a transition",
            Why::WrongCodec => "the file is not the codec being written",
            Why::WrongSize => "the file is not the size being written",
            Why::WrongRate => "the file is not the rate being written",
            Why::WrongDepth => "the file is not the bit depth being written",
            Why::OpenGop => "the file's groups of pictures are open",
            Why::NoStarts => "the file does not say where a decoder can start",
            Why::WrongColour => "the file's colour is not the one being written",
        }
    }
}

/// One piece of the timeline, in the order it is written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece {
    /// Copied from the source, from the first second to the second.
    Copy {
        /// Seconds into the source.
        from: f64,
        /// Seconds into the source.
        to: f64,
    },
    /// Re-encoded from the first second to the second of the source.
    Recode {
        /// Seconds into the source.
        from: f64,
        /// Seconds into the source.
        to: f64,
        /// Why this window is being re-encoded rather than copied.
        why: Why,
    },
}

impl Piece {
    /// The seconds of source this piece covers.
    pub fn seconds(&self) -> f64 {
        match *self {
            Piece::Copy { from, to } | Piece::Recode { from, to, .. } => (to - from).max(0.0),
        }
    }

    /// Whether it is copied.
    pub fn is_copy(&self) -> bool {
        matches!(self, Piece::Copy { .. })
    }
}

/// What the export can do with one clip.
#[derive(Clone, Debug, PartialEq)]
pub struct Treatment {
    /// Index of the clip in the request's flattened list.
    pub clip: usize,
    /// The pieces it contributes, in order; empty when it cannot be copied
    /// at all and is encoded whole, which is what the export does now.
    pub pieces: Vec<Piece>,
    /// Why nothing of it can be copied, when nothing can.
    pub declines: Vec<Why>,
}

impl Treatment {
    /// Seconds of source that are copied.
    pub fn copied(&self) -> f64 {
        self.pieces
            .iter()
            .filter(|piece| piece.is_copy())
            .map(Piece::seconds)
            .sum()
    }

    /// Seconds of source that are re-encoded.
    pub fn recoded(&self) -> f64 {
        self.pieces
            .iter()
            .filter(|piece| !piece.is_copy())
            .map(Piece::seconds)
            .sum()
    }

    /// Whether any of it is copied.
    pub fn copies_anything(&self) -> bool {
        self.pieces.iter().any(Piece::is_copy)
    }
}

/// What the whole export can do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// One treatment a base-track clip, in clip order.
    pub treatments: Vec<Treatment>,
    /// Seconds of timeline that are copied rather than rendered.
    pub copied: f64,
    /// Seconds of timeline that are rendered.
    pub rendered: f64,
}

impl Plan {
    /// The share of the timeline that is copied, 0 to 1; zero for an empty
    /// timeline rather than a division by nothing.
    pub fn copied_fraction(&self) -> f64 {
        let total = self.copied + self.rendered;
        if total <= 0.0 {
            0.0
        } else {
            self.copied / total
        }
    }

    /// One line for the export report, or `None` when nothing at all can be
    /// copied: a report that always says something would say "0 percent"
    /// on every export with an effect on it, which is noise.
    pub fn line(&self) -> Option<String> {
        if self.copied <= 0.0 {
            return None;
        }
        Some(format!(
            "smart render: {:.0}% of the cut ({}) needs no re-encode",
            self.copied_fraction() * 100.0,
            seconds_label(self.copied)
        ))
    }

    /// The reasons clips could not be copied, each once, in the order they
    /// are met: what to remove to make the export faster.
    pub fn declines(&self) -> Vec<Why> {
        let mut seen: Vec<Why> = Vec::new();
        for treatment in &self.treatments {
            for why in &treatment.declines {
                if !seen.contains(why) {
                    seen.push(*why);
                }
            }
        }
        seen
    }
}

/// What the export itself is writing, which a copy has to match exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    /// The codec being written.
    pub codec: VideoCodec,
    /// The size being written.
    pub width: u32,
    /// The size being written.
    pub height: u32,
    /// The rate being written.
    pub fps: f64,
    /// Whether ten bits a channel are being written.
    pub ten_bit: bool,
    /// Whether HDR is being written.
    pub hdr: bool,
}

impl Target {
    /// What `request` writes.
    pub fn of(request: &ExportRequest) -> Self {
        Self {
            codec: request.codec,
            width: request.width,
            height: request.height,
            fps: if request.rate_den > 0 {
                request.rate_num as f64 / request.rate_den as f64
            } else {
                0.0
            },
            ten_bit: request.ten_bit,
            hdr: request.hdr,
        }
    }
}

/// Why `clip` cannot be copied at all, or `None` when it could be.
///
/// Everything here is a property of the clip alone: the source facts are
/// checked by [`plan`], which is where a file that does not match the output
/// is caught.
pub fn refusal(clip: &ExportClip) -> Option<Why> {
    if clip.kind != ClipKind::Video {
        return Some(Why::NotFootage);
    }
    if clip.hidden {
        return Some(Why::Hidden);
    }
    if !clip.filter_chain.trim().is_empty()
        || !clip.effects.is_empty()
        || clip.cutout.is_some()
        || clip.mask.is_some()
        || clip.stabilize.is_some()
        || clip.reveal_map.is_some()
        || !clip.mask_dir.trim().is_empty()
        || !clip.motion_dir.trim().is_empty()
    {
        // A chain and an effect are both work on the pixels; a mask, a
        // cutout, a stabiliser and a motion map are work on where they go.
        return Some(
            if clip.mask.is_some()
                || clip.cutout.is_some()
                || clip.stabilize.is_some()
                || clip.reveal_map.is_some()
                || !clip.mask_dir.trim().is_empty()
            {
                Why::Masked
            } else {
                Why::Effected
            },
        );
    }
    if clip.transition.is_some() || !clip.transition_shapes.is_empty() {
        return Some(Why::InTransition);
    }
    if (clip.speed - 1.0).abs() > f64::EPSILON
        || clip.hold
        || !clip.speed_curve.is_empty()
        || !clip.speed_slopes.is_empty()
    {
        return Some(Why::Retimed);
    }
    if !clip.animation.is_empty() {
        return Some(Why::Animated);
    }
    if clip.blend != "normal" && !clip.blend.is_empty()
        || clip.opacity < 1.0
        || clip.video_fade_in > 0.0
    {
        return Some(Why::Blended);
    }
    // Identity transform, to the rounding a slider gives.
    let moved = clip.offset_x.abs() > 1e-9
        || clip.offset_y.abs() > 1e-9
        || (clip.scale - 1.0).abs() > 1e-9
        || (clip.stretch_x - 1.0).abs() > 1e-9
        || (clip.stretch_y - 1.0).abs() > 1e-9
        || clip.rotation.abs() > 1e-9
        || clip.flip_h
        || clip.flip_v
        || clip.crop.is_some()
        || clip.overscan.abs() > 1e-9
        || clip.backdrop.is_some();
    if moved {
        return Some(Why::Transformed);
    }
    None
}

/// The plan for `request`, given what the sources are.
///
/// `source_of` answers for a clip's file, and may say it does not know:
/// a file whose facts are missing is simply not copied.
pub fn plan(request: &ExportRequest, source_of: impl Fn(&str) -> Option<Source>) -> Plan {
    let target = Target::of(request);
    let mut plan = Plan::default();
    for (index, clip) in request.clips.iter().enumerate() {
        // A track above the base one is composited over what is under it
        // unless it is opaque and full-frame, which the clip's own
        // transform already says; anything above the base track with
        // anything to composite is not a copy.
        let mut why = refusal(clip);
        if why.is_none() && clip.track > 0 {
            why = Some(Why::Blended);
        }
        let source = source_of(&clip.path);
        if why.is_none() {
            why = match &source {
                None => Some(Why::NoStarts),
                Some(source) => mismatch(source, &target),
            };
        }
        match (why, source) {
            (None, Some(source)) => {
                let treatment = windows(index, clip, &source);
                plan.copied += treatment.copied().min(clip.duration);
                plan.rendered += treatment.recoded().min(clip.duration);
                plan.treatments.push(treatment);
            }
            (why, _) => {
                // Encoded whole, as every export does now: the seconds it
                // covers are rendered, and the reason is kept for the
                // report.
                plan.rendered += clip.duration.max(0.0);
                plan.treatments.push(Treatment {
                    clip: index,
                    pieces: Vec::new(),
                    declines: why.into_iter().collect(),
                });
            }
        }
    }
    plan
}

/// Whether `source` is the file `target` is writing, which is the only
/// condition under which any of it may be copied.
pub fn mismatch(source: &Source, target: &Target) -> Option<Why> {
    if source.engine_codec() != Some(target.codec) {
        return Some(Why::WrongCodec);
    }
    if source.width != target.width || source.height != target.height {
        return Some(Why::WrongSize);
    }
    match source.fps {
        // A file's rate has to be the output's rate, to the fraction, or
        // the copied frames land on the wrong grid.
        Some(fps) if (fps - target.fps).abs() > 0.001 => return Some(Why::WrongRate),
        None => return Some(Why::WrongRate),
        _ => {}
    }
    if source.ten_bit != target.ten_bit {
        return Some(Why::WrongDepth);
    }
    // A ten-bit HDR file copied into an SDR export would carry its PQ or HLG
    // tags into a file that says BT.709 - and the reverse. Neither is a copy
    // of the same pictures.
    if target.hdr && !source.ten_bit || source.ten_bit && target.hdr == false && !target.ten_bit {
        return Some(Why::WrongColour);
    }
    None
}

/// Where `clip`'s copy can start and stop, and what has to be re-encoded
/// around it.
fn windows(index: usize, clip: &ExportClip, source: &Source) -> Treatment {
    let first = clip.source_start.max(0.0);
    let last = first + clip.duration.max(0.0);
    if source.starts.is_empty() {
        return Treatment {
            clip: index,
            pieces: Vec::new(),
            declines: vec![Why::NoStarts],
        };
    }
    if !source.closed_gop {
        // The leading pictures of an open group are part of a copy's
        // dependency, and this engine cannot read them out of the packets
        // yet: the honest answer is to re-encode the clip.
        return Treatment {
            clip: index,
            pieces: Vec::new(),
            declines: vec![Why::OpenGop],
        };
    }
    let Some(clean) = source.start_at_or_after(first) else {
        // No start left in the file: the whole clip is one open group.
        return Treatment {
            clip: index,
            pieces: vec![Piece::Recode {
                from: first,
                to: last,
                why: Why::NoStarts,
            }],
            declines: Vec::new(),
        };
    };
    let mut pieces = Vec::new();
    if clean > first + START_SLACK {
        // From the cut to the next place a decoder can start: the window
        // whose frames depend on pictures the copy does not carry.
        pieces.push(Piece::Recode {
            from: first,
            to: clean,
            why: Why::NoStarts,
        });
    }
    if last > clean + START_SLACK {
        pieces.push(Piece::Copy {
            from: clean,
            to: last,
        });
    }
    Treatment {
        clip: index,
        pieces,
        declines: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Piece, Plan, Source, Target, Why, mismatch, plan, refusal};
    use crate::{ClipKind, ExportClip, ExportRequest};
    use concat_media::VideoCodec;

    /// A request that writes 1080p H.264 at 30 fps, eight bits, SDR.
    fn request(clips: Vec<ExportClip>) -> ExportRequest {
        ExportRequest {
            output: "/tmp/out.mp4".to_owned(),
            width: 1920,
            height: 1080,
            rate_num: 30,
            rate_den: 1,
            crf: 20,
            preset: "medium".to_owned(),
            codec: VideoCodec::H264,
            ten_bit: false,
            rate_mode: Default::default(),
            bitrate_kbps: 0,
            color_range: Default::default(),
            color_space: Default::default(),
            hdr: false,
            effect_time_offset: 0.0,
            canvas: [0, 0, 0, 255],
            clips,
        }
    }

    /// An untouched clip of `seconds` from `source_start` of a 1080p30
    /// H.264 file, on the base track.
    fn plain(source_start: f64, seconds: f64) -> ExportClip {
        ExportClip {
            path: "/media/a.mp4".to_owned(),
            kind: ClipKind::Video,
            start: 0.0,
            duration: seconds,
            source_start,
            track: 0,
            media_width: Some(1920),
            media_height: Some(1080),
            ..ExportClip::default()
        }
    }

    /// The same file, with keyframes every two seconds from zero.
    fn source() -> Source {
        Source {
            fps: Some(30.0),
            starts: (0..10).map(|index| f64::from(index) * 2.0).collect(),
            ..Source::new("h264", 1920, 1080)
        }
    }

    /// A cut that lands on a keyframe is copied whole; one that lands
    /// between keyframes is re-encoded only up to the next one.
    #[test]
    fn a_cut_between_keyframes_re_encodes_only_the_head() {
        let cut_on_key = plan(&request(vec![plain(4.0, 3.0)]), |_| Some(source()));
        let treatment = &cut_on_key.treatments[0];
        assert_eq!(treatment.pieces, vec![Piece::Copy { from: 4.0, to: 7.0 }]);
        assert!((treatment.copied() - 3.0).abs() < 1e-6);
        assert_eq!(treatment.recoded(), 0.0);
        assert!(cut_on_key.copied_fraction() > 0.99);

        // Half a second into a group: 1.5 s re-encoded, 3 s copied.
        let cut_inside = plan(&request(vec![plain(4.5, 4.5)]), |_| Some(source()));
        let treatment = &cut_inside.treatments[0];
        assert_eq!(treatment.pieces.len(), 2);
        assert!(matches!(
            treatment.pieces[0],
            Piece::Recode { from, to, .. } if (from - 4.5).abs() < 1e-6 && (to - 6.0).abs() < 1e-6
        ));
        assert!(matches!(
            treatment.pieces[1],
            Piece::Copy { from, to } if (from - 6.0).abs() < 1e-6 && (to - 9.0).abs() < 1e-6
        ));
        assert!((treatment.recoded() - 1.5).abs() < 1e-6);
        assert!((treatment.copied() - 3.0).abs() < 1e-6);
    }

    /// Anything that touches the pixels or the frame's place on the canvas
    /// is a re-encode, and the reason says which.
    #[test]
    fn work_on_the_pixels_is_never_copied() {
        let mut effected = plain(0.0, 2.0);
        effected.filter_chain = "eq=contrast=1.2".to_owned();
        assert_eq!(refusal(&effected), Some(Why::Effected));

        let mut moved = plain(0.0, 2.0);
        moved.scale = 1.1;
        assert_eq!(refusal(&moved), Some(Why::Transformed));

        let mut slow = plain(0.0, 2.0);
        slow.speed = 0.5;
        assert_eq!(refusal(&slow), Some(Why::Retimed));

        let mut keyed = plain(0.0, 2.0);
        keyed.animation = vec![crate::ExportKey {
            property: "scale".to_owned(),
            at: 0.5,
            value: 1.5,
            ease: [0.0, 0.0, 1.0, 1.0],
        }];
        assert_eq!(refusal(&keyed), Some(Why::Animated));

        let mut over = plain(0.0, 2.0);
        over.track = 1;
        let plan = plan(&request(vec![over]), |_| Some(source()));
        assert_eq!(plan.treatments[0].declines, vec![Why::Blended]);
        assert_eq!(plan.copied, 0.0);

        // An untouched clip is not refused by its own description.
        assert_eq!(refusal(&plain(0.0, 2.0)), None);
    }

    /// The file has to be the one being written: same codec, size, rate and
    /// depth, or the frames would land somewhere they do not belong.
    #[test]
    fn the_file_has_to_match_the_output() {
        let target = Target::of(&request(Vec::new()));
        assert_eq!(mismatch(&source(), &target), None);

        let mut hevc = source();
        hevc.codec = "hevc".to_owned();
        assert_eq!(mismatch(&hevc, &target), Some(Why::WrongCodec));

        let mut small = source();
        small.width = 1280;
        assert_eq!(mismatch(&small, &target), Some(Why::WrongSize));

        let mut pal = source();
        pal.fps = Some(25.0);
        assert_eq!(mismatch(&pal, &target), Some(Why::WrongRate));

        let mut unknown_rate = source();
        unknown_rate.fps = None;
        assert_eq!(mismatch(&unknown_rate, &target), Some(Why::WrongRate));

        let mut deep = source();
        deep.ten_bit = true;
        assert_eq!(mismatch(&deep, &target), Some(Why::WrongDepth));
    }

    /// An open group of pictures, and a file that says nothing about where
    /// a decoder can start, are both declined rather than guessed at.
    #[test]
    fn a_file_that_cannot_be_cut_cleanly_is_declined() {
        let mut open = source();
        open.closed_gop = false;
        let refused = plan(&request(vec![plain(4.0, 3.0)]), |_| Some(open.clone()));
        assert_eq!(refused.treatments[0].declines, vec![Why::OpenGop]);
        assert_eq!(refused.copied, 0.0);
        assert!(refused.line().is_none());

        // A file whose rate is not known is the wrong rate, whatever else it
        // is - the copy would land on the wrong frame grid.
        let blank = Source::new("h264", 1920, 1080);
        let unstarted = plan(&request(vec![plain(4.0, 3.0)]), |_| Some(blank.clone()));
        assert_eq!(unstarted.treatments[0].declines, vec![Why::WrongRate]);

        // A file that matches but says nothing about where a decoder can
        // start is declined rather than copied from the top.
        let mut silent = source();
        silent.starts.clear();
        let nowhere = plan(&request(vec![plain(4.0, 3.0)]), |_| Some(silent.clone()));
        assert_eq!(nowhere.treatments[0].declines, vec![Why::NoStarts]);

        // A file the caller knows nothing about is not copied either.
        let unknown = plan(&request(vec![plain(4.0, 3.0)]), |_| None);
        assert_eq!(unknown.copied, 0.0);
        assert_eq!(unknown.rendered, 3.0);
    }

    /// The report line is the share copied and the seconds, and the reasons
    /// are listed once each however many clips share them.
    #[test]
    fn the_plan_says_what_it_would_save_and_what_stops_it() {
        let mut moved = plain(4.0, 3.0);
        moved.scale = 2.0;
        let planned = plan(
            &request(vec![plain(4.0, 6.0), moved.clone(), moved.clone()]),
            |_| Some(source()),
        );
        let line = planned.line().expect("a line when something is copied");
        assert!(line.contains("smart render"), "{line}");
        assert!(line.contains('%'), "{line}");
        let declines = planned.declines();
        assert_eq!(declines, vec![Why::Transformed]);
        assert_eq!(declines[0].sentence(), "it is moved, scaled or cropped");
        // Nothing copied at all is no line rather than a zero.
        let nothing: Plan = plan(&request(vec![moved]), |_| Some(source()));
        assert!(nothing.line().is_none());
    }
}
