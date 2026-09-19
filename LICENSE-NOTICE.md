# Tool Recap Rust — License Notice

Tool Recap Rust is released under **GNU AGPL-3.0-only**.

The portable package includes a separate, pinned VoiceStudio runtime. VoiceStudio v0.5.3 is also AGPL-3.0-only and its verbatim license and license notice are preserved under `runtime/voicestudio/`.

Downloaded model weights are separate works with their own terms. VoiceStudio v0.5.3's license notice states that its default `k2-fsa/OmniVoice` pretrained weights are **CC-BY-NC**, with separately licensed tokenizer/model components. Tool Recap Rust does not redistribute those weights; they are requested on first use and cached under `data/models`.

Users should review the model terms for their intended use.
