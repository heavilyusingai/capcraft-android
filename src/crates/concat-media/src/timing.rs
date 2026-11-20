// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! What a file's frame timing really is, and how to lay it on a constant
//! grid without the picture drifting away from the sound.
//!
//! Phones, screen recorders and webcams do not record at a constant rate.
//! They stamp each frame when it arrives - 33 ms, 33 ms, then a 180 ms gap
//! while the encoder catches its breath, then a burst at 8 ms - and a
//! container that reports "30 fps" is reporting an average, not a schedule.
//! An editor that believes the average drifts: the sound is locked to real
//! time and the picture is locked to frame numbers, so by the end of a long
//! recording they are seconds apart. This is the single most common cause of
//! "the audio is out of sync after exporting", and it is why every serious
//! tool converts variable-rate footage to a constant rate on the way in.
//!
//! Two questions, then, and this module answers both without touching a
//! decoder:
//!
//! * **What rate is this file really?** Not the container's average, which
//!   is a statistic, but the standard rate that explains the most frames
//!   with the least repeating and dropping. [`survey`] tries the real
//!   broadcast and camera rates and keeps the one that costs least.
//! * **Which source frame belongs on output frame *n*?** [`conform`] walks a
//!   constant grid and, for each instant, names the frame that was on screen
//!   then - the rule FFmpeg's `fps` filter and `-fps_mode cfr` implement, and
//!   the one `MediaCodec` does not do for you (it never drops a frame; you
//!   have to stop feeding it).
//!
//! The measurements are taken from presentation times because that is what a
//! container holds and what a decoder returns. Nothing here guesses from a
//! file's name, its container's `avg_frame_rate`, or its codec.

use concat_core::time::FrameRate;

/// The rates a file is actually recorded at: the two NTSC families, their
/// whole-number twins, and the phone rates. A file that is none of these is
/// measured and described, not forced.
pub const CANDIDATES: [FrameRate; 8] = [
    FrameRate::FILM_NTSC,
    FrameRate::FILM,
    FrameRate::PAL,
    FrameRate::NTSC_30,
    FrameRate::THIRTY,
    FrameRate::NTSC_60,
    FrameRate::SIXTY,
    FrameRate::HUNDRED_TWENTY,
];

/// How much a repeated frame costs against a dropped one when choosing the
/// rate. A dropped frame is a picture nobody sees; a repeated frame is a
/// picture held a moment too long, which the eye forgives. Dropping is
/// therefore the costlier mistake, and a rate that drops fewer frames wins
/// even if it repeats a few.
const DROP_COSTS: f64 = 1.5;

/// What a file's timing looks like, measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Survey {
    /// Frames measured.
    pub frames: usize,
    /// The shortest gap between two consecutive frames, in seconds.
    pub shortest_gap: f64,
    /// The longest gap, in seconds - a stall, if it is much longer than the
    /// median.
    pub longest_gap: f64,
    /// The middle gap: the file's usual pace.
    pub gap: f64,
    /// The rate the file was recorded at, as the nearest rate that explains
    /// its frames with the fewest repeats and drops.
    pub rate: FrameRate,
    /// Whether the gaps are uneven enough to matter: a file stamped at a
    /// constant rate has gaps that differ by microseconds, and a screen
    /// recording's differ by hundreds of milliseconds.
    pub variable: bool,
    /// Frames this file would have had at [`Survey::rate`], across its span.
    pub frames_at_rate: usize,
    /// Frames that would be repeated to fill the grid.
    pub repeated: usize,
    /// Frames that would be dropped - never shown.
    pub dropped: usize,
}

impl Survey {
    /// The file's average pace, in frames a second, straight from its
    /// frames: the count over the span.
    pub fn average_fps(&self) -> f64 {
        match self.frames_at_rate.checked_sub(1) {
            Some(_) if self.gap > 0.0 => 1.0 / self.gap,
            _ => 0.0,
        }
    }

