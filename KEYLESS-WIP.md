# Keyless transcript checkpoint — WORK IN PROGRESS

This is an unfinished source checkpoint, not a release or a ready-to-use keyless app.

## Saved implementation

`src/transcript.rs` contains standalone local code for:
- SRT/WebVTT parsing, plain TXT reading, and JSON segments/real word timestamps.
- Bounded text-rule suggestions with Best/Funny/Hype/Tips focus and keyword search.
- Complete-caption-block cut validation, without inventing timing.
- SRT/ASS output and Clean/Bold/Boxed/Karaoke caption rendering with size/position controls. Karaoke falls back to static captions without actual word timing.

The new module has no HTTP/inference client or API-key handling.

## Not yet integrated or verified

The application's existing main/media/UI files are still the released v0.3.0 baseline and do not call this new module. Import/suggestion routes, direct local-video exports, UI changes, migration away from the old optional Groq path, and new automated/native Windows tests still need integration. No claim is made that this checkpoint compiles the new module or passes its tests.

Do not treat the old optional cloud workflow as the completed keyless workflow. No stored credentials were accessed, revoked or deleted by this checkpoint. No API keys, recordings, build caches or development-environment files are included.

## Next steps

1. Wire a size-limited, loopback-authenticated transcript-import API and preserve project/video association.
2. Add local suggestion review and exact-boundary exports with read-only source references.
3. Replace the cloud/key UI with transcript import, search, style controls and explicit local rendering.
4. Add parser, boundary, resource-limit, caption-rendering and no-inference-network tests.
5. Run Windows/Linux verification before merging into main or publishing a new EXE.

The working main branch and v0.3.0 release must stay unchanged until the replacement is ready.
