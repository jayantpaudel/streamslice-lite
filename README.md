# StreamSlice Lite

A Rust desktop engine for quality-first clipping on modest Windows PCs. Independent original software, **not Mani Clips or a feature-for-feature clone**.

## Get the app

The authoritative source and build history are here on GitHub, not temporary chat downloads. Open **Actions → Rust and native Windows verification → a successful run → StreamSlice-Windows-x64**. Extract the artifact and run `StreamSlice.exe`. Artifacts expire after 14 days; source and workflows remain in Git, so builds can be reproduced. Do not use a failed/incomplete run as a release.

1. Close any earlier StreamSlice instance.
2. Run the executable. It opens a local dashboard in your existing browser.
3. Click **Install / repair helpers** (about 200 MB once, at least 1.5 GiB disk free). SHA-256 is verified before extraction/execution. No admin, PATH change, Rust compiler or separate Python installation is needed.
4. Click **Test this PC**. It generates video, audio, captions and a playable MP4 locally; nothing is uploaded.
5. Enter a Twitch channel you own/have permission to capture, confirm rights, and Start. Wait 15–30 seconds for completed footage. Absolute local video paths simulate live input too.
6. Save manually, or add your own Groq key and explicitly allow candidate-audio upload for AI highlights/captions. The key stays in memory unless you explicitly enable Windows Credential Manager storage.

**This is a reconstruction, not the bit-identical earlier 0.2.1 archive. Previous chat test counts do not validate this source.** Current CI and `VALIDATION.md` are authoritative. No code-signing, physical Windows 10/i3 performance certification, real Twitch connectivity or real Groq-key test is claimed unless recorded explicitly.

## Quality / resource policy

- Default source: **Best/original**. Encoding cannot restore detail absent from the input.
- Quality: **CRF 17**, x264 medium, Lanczos scaling, up to **1080×1920**, preserving source motion up to 60 FPS.
- Balanced: CRF 21 / veryfast, up to 720×1280 and 30 source FPS. Slower input is not needlessly duplicated to 30 FPS.
- Original-shape output does not upscale small sources. Vertical fit adds padding; center crop can remove HUD/webcam content. No face tracking or AI super-resolution is claimed.
- Copy: no video re-encoding and original source detail; keyframe-approximate cuts/caption sync. Use encoded presets for precise cuts/burned captions.
- Encoded output: H.264/yuv420p + AAC/48kHz stereo, one-pass loudness normalization, fast-start MP4. H.264/AAC SDR input is recommended. HDR tone mapping/full arbitrary-codec support is not implemented.
- One media job runs at a time, at most three snapshots active/queued; source snapshots and the rolling buffer live on disk. The adaptive buffer retains up to ~3 minutes and shortens history at high bitrate, with a 256 MiB safety stop.
- Decoder/filter/encoder threads are constrained; on Windows helpers run below normal priority. No GPU or local AI model is required. CPU encoding still takes time and memory; no fixed total-RAM or zero-load promise is made. Browser/helper/OS memory is separate from the Rust engine.
- Capture reconnects are bounded, stalled capture is detected, progress/cancellation are available, finished exports are never automatically deleted, and a file lock prevents two instances clearing each other's work.

Keep several GB disk free. Use **Quit**, not just browser-tab close. Do not force-kill ongoing exports unless necessary. Large HD/high-FPS clips can take minutes on an old i3; choose Balanced or Copy when busy.

## Optional cloud AI

The speech API receives at most a 90-second sampled mono FLAC window; ranking receives its transcript. **The model does not see video** and can miss quiet/gameplay events. Real word timestamps are grouped into short caption cues when available; segment timestamps are a fallback. Clearly unreliable silence segments, malformed/out-of-bounds decisions, weak selections and overlapping recent AI clips are rejected/skipped. Review names, context, captions, cuts and framing. Scores do not predict virality.

Default models: `whisper-large-v3` and `llama-3.3-70b-versatile` via Groq's OpenAI-compatible API. Review current provider privacy, model availability, pricing and quota terms at <https://console.groq.com/>. Advanced model overrides: `GROQ_SPEECH_MODEL`, `GROQ_CHAT_MODEL`.

Consent must be granted each run and can be withdrawn. Requests already sent cannot be undone. Keys are never written to settings, echoed in status or inherited by helper subprocesses. Optional Windows Credential Manager storage is scoped to your account and this app-data folder; cloud consent still resets each run. Uncheck Remember for session-only entry; Remove key clears memory and requests deletion of this app’s stored credential. OS failures are reported, not silently replaced with plaintext storage. Other software running as you can still access your process memory.

The local UTC-day AI-window counter persists across restarts; default 12 windows/day. Usually two API requests/window. This is **not a financial spending cap** and cannot prove your account is free. HTTP 429 pauses AI; no automatic provider retries, paid fallbacks or billing changes. Automatic sampling is not continuous visual coverage and can miss events.

## Storage / security

Windows data: `%LOCALAPPDATA%\StreamSliceLite`; override with `STREAMSLICE_HOME` pointing to an empty dedicated folder. Source files are never modified. Temporary buffer/work is recycled; `exports` is never auto-deleted. Keep recordings, API keys and this data folder out of Git.

The local service binds only to 127.0.0.1, checks Host/Origin and a random per-run API token, has bounded request/response sizes, uses argument arrays instead of a shell, and sanitizes AI text before subtitle rendering. It is not a security-audited service: do not expose it through a tunnel/reverse proxy. Keep Windows updated/protected. Unsigned builds are not antivirus-certified; **never disable protection to run them**.

FFmpeg and portable Streamlink are separate helpers; Streamlink includes embedded Python. The engine is Rust, but the entire stack is not pure Rust. Original Rust application source is MIT licensed. Helpers retain their upstream licenses; they are downloaded directly, not bundled in this repository.

Pinned upstream assets:
- FFmpeg/Gyan 9.0.2 essentials: SHA-256 `60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba`, <https://github.com/GyanD/codexffmpeg/releases/tag/9.0.2>.
- Official Streamlink Windows 8.6.3-1 / Python 3.15 portable: SHA-256 `3b1f1470d4b135080a8107ca2917379ca3ff5acadbcdc215568b79eea1450497`, <https://github.com/streamlink/windows-builds/releases/tag/8.6.3-1>.

Manual overrides accept executable-file paths: `FFMPEG_PATH`, `FFPROBE_PATH`, `STREAMLINK_PATH`. Portable Streamlink needs all companion files. Changing a pinned upstream asset requires review and a newly verified hash, never silently trusting a mismatch.

## Development / verification

Install stable Rust (and the normal C++ Build Tools/SDK for MSVC development), then:

```sh
cargo test --locked
cargo build --locked
python scripts/smoke.py --exe target/debug/streamslice-lite
```

Linux development requires FFmpeg/ffprobe from its package manager. Python is a development-test dependency, **not the app runtime**. CI tests the real Windows executable and portable setup on a native Windows runner, including simultaneous capture and 1080p exports. Its artifact is built with a static MSVC CRT. No real Twitch/Groq calls or user recordings are used in synthetic checks.

Not implemented: social auto-posting, global OS hotkeys, a timeline editor, arbitrary long-video batch imports, a native file picker, face tracking, HDR conversion or guaranteed "best/viral" selection. These are not silently represented as completed features; see `ROADMAP.md`.
