# Tool Recap Rust

Standalone portable Windows application for automated video recap production, built primarily in Rust.

The original `longthao9820-alt/Tool-REcap` repository is used only as a behavioral reference and is not modified.

## Workflow

- Select one supported video, or one folder.
- Folder mode scans **direct children only**. It never recurses into subfolders and ignores non-video files.
- Detected episodes are shown in a naturally sorted queue.
- Start runs episodes sequentially: **Analyze → Prepare narration/voice/video → Render**.
- Rendering begins automatically after preparation.
- Stop cancels the active job and kills its FFmpeg process tree.
- Completion, Stop, and failure all restore the UI so Start can be used again.

Supported inputs: MP4, MKV, MOV, AVI, WebM, M4V, and TS.

## VoiceStudio

VoiceStudio is the mandatory upstream English voice system. This project pins the reviewed `debpalash/VoiceStudio` release **v0.5.3** rather than following `main`.

The managed local runtime uses:

- `/system/info` readiness
- `/v1/audio/voices` for voice discovery
- `/v1/audio/speech` for preview and narration
- `/v1/audio/transcriptions` for source transcription
- `/models/install` and `/models/install/status` for first-use model installation progress
- `/engines/select` to select the installed Faster Whisper ASR model

The portable build freezes VoiceStudio into a self-contained Windows backend. Users do not install Python, packages, or VoiceStudio. To keep the verified portable release within GitHub Releases' single-asset limit, the bundled VoiceStudio runtime uses the official CPU builds of its pinned PyTorch 2.8 stack; this does not replace VoiceStudio or change its API/model system. NVIDIA RTX remains mandatory for the FFmpeg video-render pipeline. Large model weights are deliberately excluded from the initial package, download on first use into `data/models`, and are reused later.

A future VoiceStudio pin must pass the portable workflow's boot/API contract smoke test before publication. Failed or incompatible upgrades therefore do not replace the last working release.

## NVIDIA RTX rendering

Bundled FFmpeg is probed at runtime. Start is enabled only when:

1. `h264_nvenc` is present, and
2. a real short H.264 NVENC hardware encode succeeds.

CUVID/NVDEC decoding is selected when the actual source codec and driver support it. If hardware decode fails for a particular source, only decoding falls back to software; H.264 NVENC remains mandatory for output.

The user does **not** install CUDA Toolkit. A supported NVIDIA GeForce RTX GPU and NVIDIA driver are still required.

## Portable distribution

The verified release layout is:

```text
Tool-Recap-Rust/
  tool-recap-rust.exe
  tool-recap-updater.exe
  runtime/
    ffmpeg/bin/ffmpeg.exe
    ffmpeg/bin/ffprobe.exe
    ffmpeg/bin/ffplay.exe
    voicestudio/version.json
    voicestudio/backend/...
  data/                       # preserved by updates
    settings.json
    models/
    jobs/
    logs/
    updates/
```

There is no installer and no user-side Rust, Python, FFmpeg, VoiceStudio, CUDA Toolkit, or package-manager setup.

## Updates

The application checks public GitHub Releases from this repository. No private GitHub credentials or PATs are embedded.

A newer release shows **Update and restart** and **Later**. The app downloads the portable ZIP and SHA-256 sidecar, verifies it, extracts to staging, and hands off to a small Rust updater. The updater backs up every replaced file, skips `data/`, rolls back partial application failures, and restarts the app.

Settings, API configuration, downloaded models, voice data, and user data remain in `data/` and are preserved.

## UI/UX

The interface follows UI/UX Pro Max guidance: production-oriented visual hierarchy, professional dark Windows styling, accessible contrast, explicit runtime status, visible queue stages and progress, reliable disabled/enabled control states, and an in-app completion notification with a visible **×** dismiss control. A Windows completion toast is also shown.

The executable/window/taskbar identity uses an original AI + film frame/play + voice waveform icon embedded directly in the Rust binaries.

## Build and validation

PR checks are intentionally targeted:

```powershell
./scripts/verify-source.ps1
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo build --workspace --release
```

The heavy integration build is separated into **Portable Windows build**. It runs only manually or for release tags and:

- downloads pinned FFmpeg 9.0.1 and verifies the vendor SHA-256;
- checks out VoiceStudio v0.5.3, resolves its frozen dependencies, builds the PyInstaller backend, boots it, and checks all API paths this app uses;
- assembles the portable folder;
- runs `tool-recap-rust.exe --self-check`;
- produces the release ZIP plus SHA-256 file.

This avoids repeatedly rebuilding unchanged ML/runtime dependencies when existing evidence is still valid.

## License

Tool Recap Rust is AGPL-3.0-only because the portable application integrates VoiceStudio under that license. See `LICENSE`, `LICENSE-NOTICE.md`, and `THIRD_PARTY_NOTICES.md`.

Downloaded model weights retain their upstream terms and are not redistributed by this repository.
