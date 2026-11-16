# Prioritized enhancements and research

The working clipping workflow takes priority over feature count. This is a scoped engineering backlog, not a claim to have researched every possible enhancement.

1. **Release evidence:** native Windows 10 hardware tests, signed builds, stable GitHub Releases, dependency/license/SBOM review, source-to-binary provenance. Test forced-exit cleanup using Windows Job Objects without capturing the user's browser/Explorer processes.
2. **Measured resource budgets:** record total engine + Streamlink + decoder/encoder + browser RAM, per-core CPU, disk usage and export latency for 720p/1080p30/60 on representative older i3 hardware. Tune lookahead/encoder speed from measurements rather than labels.
3. **Selection quality:** benchmark human-rated complete hooks/setup/payoffs; distinguish transcript-only inference from visual understanding. Explore privacy-preserving local audio/event triggers and opt-in chat signals to reduce cloud calls. Do not equate model scores with virality.
4. **Caption accuracy:** English/Hindi/Urdu word-timing and named-entity benchmarks on licensed/consented audio, confidence-aware rejection, editable captions and waveform review, font coverage/RTL/line-wrap tests. Test boundary alignment at 23.976/29.97/59.94 FPS.
5. **Framing / colour:** explicit crop preview and movable safe regions, optional face tracking only after CPU/privacy evaluation, correctly detected HDR-to-SDR tone mapping, audio true-peak/two-pass normalization options, source-resolution-aware output recommendations.
6. **Usability:** native file picker, structured redacted diagnostics, global hotkey, timeline adjustments and re-encode of existing AI cuts without paying for another model call.
7. **Provider resilience:** documented model-availability health checks, user-controlled provider selection, explicit spending budgets where providers support them, contract tests with mock responses. No automatic paid fallback or security bypasses.
8. **Regression defence:** golden synthetic clips, full output decode checks, caption frame comparisons, error/timeout/disk-full/reconnect/queued-cancel tests, untrusted metadata and ZIP fuzzing, short-lived CI tokens and pinned action/dependency revisions.

External social publishing, if added, needs separate explicit user confirmation for each publishing scope; no automatic posting is enabled here.