    /// Whether the file can be read as constant-rate as it stands: the
    /// gaps are even and its frames land on the rate's own grid.
    pub fn is_constant(&self) -> bool {
        !self.variable && self.repeated == 0 && self.dropped == 0
    }

    /// The line for a report or a log: what was measured, and what it
    /// would take to lay it on its grid.
    pub fn sentence(&self) -> String {
        let rate = self.rate.fps();
        let label = format!("{:.3}", rate.as_f64());
        let label = label.trim_end_matches('0').trim_end_matches('.').to_owned();
        if self.is_constant() {
            return format!("{label} fps, even ({} frames)", self.frames);
        }
        format!(
            "gaps {:.0}-{:.0} ms around {:.0} ms: normalised to {label} fps, {} repeated, {} dropped",
            self.shortest_gap * 1000.0,
            self.longest_gap * 1000.0,
            self.gap * 1000.0,
            self.repeated,
            self.dropped
        )
    }
}

/// The grid a file's frames are laid on: for each output frame, which
/// source frame is shown.
#[derive(Clone, Debug, PartialEq)]
pub struct Mapping {
    /// The rate of the grid.
    pub rate: FrameRate,
    /// The first frame's presentation time, which the grid starts from.
    pub origin: f64,
    /// One source frame index per output frame, ascending and never
    /// repeating within a run of the same picture - repeats are how a slow
    /// stretch is held.
    pub source: Vec<usize>,
    /// Source frames never shown.
    pub dropped: usize,
    /// Output frames that hold the picture before them.
    pub repeated: usize,
}

impl Mapping {
    /// Output frames on the grid.
    pub fn frames(&self) -> usize {
        self.source.len()
    }

    /// The source frame shown at output frame `index`, or the last one for
    /// a frame past the end - a grid asked for one frame too many holds the
    /// picture rather than failing.
    pub fn at(&self, index: usize) -> Option<usize> {
        match self.source.len() {
            0 => None,
            length if index >= length => self.source.last().copied(),
            _ => Some(self.source[index]),
        }
    }

    /// The source frame index shown at `seconds` into the file, from the
    /// grid - the question a seeking reader asks.
    pub fn at_time(&self, seconds: f64) -> Option<usize> {
        let fps = self.rate.fps().as_f64();
        if fps <= 0.0 || seconds < self.origin {
            return self.at(0);
        }
        let index = ((seconds - self.origin) * fps).floor();
        if index < 0.0 {
            return self.at(0);
        }
        self.at(index as usize)
    }
}

/// Measures `times` - the presentation time of every frame of a file, in
/// seconds, in order - and says what it is and what it would take to lay it
/// on a constant grid. `None` for fewer than two frames, which has no gaps
/// to measure.
pub fn survey(times: &[f64]) -> Option<Survey> {
    let times = sorted(times);
    if times.len() < 2 {
        return None;
    }
    let mut gaps: Vec<f64> = times.windows(2).map(|pair| pair[1] - pair[0]).collect();
    // A file with two frames stamped at the same instant has no gap there;
    // it is a duplicate, not a measurement.
    gaps.retain(|gap| *gap > 0.0);
    if gaps.is_empty() {
        return None;
    }
    let mut ordered = gaps.clone();
    ordered.sort_by(f64::total_cmp);
    let gap = ordered[ordered.len() / 2];
    let shortest_gap = ordered[0];
    let longest_gap = *ordered.last().expect("not empty");
    // Uneven when the gaps span more than half again the usual one, or when
    // the middle half of them disagree by more than a tenth. A constant-rate
    // file's gaps differ in the last microseconds of a timestamp; this is
    // looking for the tens of milliseconds that a recorder's stalls make.
    let variable = longest_gap > gap * 1.5 || shortest_gap < gap * 0.6;

    let mut best: Option<Survey> = None;
    for rate in CANDIDATES {
        let mapping = conform(&times, rate);
        let cost = mapping.dropped as f64 * DROP_COSTS + mapping.repeated as f64;
        let candidate = Survey {
            frames: times.len(),
            shortest_gap,
            longest_gap,
            gap,
            rate,
            variable,
            frames_at_rate: mapping.frames(),
            repeated: mapping.repeated,
            dropped: mapping.dropped,
        };
        best = match best {
            None => Some(candidate),
            Some(current) => {
                let current_cost = current.dropped as f64 * DROP_COSTS + current.repeated as f64;
                // A cheaper layout wins; on a tie the rate nearest the
                // file's own pace does, so a 60 fps recording is not
                // reported as 59.94 because it fitted one frame better.
                let closer = (rate.fps().as_f64() - 1.0 / gap).abs()
                    < (current.rate.fps().as_f64() - 1.0 / gap).abs();
                Some(if cost < current_cost || (cost == current_cost && closer) {
                    candidate
                } else {
                    current
                })
            }
        };
    }
    best
}

