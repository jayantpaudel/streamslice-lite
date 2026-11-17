# StreamSlice Lite: Claude Code handoff

## Start here

This branch is `wip/keyless-transcripts`, a public source checkpoint based on commit `e82455d08e29b1027e29ba5ecad36d012a8e5e13`. Read `KEYLESS-WIP.md` before changing code. The released baseline is v0.3.0; this checkpoint is not a new release.

**Crucial status:** `src/transcript.rs` is new, standalone, unfinished code. `src/main.rs` still declares `mod ai`, not `mod transcript`, and the existing main/media/UI remain the v0.3.0 implementation. The new module is not wired into the executable and has not been verified by the existing test suite. Do not interpret a successful baseline build as validation of this module. Older chat test counts do not validate this checkpoint.

## Latest product requirements

- Make the new workflow require **no AI keys, Groq account, external inference service, or paid subscription**. Base suggestions on reading imported transcripts locally.
- Import SRT/WebVTT or timed JSON for synchronized captions and cuts. Plain TXT is for reading/search/export only; do not fabricate timing. Real word timestamps may drive Karaoke; segment-only data must fall back honestly.
- Keep complete source imagery: proportional fit and padding into **9:16 or 1:1**, never spatial cropping or stretching. Original Copy is explicitly unchanged-source output.
- Keep processing practical on modest Windows/i3 hardware: bounded input/history, one background render, bounded queue, lazy previews. No mandatory GPU, large local language model, Python installation, or model downloads for the transcript-reading workflow.
- Preserve existing source files, recordings and exports. The old optional cloud implementation is still present in the baseline and must be replaced or isolated before claiming the application is keyless. Legacy credentials should not be loaded or used by offline jobs; do not delete or revoke credentials as an incidental migration.
- The user wanted useful free features inspired by Mani Clips, ClipIt, OpenClip and Clips Kitty, not a misleading claim of complete parity. Implement original functionality; check licenses before reusing upstream code or assets. Clips Kitty's app is AGPL-3.0-or-later, not a source to copy into this MIT app without license compliance.

## Repository map

- `src/main.rs`: local Axum dashboard, application state, routes, settings, queue setup.
- `src/media.rs`: capture/buffer, pinned jobs, FFmpeg processing and exports.
- `src/ai.rs`: old optional Groq workflow; currently still compiled by main.
- `src/vault.rs`: old optional Windows credential storage.
- `src/setup.rs`: pinned/hash-verified helper downloads.
- `src/transcript.rs`: new local parser, suggestions and caption rendering to integrate.
- `ui/index.html`: compiled-in browser dashboard, currently the old UI.
- `scripts/smoke.py`, `.github/workflows/ci.yml`, `VALIDATION.md`: baseline verification machinery.

## Suggested implementation sequence

1. Review/compile/test the standalone module. Add parser tests for BOM/CRLF, VTT metadata, malformed and overlapping cues, bounds, and consistent limits across formats. Audit floating-point/non-finite inputs, query/focus validation and subtitle escaping.
2. Add a size-limited, loopback-authenticated transcript import API. Bind each imported document to the correct source/project; freeze the document and preferences per queued job.
3. Add local suggestion review, keyword search, transcript view/download/copy, and explicit exports. Label scores as text-rule matches, not AI understanding or virality predictions.
4. Support direct local video references without copying whole long files into memory. Validate source duration, read-only handling, changed/disconnected files, cut boundaries and cancellation. The baseline probe currently limits duration to 240 seconds; audit this for full-file projects rather than silently analyzing only a short prefix.
5. Expose Clean/Bold/Boxed/Karaoke, 75/100/125% sizes and top/center/bottom placement. Reuse ASS for burned captions and sidecars; never claim old burned videos are restyled without a new render.
6. Replace cloud/key controls and inference code paths in the new workflow. Network access for explicitly requested helper installation or Twitch capture is distinct from inference; do not claim those operations are offline.
7. Add end-to-end tests proving no provider credentials or inference network requests are needed. Render presets/positions, verify all four source corners in both output shapes (including anamorphic input), check caption timing and retain exports across cancellation/restart.
8. Run native Windows and Linux checks before merging or publishing a new EXE. Keep main and the released v0.3.0 artifact usable while work continues here.

## Development

```sh
cargo test --locked
cargo build --locked
python scripts/smoke.py --exe target/debug/streamslice-lite
```

Use stable Rust satisfying Cargo.toml and FFmpeg/ffprobe for Linux development; Windows development uses normal MSVC build tools. Test with synthetic fixtures. No real user recordings or provider keys are included. Automatic speech-to-text from raw audio is not provided by a transcript reader; any optional future offline speech recognizer needs explicit model/CPU/storage/license consideration and must not be represented as already implemented.
