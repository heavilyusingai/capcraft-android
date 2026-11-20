// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer and Concat contributors
// SPDX-FileCopyrightText: 2026 CapCraft contributors

//! Writing the timeline to a video file: one export at a time, on a thread
//! of its own, reporting through a listener and stopping when asked.
//!
//! The file is written into the app's cache; the app publishes it to the
//! phone's Movies folder once it is finished.

use std::sync::Arc;

use concat_host::export::{ExportSpec, Progress, RateMode, VideoCodec};

use crate::library::even;
use crate::session::MobileSession;
use crate::{MobileError, context};

/// What an export is likely to cost, as the sheet says it while it runs.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ExportEstimate {
    /// The file's size in bytes.
    pub bytes: u64,
    /// Seconds of timeline the cut covers.
    pub seconds: f64,
    /// How long the render is likely to take, in seconds.
    pub render_seconds: f64,
    /// Frames the cut will render.
    pub frames: i64,
    /// The time came from an export measured on this phone rather than a
    /// rule of thumb.
    pub measured: bool,
    /// The line to show: "≈ 210 MB · ≈ 4:10 (measured)".
    pub label: String,
    /// The space free in the folder the file is going to, when that can be
    /// asked.
    pub free_bytes: Option<u64>,
    /// Whether the file will fit there with room to write it: the sheet
    /// refuses before the first frame rather than at ninety-nine percent.
    pub fits: bool,
}

/// What an export reports.
#[uniffi::export(with_foreign)]
pub trait ExportListener: Send + Sync {
    /// How far along it is, 0 to 1.
    fn on_progress(&self, fraction: f32);
    /// Where the time went, as the engine measured it, in the lines the
    /// app's export report shows. Said once, before [`on_done`] or
    /// [`on_failed`] - an export that failed has a profile worth reading
    /// too, and it is the one that matters most.
    ///
    /// [`on_done`]: ExportListener::on_done
    /// [`on_failed`]: ExportListener::on_failed
    fn on_report(&self, report: String);
    /// It finished: the file written.
    fn on_done(&self, path: String);
    /// It failed or was cancelled (`cancelled` true).
    fn on_failed(&self, message: String, cancelled: bool);
}

/// How the file is made.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ExportSettings {
    /// The output's short side in pixels (720, 1080, 1440, 2160); the long
    /// side follows the canvas's shape.
    pub resolution: u32,
    /// Frames a second (24, 25, 30, 50, 60).
    pub fps: u32,
    /// Target bitrate in kbps; 0 lets the encoder choose for the size.
    pub bitrate_kbps: u32,
    /// H.265 rather than H.264.
    pub hevc: bool,
    /// The file to write.
    pub output: String,
}

/// The size of an export at `resolution` for a `width` by `height` canvas.
#[uniffi::export]
pub fn export_size(width: u32, height: u32, resolution: u32) -> Vec<u32> {
    let short = width.min(height).max(1);
    let scale = f64::from(resolution.max(2)) / f64::from(short);
    vec![
        even((f64::from(width) * scale).round() as u32),
        even((f64::from(height) * scale).round() as u32),
    ]
}

/// A bitrate for a frame size and rate that looks right on a phone: about
/// what phone cameras record at, kept inside gold-standard ranges for
/// each resolution - H.264 4K at 30 fps lands at about 25 Mbps, inside
/// the 13-34 Mbps that 4K needs; HEVC gets about three fifths of that.
#[uniffi::export]
pub fn suggested_bitrate_kbps(width: u32, height: u32, fps: u32) -> u32 {
    let pixels = f64::from(width) * f64::from(height);
    // a tenth of a bit a pixel a frame: about 12 Mbps for 1080p at 30 fps,
    // about 25 Mbps for 4K at 30 fps - inside gold-standard 4K ranges
    let per_frame = pixels * 0.1;
    ((per_frame * f64::from(fps.max(1)) / 1000.0) as u32).clamp(1000, 120_000)
}

