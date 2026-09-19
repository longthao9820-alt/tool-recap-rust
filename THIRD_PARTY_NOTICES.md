# Third-party notices

## VoiceStudio

- Upstream: `debpalash/VoiceStudio`
- Pinned release: `v0.5.3`
- Runtime license: GNU AGPL-3.0-only
- Used API contract: `/system/info`, `/v1/audio/speech`, `/v1/audio/transcriptions`, `/v1/audio/voices`, `/models/install`, `/models/install/status`, and `/engines/select`.
- Release packages preserve VoiceStudio's verbatim license files.
- Model weights are not bundled and retain their own upstream terms.

## FFmpeg

- Windows build distributor: GyanD/codexffmpeg
- Pinned version: FFmpeg 9.0.1 essentials build
- The package script verifies the vendor-provided SHA-256 before staging binaries.
- The archive's license file is preserved at `runtime/ffmpeg/LICENSE.txt`.
- The selected Windows build exposes NVIDIA CUDA/CUVID/NVDEC/NVENC support.

All Rust dependencies retain their own licenses.
