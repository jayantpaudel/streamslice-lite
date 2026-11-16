# Validation and release gates

This repository is a reconstruction from code/design preserved in the conversation. The original 0.2.1 temporary workspace/archive could not be recovered. Historical pass counts are not copied as evidence for this source.

## Required checks

- [x] Fresh Rust unit tests: 19 passed locally on the reconstructed source.
- [x] Linux integration: 13 checks passed locally; real codecs, local source, concurrent Copy/720p/1080p exports, source FPS, audio, captions, privacy, cancellation and shutdown.
- [ ] Native Windows CI: standalone executable, verified portable helper installation, same synthetic integration checks and downloadable artifact.
- [ ] Independent trial on the intended Windows 10 / i3 / 12 GB laptop.
- [ ] User-authorized real Twitch and real provider/model/quotas test; requires the user's source and key and is not performed without consent.

A successful CI build is not code signing, an antivirus/security audit, a visual/ASR accuracy benchmark or certification on every PC. Do not call an unfinished or failed workflow production-ready. Read actual run logs and commit SHAs.