/// Starts writing `session`'s timeline as `settings` say. Refused while
/// another export runs.
#[uniffi::export]
pub fn start_export(
    session: Arc<MobileSession>,
    settings: ExportSettings,
    listener: Arc<dyn ExportListener>,
) -> Result<(), MobileError> {
    let context = context()?;
    let job = context
        .exporter
        .begin()
        .map_err(|_| MobileError::Busy("An export is already running.".to_owned()))?;
    let request = session.export_request(&settings)?;
    std::thread::Builder::new()
        .name("capcraft-export".to_owned())
        .spawn(move || {
            let cancel = job.cancel_handle();
            let mut last = -1.0f32;
            let (result, profile) =
                concat_host::export::run_profiled(&request, None, &cancel, |progress: Progress| {
                    let fraction = fraction_of(progress);
                    if fraction - last >= 0.005 || fraction >= 1.0 {
                        last = fraction;
                        listener.on_progress(fraction);
                    }
                });
            // Where the time went: the log line, the file that outlives the
            // app, and the sheet's report all say it.
            let report = profile.table();
            log::info!("export: {}", profile.summary());
            if let Ok(context) = context() {
                // Kept, so the next export can say what it will cost from
                // what this one took rather than from a rule of thumb.
                if let Ok(mut last) = context.last_export.lock() {
                    *last = Some(profile.clone());
                }
                match save_report(&context.dirs.data, &report) {
                    Ok(path) => log::info!("export: report at {}", path.display()),
                    Err(error) => log::warn!("export: could not keep the report: {error}"),
                }
            }
            listener.on_report(report);
            match result {
                Ok(path) => listener.on_done(path),
                Err(message) => listener.on_failed(message, job.cancelled()),
            }
            drop(job);
        })
        .map_err(|error| MobileError::Failed(format!("could not start the export: {error}")))?;
    Ok(())
}

/// The last export's report, as it was kept: the stages, the per-frame
/// cost, the quarters and the encoder of the export the app ran last, so a
/// slow one can be read after the sheet has closed. `None` when no export
/// has finished on this phone yet.
///
/// Android 10 has no `ApplicationExitInfo` to read a death back out of (it
/// arrived in API 30); this is the app's own record, and it needs no API.
#[uniffi::export]
pub fn last_export_report() -> Option<String> {
    let context = context().ok()?;
    let folder = context.dirs.data.join("reports");
    let newest = std::fs::read_dir(&folder)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "txt"))
        .max()?;
    std::fs::read_to_string(newest).ok()
}

