# veyron-plugins

Plugins for the [Veyron](https://github.com/veyron-core/veyron) plugin
kernel.

## Plugins

| Plugin | Path | Permissions | Description |
|---|---|---|---|
| `ping-pong` | `plugins/ping-pong-rs/` | none | Minimal example plugin that responds to ping actions. |
| `network` | `plugins/network/` | `PERMISSION_NETWORK` | Outbound HTTP for plugins/kernel via one `http_request` action. HTTP-only v1 (no WebSocket). See `plugins/network/README.md`. |
| `ai` | `plugins/ai/` | `PERMISSION_NETWORK` | Provider-agnostic LLM chat completion (`chat_completion`) for anthropic + openai-compatible providers. Routes through `network`'s gated `http_request`, so declares `network` itself (T-19). See `plugins/ai/README.md`. |
| `database` | `plugins/database/` | `PERMISSION_STORAGE` | Per-caller-namespaced KV + raw SQL storage over SQLite, five `db_*` actions. See `plugins/database/README.md`. |
| `tts` | `plugins/tts/` | `PERMISSION_NETWORK` | Text-to-speech via `tts_synthesize` + `tts_voices`: in-process local ONNX engine (sherpa: Kokoro/Piper, fully offline) + cloud providers (openai, elevenlabs) routed through `network`'s gated `http_request` (declares `network`, T-19). See `plugins/tts/README.md`. |
| `stt` | `plugins/stt/` | `PERMISSION_NETWORK` | Speech-to-text via `stt_transcribe` + `stt_models`: in-process local ONNX engine (sherpa: zipformer/whisper, fully offline) + cloud provider (openai audio API) routed through `network`'s gated `http_request` (declares `network`, T-19). See `plugins/stt/README.md`. |

## Registry

`registry.json` is a slug-keyed v2 map: a root `meta` block (`apiVersion`,
`lastUpdated`) plus a root `revoked` list, then one entry per plugin slug. Each
slug entry carries `name`, `description`, `category`, `tags`, `status`, and
`source_url`, plus a `versions` map whose semver keys hold per-version delivery
metadata — an absolute `archive_url` into the `dist/<slug>/versions/<version>/`
hierarchy, `sha256`, `signature`, and the kernel compatibility range.
Permissions are not stored in the registry (execution metadata lives in each
plugin's manifest). The `dist/` tree is hierarchical: `dist/<slug>/latest.json`
(newest registered version), `dist/<slug>/assets/`, and
`dist/<slug>/versions/<version>/` (binary + source zips, a `plugin.json` browse
copy, `checksum.sha256`, `signature.sig`). A plugin only gets an entry once
it's packaged and released via `scripts/package.sh` — see each plugin's own
README for its current status.