/// Lays `times` on `rate`'s grid: which source frame is shown at each output
/// frame.
///
/// The rule is the one FFmpeg's `fps` filter uses and `-fps_mode cfr` names:
/// at each instant on the grid, show the frame that most recently arrived -
/// the last frame whose presentation time is at or before it. A slow stretch
/// therefore holds its last picture, and a burst keeps the newest frame of
/// the burst, which is what the eye would have seen.
pub fn conform(times: &[f64], rate: FrameRate) -> Mapping {
    let times = sorted(times);
    let fps = rate.fps().as_f64();
    if times.is_empty() || fps <= 0.0 {
        return Mapping {
            rate,
            origin: times.first().copied().unwrap_or(0.0),
            source: Vec::new(),
            dropped: 0,
            repeated: 0,
        };
    }
    let origin = times[0];
    let span = (times[times.len() - 1] - origin).max(0.0);
    // The frame on the grid that starts at the file's first picture, and the
    // last that starts inside it: a grid frame at or past the end holds the
    // last picture.
    let frames = (span * fps).floor() as usize + 1;
    let mut source = Vec::with_capacity(frames);
    let mut shown = vec![false; times.len()];
    let mut repeated = 0;
    for index in 0..frames {
        let instant = origin + index as f64 / fps;
        // The last frame at or before this instant. The half-microsecond is
        // the tolerance of a timestamp written in a container's own time
        // base: a frame stamped 33.3333 ms belongs to the grid frame that
        // starts at 33.3333 ms, not to the one before it.
        let chosen = times
            .partition_point(|time| *time <= instant + 0.5e-6)
            .saturating_sub(1);
        if index > 0 && source.last() == Some(&chosen) {
            repeated += 1;
        }
        shown[chosen] = true;
        source.push(chosen);
    }
    Mapping {
        rate,
        origin,
        dropped: shown.iter().filter(|seen| !**seen).count(),
        repeated,
        source,
    }
}

/// How many frames [`survey_file`] reads before it stops. A hundred seconds
/// at 30 fps: long enough that a recorder's stalls and bursts are all in it,
/// short enough that opening a ten-minute file costs a fraction of a second
/// and not a walk through all of it.
pub const SAMPLE_FRAMES: usize = 3_000;

/// Measures a file by demuxing its pictures - no decoding, no pixels - and
/// returns what its timing is. `None` when the file will not open or has no
/// picture stream.
///
/// Reading packets rather than decoding frames is the cheap way to a
/// container's own schedule: the presentation time of every picture is in
/// the packet header, and this never touches a codec.
pub fn survey_file(path: &std::path::Path) -> Option<Survey> {
    let times = sample_file(path, SAMPLE_FRAMES)?;
    survey(&times)
}