/// Writes `report` under `data/reports/`, one file an export, and keeps the
/// newest twenty.
fn save_report(data: &std::path::Path, report: &str) -> std::io::Result<std::path::PathBuf> {
    let folder = data.join("reports");
    std::fs::create_dir_all(&folder)?;
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let path = folder.join(format!("export-{seconds:010}.txt"));
    std::fs::write(&path, report)?;
    // The oldest go: a phone that has exported a hundred videos should not
    // be carrying a hundred reports.
    let mut kept: Vec<std::path::PathBuf> = std::fs::read_dir(&folder)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "txt"))
        .collect();
    if kept.len() > 20 {
        kept.sort();
        let excess = kept.len() - 20;
        for path in kept.drain(..excess) {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(path)
}

/// What an export of `session` at `settings` is likely to cost, before it
/// is started: the file's size from the bitrate and the cut's length, and
/// the time from the last export the engine measured on this phone - or, the
/// first time, from a conservative rule of thumb. [`ExportEstimate::measured`]
/// says which.
#[uniffi::export]
pub fn export_estimate(
    session: Arc<MobileSession>,
    settings: ExportSettings,
) -> Result<ExportEstimate, MobileError> {
    let request = session.export_request(&settings)?;
    let last = context()
        .ok()
        .and_then(|context| context.last_export.lock().ok().and_then(|last| *last));
    let estimate = concat_host::export::estimate_request(&request, last.as_ref());
    // The folder the file is written into, not the one it is published to:
    // that is where the bytes land while the export runs.
    let folder = std::path::Path::new(&settings.output)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let free = folder.and_then(concat_host::export::free_bytes);
    // No answer is not a refusal: a phone that will not say how full it is
    // gets the export, and fails where it fails.
    let fits = free.is_none_or(|free| concat_host::export::fits(&estimate, free));
    Ok(ExportEstimate {
        bytes: estimate.bytes,
        seconds: estimate.seconds,
        render_seconds: estimate.render_seconds,
        frames: estimate.frames,
        measured: estimate.measured,
        label: estimate.line(),
        free_bytes: free,
        fits,
    })
}

/// Asks the running export to stop.
#[uniffi::export]
pub fn cancel_export() {
    if let Ok(context) = context() {
        context.exporter.cancel();
    }
}

/// Drawing the frames is most of the work; mixing the sound and the mux
/// the rest. The stages are the engine's own names for them.
fn fraction_of(progress: Progress) -> f32 {
    let done = if progress.total > 0 {
        (progress.frame as f32 / progress.total as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    match progress.stage {
        "rendering" => done * 0.9,
        "mixing audio" => 0.9 + done * 0.06,
        "muxing" => 0.96 + done * 0.04,
        // a stage this bridge does not know: hold where drawing ended
        _ => 0.9,
    }
}

impl MobileSession {
    /// The engine's request for `settings`, made under the session's lock.
    pub(crate) fn export_request(
        &self,
        settings: &ExportSettings,
    ) -> Result<concat_export::ExportRequest, MobileError> {
        self.with_session(|session| {
            let canvas = session.settings();
            let size = export_size(canvas.width, canvas.height, settings.resolution);
            let (width, height) = (size[0], size[1]);
            // Gold standard for file export: VBR keeps quality up on busy
            // scenes and bits down on calm ones - the phone's MediaCodec
            // encoder runs VBR anyway, and the software encoders honour it.
            let rate_mode = RateMode::Vbr;
            // HEVC needs about three fifths of H.264's bitrate for the same
            // picture at 4K (gold standard: H.264 13-34 Mbps, HEVC
            // 12-20 Mbps at 4K 30 fps); a caller-supplied bitrate wins.
            let bitrate = if settings.bitrate_kbps > 0 {
                settings.bitrate_kbps
            } else {
                let base = suggested_bitrate_kbps(width, height, settings.fps);
                if settings.hevc {
                    base * 3 / 5
                } else {
                    base
                }
            };
            let spec = ExportSpec {
                output: settings.output.clone(),
                crf: 20,
                preset: "veryfast".to_owned(),
                codec: if settings.hevc {
                    VideoCodec::Hevc
                } else {
                    VideoCodec::H264
                },
                ten_bit: false,
                rate_mode,
                bitrate_kbps: bitrate,
                color_range: Default::default(),
                hdr: false,
            };
            let titles = context()
                .map(|context| {
                    context
                        .titles
                        .clips(session.project(), width, height)
                        .into_iter()
                        .map(|title| title.clip)
                        .collect()
                })
                .unwrap_or_default();
            let mut request = concat_host::export::request(session, &spec, titles);
            crate::background::add(
                session.project(),
                &canvas,
                std::path::Path::new(session.path()),
                &mut request.clips,
            );
            request.width = width;
            request.height = height;
            request.rate_num = i64::from(settings.fps.max(1));
            request.rate_den = 1;
            Ok(request)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Progress, fraction_of};

    fn at(frame: i64, total: i64, stage: &'static str) -> f32 {
        fraction_of(Progress {
            frame,
            total,
            stage,
        })
    }

    /// The ring moves with the frames drawn, then steps through the sound
    /// and the mux, and never goes backwards.
    #[test]
    fn progress_follows_the_engines_stages() {
        assert_eq!(at(0, 300, "rendering"), 0.0);
        assert!((at(150, 300, "rendering") - 0.45).abs() < 1e-6);
        let steps = [
            at(299, 300, "rendering"),
            at(0, 300, "mixing audio"),
            at(300, 300, "muxing"),
        ];
        assert!(steps.windows(2).all(|pair| pair[0] <= pair[1]), "{steps:?}");
        assert_eq!(steps[2], 1.0);
    }
}
