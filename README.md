# veyron-plugins

Plugins for the [Veyron](https://github.com/veyron-core/veyron) plugin
kernel.

## Plugins

| Plugin | Path | Permissions | Description |
|---|---|---|---|
| `ping-pong` | `plugins/ping-pong-rs/` | none | Minimal example plugin that responds to ping actions. |
| `network` | `plugins/network/` | `PERMISSION_NETWORK` | Outbound HTTP for plugins/kernel via one `http_request` action. HTTP-only v1 (no WebSocket). See `plugins/network/README.md`. |
| `ai` | `plugins/ai/` | none | Provider-agnostic LLM chat completion (`chat_completion`) for anthropic + openai-compatible providers. Routes through `network`, declares no permissions itself. See `plugins/ai/README.md`. |
| `database` | `plugins/database/` | `PERMISSION_STORAGE` | Per-caller-namespaced KV + raw SQL storage over SQLite, five `db_*` actions. See `plugins/database/README.md`. |
| `tts` | `plugins/tts/` | none | Text-to-speech via `tts_synthesize` + `tts_voices`: in-process local ONNX engine (sherpa: Kokoro/Piper, fully offline) + cloud providers (openai, elevenlabs) routed through `network`. Declares no permissions itself. See `plugins/tts/README.md`. |
| `stt` | `plugins/stt/` | none | Speech-to-text via `stt_transcribe` + `stt_models`: in-process local ONNX engine (sherpa: zipformer/whisper, fully offline) + cloud provider (openai audio API) routed through `network`. Declares no permissions itself. See `plugins/stt/README.md`. |

## Registry

`registry.json` lists released/published plugin archives (marketplace
metadata: version, archive URL, sha256). A plugin only gets an entry once
it's packaged and released — see each plugin's own README for its current
status.