/// The presentation times of the first `limit` pictures in the file, in
/// seconds, in file order. `None` when the file will not open or has no
/// picture stream.
pub fn sample_file(path: &std::path::Path, limit: usize) -> Option<Vec<f64>> {
    let mut input = ffmpeg::format::input(path).ok()?;
    let (index, time_base) = {
        let stream = crate::probe::picture_stream(&input)?;
        (stream.index(), stream.time_base())
    };
    let mut times = Vec::with_capacity(limit.min(SAMPLE_FRAMES));
    while times.len() < limit {
        match crate::feed::read_packet(&mut input, index) {
            Ok(crate::feed::Input::Packet(packet)) => {
                // A packet with no timestamp is not a measurement; a
                // container that carries none at all yields nothing here,
                // and the caller falls back to the rate it probed.
                // The presentation time where the container has one, and the
                // decode time where a B-frame stream only carries that: it is
                // the frame's own place on the timeline either way.
                if let Some(ticks) = packet.pts().or(packet.dts()) {
                    if let Some(seconds) = crate::ffi::seconds(ticks, time_base) {
                        times.push(seconds.as_f64());
                    }
                }
            }
            Ok(crate::feed::Input::End) | Err(_) => break,
        }
    }
    Some(times)
}

/// `times` in ascending order. A file whose timestamps are out of order is a
/// broken file, not an error worth refusing: sorting it is what a decoder's
/// own index would do.
fn sorted(times: &[f64]) -> Vec<f64> {
    let mut sorted: Vec<f64> = times
        .iter()
        .copied()
        .filter(|time| time.is_finite())
        .collect();
    if !sorted.windows(2).all(|pair| pair[0] <= pair[1]) {
        sorted.sort_by(f64::total_cmp);
    }
    sorted
}

#[cfg(test)]
mod tests {
    use super::{CANDIDATES, Mapping, conform, survey};
    use crate::time::FrameRate;

    /// Presentation times for `frames` frames at exactly `fps`.
    fn constant(fps: f64, frames: usize) -> Vec<f64> {
        (0..frames).map(|index| index as f64 / fps).collect()
    }

    /// A file stamped at a constant rate is recognised as one, at its own
    /// rate, with nothing to repeat or drop.
    #[test]
    fn an_even_file_is_left_alone() {
        for (fps, rate) in [
            (30.0, FrameRate::THIRTY),
            (25.0, FrameRate::PAL),
            (60.0, FrameRate::SIXTY),
            (24.0, FrameRate::FILM),
        ] {
            let measured = survey(&constant(fps, 300)).expect("measures");
            assert_eq!(measured.rate, rate, "{fps} fps");
            assert!(!measured.variable, "{fps} fps");
            assert_eq!(measured.dropped, 0, "{fps} fps");
            assert_eq!(measured.repeated, 0, "{fps} fps");
            assert!(measured.is_constant());
            assert!(
                measured.sentence().contains("fps, even"),
                "{}",
                measured.sentence()
            );
        }
    }

    /// 29.97 is not 30: the thousandth of a frame a second is 3.6 frames a
    /// minute, and a file recorded at it is labelled as what it is.
    #[test]
    fn ntsc_rates_are_not_rounded_to_their_whole_twins() {
        let measured = survey(&constant(30000.0 / 1001.0, 600)).expect("measures");
        assert_eq!(measured.rate, FrameRate::NTSC_30, "{}", measured.sentence());
        assert!(measured.dropped <= 1, "{}", measured.sentence());
        let measured = survey(&constant(24000.0 / 1001.0, 480)).expect("measures");
        assert_eq!(measured.rate, FrameRate::FILM_NTSC);
    }

    /// A screen recording that stalls for a fifth of a second is variable,
    /// and normalising it repeats the picture it stalled on rather than
    /// letting the whole file fall behind by that much.
    #[test]
    fn a_stalled_recording_is_normalised_by_holding_its_picture() {
        let mut times = constant(30.0, 120);
        // A 0.2 s stall: frames 120-125 never arrived, the file carries on
        // from where it left off.
        let stall = 0.2;
        for after in 120..240 {
            times.push(120.0 / 30.0 + (after - 120) as f64 / 30.0);
        }
        for time in times.iter_mut().skip(120) {
            *time += stall;
        }
        let measured = survey(&times).expect("measures");
        assert!(measured.variable, "{}", measured.sentence());
        assert_eq!(measured.rate, FrameRate::THIRTY, "{}", measured.sentence());
        // A stall costs the picture nothing: at most the one frame that
        // arrived just before the join, which the grid already had.
        assert!(measured.dropped <= 1, "{}", measured.sentence());
        // Six frames' worth of stall is held, not skipped.
        assert!(
            (measured.repeated as i64 - (stall * 30.0).round() as i64).abs() <= 1,
            "{}",
            measured.sentence()
        );
        assert!(
            measured.sentence().contains("normalised to 30 fps"),
            "{}",
            measured.sentence()
        );
    }

