# CapCraft Android — 50 things to do next build

Core/root = the 11-crate Rust engine (`concat-*`). UI/UX = the Kotlin + Jetpack Compose app.
Every item is concrete, traceable to current repo state or to internet research, and sized for a single build cycle.

Status at branch point `8baad4c` (`arena/8aaa374e-capcraft-android`, on `31b4109`):
- Done: GPU adapter fix (#20), 4K gold-standard bitrates/VBR, MediaCodec 4K profile/level pinning, wgpu non-compliant adapter env escape hatch.
- Blocked: upstream PR conversion + issue comment reply (upstream token is read-only). See "Immediate next steps" below.

---

## A. Core / root (Rust engine) — 25

**A-1. mediaProcessing foreground service for Android 15+ exports** — `ExportService` starts with `FOREGROUND_SERVICE_TYPE_MEDIA_PROCESSING` + `FOREGROUND_SERVICE_MEDIA_PROCESSING` permission on API 35+; implements `Service.onTimeout` (stopSelf within seconds), 6 h / 24 h budget shared across mediaProcessing services, stops on done / fail / cancel; `dataSync` type on APIs ≤34. Only start from direct user interaction so the full six hours run after the app goes background.
*Research:* Android 15 foreground-service timeouts + `mediaProcessing` type (developer.android.com). Session next-build item #3.

**A-2. CONCAT_GPU override unit tests** — add to `concat-render/src/gpu/tests.rs`: (a) env override forces Vulkan|GL, (b) strict adapter pass then `ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER` retry ordering, (c) Vulkan→GL one-at-a-time fallback, (d) `compositor: drawing on ...` info log emitted.
*Research:* wgpu 30 `InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER` / `WGPU_*` env vars (docs.rs). Session next-build item #4.

**A-3. Export stall-retry unit test** — `concat-media/src/encode.rs` test: hardware encoder stalls past `ENCODER_PATIENCE`; export retries `again_in_software` via the `software_only` path and writes a complete, decodable file. `Error::Stalled` with `hardware=true` only for chip stalls; CPU stops and other errors fail again.
*Research:* `again_in_software` stall recovery already in `concat-export/src/lib.rs`; must stay preserved per the 4K constraint.

**A-4. 4K profile/level pin device matrix** — verify H.264 High@5.1 / HEVC Main@5.1 pin (≥3840×2160) opens on Pixel, Samsung, Xiaomi, Fire TV chips; log which attempt opened (AsAsked / Plain / component) per codec so the field can distinguish chip limits from settings bugs.
*Builds on:* commit `8baad4c` pinning.

**A-5. HEVC 10-bit P010 MediaCodec decode** — verify 10-bit HEVC decode on Android 13–16, P010 output frames feed the compositor's `Rgba16Float` resolve path without downconvert banding.
*Scope item from the 12-gate audit.*

**A-6. Rgba16Float + BT.2408 HDR compositing** — HDR timeline keeps light above white; SDR timeline conforms HDR clips via BT.2390 rolloff; resolve once at the end; no HDR file tagged from SDR frames outside the perf harness.

**A-7. 4K proxy playback** — generate down-scaled proxies for timeline scrubbing of 4K sources; full-res only at export; proxy cache invalidated on source edit or effect param change.

**A-8. Android 13–16 OS compliance** — scoped storage / MediaStore (API 33+), foreground-service restrictions per API level, targetSdk 36 behavior changes, no `BOOT_COMPLETED`-launched foreground services, no `SYSTEM_ALERT_WINDOW`-gated FGS starts.

**A-9. Reproducible builds (F-Droid)** — `RUSTFLAGS="--remap-path-prefix=$CARGO_HOME=/cargo -ffile-prefix-map=..."`, `SOURCE_DATE_EPOCH`, fixed `CARGO_TARGET_DIR`, NDK symlinked at a fixed path, `codegen-units=1`; byte-identical `arm64-v8a` lib from two differently placed checkouts (F-Droid publishes only byte-identical APKs).
*Research:* F-Droid Reproducible Builds docs; Terpoir #207 reproducible Rust core build.

**A-10. No prebuilt `.so` blobs in Git** — all native libs source-built; FFmpeg for phones built via `scripts/ffmpeg-mobile.sh` (LGPL, MediaCodec/VideoToolbox only, no GPL encoders); no unmeasured/placeholder device benchmark numbers committed.

**A-11. 16 KB ELF alignment** — `arm64-v8a` release `.so` page-aligned for 16 KB page devices (Android 15+); verify with `readelf -lW` segment alignment in CI.

**A-12. Zero build-host path leaks** — audit release `.so` strings for `/home/user`, `/tmp`, `CARGO_HOME`, absolute NDK paths; block release if any leak is found (`-ffile-prefix-map` / `--remap-path-prefix` covers most).

**A-13. Export bitrate estimate accuracy** — `suggested_bitrate_kbps()` (0.1 bits/px/frame, clamp 1000–120000 kbps) vs actual output size across 480p–4K, 24–60 fps, H.264/HEVC; adjust clamp or per-frame factor if actual drifts beyond ±30%.
*Done baseline:* commit `91cbf64` (~25 Mbps H.264 / ~15 Mbps HEVC at 4K30).

**A-14. QP caps on MediaCodec encoders** — `qp_i_max/qp_p_max/qp_b_max = 40` enforced in VBR plain mode so a chip that misjudges its budget still spends enough per picture; quality floor kept at 4K.

**A-15. Hardware-lead encode with software fallback** — `EncodeOptions { hardware: true, software_only: false, rate_mode: Vbr }` preserved; on chips lacking HEVC/H.264 hardware the export falls to Android's software component without failing; caller-supplied bitrate still wins over suggested.

**A-16. AV1 availability gating** — `av1_mediacodec` offered only where `encodable()` says the chip takes it; HEVC/H.264 fallback; export sheet hides AV1 when unavailable (few phones have AV1 hardware).

**A-17. Audio loudness normalization** — concat-media loudness: true peak + integrated loudness (LUFS), normalize the mix to −16 LUFS stereo, no clipping on export.

**A-18. On-device speech transcription** — concat-speech Whisper via concat-vision ONNX runtime, offline auto captions/lyrics; model bundled, AGPL-compatible; no remote transcription.

**A-19. Project validation before export** — concat-project commands validate clip ranges, track overlaps, speed curves; fail fast with an actionable error instead of dying mid-encode.

**A-20. Undo/redo transaction engine** — every editor command (trim, split, effect, reorder, property change) is a reversible transaction in `concat-project` history; undo stack persists across sessions.

**A-21. Render effect cache** — hash-based cache keys for rendered layers/effects; reuse on unchanged inputs; invalidate on source or parameter change; keyed by project content hash, not path.

**A-22. Frame buffer memory pool** — `concat-media/pool.rs` reuse of RGBA / RGBA64LE buffers across export frames to cut allocations on long 4K timelines and reduce jank.

**A-23. Export thread panic safety** — `catch_unwind` around the `capcraft-export` thread; any codec panic reports `on_failed` with a message instead of killing the app process.

**A-24. Cancellation between frames** — `cancel_export()` honored at each frame boundary; partial output reset/deleted so a redelivered export cannot append stale bytes to MP4/MKV output.

**A-25. Export CI smoke test** — headless emulator/device test: export 4K30 HEVC and H.264, assert the file plays back, duration matches, bitrate within estimate ±30%, hardware-lead where available.

---

## B. UI/UX (Kotlin + Jetpack Compose) — 25

**B-1. Export sheet redesign** — resolution (480p–4K) / fps (24–60) / bitrate chips, HEVC toggle, VBR default, estimated file size from `size_factor()`, last-used resolution/fps/codec remembered per project; caller bitrate override shown when set.

**B-2. Home screen quick actions** — AutoCut, Retouch, Photo tools, Remove background, Auto captions, Cover maker as primary tiles above recents (per README home layout); "New video" and "Edit photo" entry points kept.

**B-3. Timeline landscape scroll safety (390 dp)** — timeline never scrolls off the screen's side in 390 dp landscape; drag past the edge triggers overscroll, not clip loss (follow-up to issue #16 edge-drag scroll fix).

**B-4. Full localization + RTL** — all 825 keys translated; RTL layout mirroring in editor panels, dock, export sheet; test text expansion for German and Arabic without clipping.

**B-5. Dark theme only** — every panel uses `Palette`/`Theme.kt` colors; audit for hardcoded `#FFFFFF`/`#000000`; forced dark, follows system night mode.

**B-6. Material 3 bottom sheets** — all editor panels as `ModalBottomSheet`: centered drag handle, top-corner rounding (28 dp), scrim, system back + scrim tap close, content ends above the gesture-bar inset (per Material 3 bottom-sheet guidelines).

**B-7. TalkBack / accessibility** — localized `contentDescription` on every icon button and tile, `stateDescription` on toggles, `heading` semantics on section titles, `mergeDescendants` where image+text belong together, custom accessibility actions for overflow; test with TalkBack and Accessibility Scanner.
*Research:* Compose semantics / TalkBack best practices (contentDescription, onClickLabel, minimumTouchTargetSize, roles, traversal order).

**B-8. 48 dp touch targets** — `Modifier.minimumTouchTargetSize()` on timeline controls, chips, dock icons; 24 dp visuals stay 24 dp but tappable area expands to 48 dp.

**B-9. Baseline profiles** — `BaselineProfile.kt` covering app launch, export sheet open, timeline scroll, media picker grid; verified with Macrobenchmark on release builds.

**B-10. Compose scrub performance** — `remember` + `derivedStateOf` for playhead position; timeline scrub defers state reads; no per-pixel recomposition during drag; heavy work moved out of composition to ViewModel/coroutines.
*Research:* Compose performance (remember, derivedStateOf, defer state reads, lambda modifiers).

**B-11. Lazy lists with stable keys** — `LazyColumn`/`LazyGrid` with `key` + `contentType` for media picker, effects catalogue, stickers, fonts; precomputed thumbnails; no date formatting or image resizing inside item composables.

**B-12. Export progress ring** — 0→1 across rendering (90%), mixing audio (6%), muxing (4%) via `fraction_of()`; never backwards; completion shows done + share action.

**B-13. Export foreground notification** — ongoing notification with progress, cancel action, dedicated "Exporting video" channel; dismissed on done/fail; maps to the mediaProcessing FGS (A-1).

**B-14. Picture-in-picture** — preview continues in PiP while export runs in the background (API 31+); decoders released when the user returns to the app.

**B-15. Foldable / large-screen adaptive UI** — window size class; list-detail layout (project library left + editor right) at ≥600 dp width; navigation rail alternative; panels reflow, not just stretch.

**B-16. Haptic feedback** — `VibrationEffect` on timeline snap, trim-handle engage/disengage, cut transitions; per-event amplitude tuned so it confirms, not annoys.

**B-17. Undo Snackbar** — destructive actions (delete clip, remove effect, clear timeline) show an undoable Snackbar for 5 s; state restores exactly on tap.

**B-18. Resolution/fps persistence** — last export resolution, fps, codec, HEVC toggle remembered per project and restored when the sheet reopens.

**B-19. Gesture navigation insets** — `WindowInsets` handled for status bar and gesture bar; timeline and dock never sit under system bars; edge-to-edge on API 31+.

**B-20. Font scaling** — all `TextStyle`s in `sp`; test at the largest system font size; no clipped buttons or overlapping text at 2.00× scale.

**B-21. Contrast-safe clip row colors** — `TextClip`/`CaptionClip`/`EffectClip`/`FilterClip`/`StickerClip`/`AudioClip` checked ≥4.5:1 against Chrome/Panel; `Danger` reserved for delete actions only.

**B-22. Export error UI** — actionable messages ("Codec not supported on this device", "Storage full") with retry and a "Software-only" fallback toggle; never a silent failure.

**B-23. Recent projects with filmstrip thumbnails** — per-project filmstrip generated via `concat-media` scrub; recents grid sortable by date/name; empty state guides to "New video".

**B-24. Export to Movies + share sheet** — output written to MediaStore Movies via `DocumentFile`; completion offers the system share sheet (WhatsApp, Status, Messages…).

**B-25. Compose compiler metrics in CI** — enable the Compose compiler metrics report; track unstable composables (inline lambdas, non-skippable params) in editor panels; fix the top 5 per build.

---

## Cross-cutting constraints (standing)

1. Upstream contributions clean, source-built, F-Droid-compatible — no prebuilt `.so`, no placeholder benchmarks, reproducible `-ffile-prefix-map` / `--remap-path-prefix` builds.
2. 4K export must keep working — hardware-lead encode with software fallback, VBR default, QP caps, stall recovery (`again_in_software`) preserved by every encoder change.
3. Delivery: separate clean topic branches/PRs upstream + combined `arm64-v8a` release APK, patches, and reports in Google Drive (`1B5wUxRsq7xmjjX8LLM-VQ7VZDg_PuEfk`).
4. Keep `/home/user` clean of repo/build artifacts and credentials.

---

## Immediate next steps (blocked items from issue #20)

1. Upload the 3-commit patch `/opt/work/out/capcraft-issue20-gpu-fix-and-4k-export.patch` (18,777 bytes, `7a89d3f`+`91cbf64`+`8baad4c`) to Drive, superseding file `1Jikh-Zo1cvOt4f4DI6XsjakTDjlWcjPW` (old 14,300-byte 2-commit version).
2. User or upstream-write holder converts fork PR #1 (https://github.com/heavilyusingai/capcraft-android/pull/1) into an upstream PR on `artcraft-android/capcraft-android`.
3. Post `/home/user/issue20-reply.md` to https://github.com/artcraft-android/capcraft-android/issues/20#issuecomment-6867654460 (draft ready; upstream token is read-only so it must be posted by someone with write access).
4. Optionally trash superseded Drive rounds (Nov 12/13), keep the Nov 13 22:08 self-audited release and the Nov 14 handoff doc.