    /// A 60 fps file laid on a 30 fps grid drops every other frame and holds
    /// none; the frames it keeps are the newest of each pair.
    #[test]
    fn halving_the_rate_keeps_the_newest_picture() {
        let times = constant(60.0, 120);
        let mapping = conform(&times, FrameRate::THIRTY);
        assert_eq!(mapping.frames(), 60);
        assert_eq!(mapping.repeated, 0);
        assert_eq!(mapping.dropped, 60);
        // Frame 0 at 0 s, frame 2 at 1/30 s, frame 4 at 2/30 s...
        for index in 0..10 {
            assert_eq!(mapping.at(index), Some(index * 2));
        }
        // Reading by time gives the same answer as reading by index.
        assert_eq!(mapping.at_time(1.0 / 30.0 + 1e-9), Some(2));
        assert_eq!(mapping.at_time(0.0), Some(0));
    }

    /// Doubling the rate repeats each picture once, and the grid is never
    /// asked for a frame the file does not have.
    #[test]
    fn doubling_the_rate_repeats_each_picture() {
        let times = constant(30.0, 60);
        let mapping = conform(&times, FrameRate::SIXTY);
        // 60 pictures span 59 intervals, so the 60 fps grid has 119 frames:
        // every one of the sixty is shown twice except the last.
        assert_eq!(mapping.frames(), 119);
        assert_eq!(mapping.repeated, 59);
        assert_eq!(mapping.dropped, 0);
        assert_eq!(mapping.at(0), Some(0));
        assert_eq!(mapping.at(1), Some(0));
        assert_eq!(mapping.at(2), Some(1));
        // Past the end is the last picture, not a panic.
        assert_eq!(mapping.at(9_999), Some(59));
    }

    /// A file whose timestamps arrive out of order, or with a duplicate, is
    /// still laid on a grid - a decoder's index would sort it too.
    #[test]
    fn broken_timestamps_are_sorted_rather_than_refused() {
        let mut times = constant(30.0, 10);
        times.swap(2, 7);
        times.push(times[3]);
        let measured = survey(&times).expect("measures");
        assert_eq!(measured.rate, FrameRate::THIRTY);
        // Nothing to measure at all, though, is not a survey.
        assert!(survey(&[]).is_none());
        assert!(survey(&[1.0]).is_none());
        assert!(survey(&[1.0, 1.0, 1.0]).is_none());
        // And a rate that cannot be walked gives an empty mapping, not a
        // panic.
        let empty: Mapping = conform(&[], FrameRate::THIRTY);
        assert_eq!(empty.frames(), 0);
        assert_eq!(empty.at(0), None);
        assert_eq!(empty.at_time(1.0), None);
    }

    /// Every candidate rate is tried and the cheapest layout wins: a file
    /// that is mostly 30 with a burst of 60 is 30 fps, held where it stalled,
    /// rather than 60 fps with half its frames invented.
    #[test]
    fn the_cheapest_layout_wins() {
        let mut times = constant(30.0, 300);
        // Two bursts at 60 fps inserted in the middle.
        times.extend((0..30).map(|index| 10.0 + index as f64 / 60.0));
        times.sort_by(f64::total_cmp);
        let measured = survey(&times).expect("measures");
        assert_eq!(measured.rate, FrameRate::THIRTY, "{}", measured.sentence());
        assert!(measured.variable, "{}", measured.sentence());
        assert!(CANDIDATES.contains(&measured.rate));
    }
}
