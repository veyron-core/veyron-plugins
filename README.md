# veyron-plugins

Plugins for the [Veyron](https://github.com/veyron-core/veyron) plugin
kernel.

## Naming: vynkor

Veyron is being renamed **vynkor** ("veyron core" contracted) — the kernel
and every sibling repo, eventually. **New code and docs in this repo use
`vynkor`**; keep "Veyron" only when referring to the historical name or a
rename in progress. The `vyn` binary name stays `vyn`. Stable identifiers —
`plugin_id` slugs, binary names, env-var names (`*_PLUGIN_*`), permission
strings — are protocol/config surfaces and keep their current spellings
even when prose says vynkor.

## Plugins

| Plugin | Path | Permissions | Description |
|---|---|---|---|
| `ping-pong` | `plugins/ping-pong-rs/` | none | Minimal example plugin that responds to ping actions. |
| `network` | `plugins/network/` | `PERMISSION_NETWORK` | Outbound HTTP for plugins/kernel via one `http_request` action. HTTP-only v1 (no WebSocket). See `plugins/network/README.md`. |
| `ai` | `plugins/ai/` | `PERMISSION_NETWORK` | Provider-agnostic LLM chat completion (`chat_completion`) for anthropic + openai-compatible providers. Routes through `network`'s gated `http_request`, so declares `network` itself (T-19). See `plugins/ai/README.md`. |
| `database` | `plugins/database/` | `PERMISSION_STORAGE` | Per-caller-namespaced KV + raw SQL storage over SQLite, five `db_*` actions. See `plugins/database/README.md`. |
| `tts` | `plugins/tts/` | `PERMISSION_NETWORK`, `PERMISSION_AUDIO_STREAM` | Text-to-speech via `tts_synthesize` + `tts_voices` + `tts_speak`: in-process local ONNX engine (sherpa: Kokoro/Piper, fully offline) + cloud providers (openai, elevenlabs) routed through `network`'s gated `http_request` (declares `network`, T-19); `tts_speak` streams Opus `AudioStreamChunk`s to a peer (D-12). See `plugins/tts/README.md`. |
| `stt` | `plugins/stt/` | `PERMISSION_NETWORK`, `PERMISSION_AUDIO_STREAM`, `PERMISSION_EVENT_PUBLISH` | Speech-to-text via `stt_transcribe` + `stt_models` + `stt_listen_start`/`stt_listen_stop`: in-process local ONNX engine (sherpa: zipformer/whisper, fully offline) + cloud provider (openai audio API) routed through `network`'s gated `http_request` (declares `network`, T-19); the listen actions stream PCM in and publish a `stt_text` event (D-12). See `plugins/stt/README.md`. |
| `secrets` | `plugins/secrets/` | `PERMISSION_SECRETS` | Encrypted credential/API-key vault (`secret_get`/`secret_set`/`secret_delete`/`secret_list`), ChaCha20-Poly1305 per-caller `.vault` files, master key via `SECRETS_PLUGIN_MASTER_KEY`. `ai`/`tts`/`stt` resolve provider keys vault-first with env-var fallback. See `plugins/secrets/README.md`. |
| `gated-write` | `plugins/gated-write/` | — | Reference impl of the D-09 confirmation gate: risky file write split into `request_write` (any caller, `requires_confirmation`) + `confirm_write` (allowlisted callers only), writes confined to a data dir. See `plugins/gated-write/README.md`. |
| `sync` | `plugins/sync/` | `PERMISSION_STORAGE`, `PERMISSION_EVENT_PUBLISH` | Host-side sync state primitive (D-13): versioned SQLite KV + `sync_get_snapshot`/`sync_get`/`sync_set`/`sync_del`, publishes `sync.delta` events on every mutation. |
| `sync-client` | `plugins/sync-client/` | `PERMISSION_SCHEDULER`, `PERMISSION_IPC_SEND` | Client-side mirror + heartbeat scheduler (D-13): subscribes to `sync.delta`, pulls `sync_get_snapshot` on (re)connect, pushes heartbeats via `sync_set` on a timer. |
| `notify` | `plugins/notify/` | `PERMISSION_NOTIFY` | Desktop/system notifications via host binaries — `notify-send` (libnotify), `wall`, `espeak`; argv-only spawn, never a shell. v0.2: `speak: true` озвучка через `tts`-плагин + `silent: true` inbox (`notify_list`/`notify_mark_read`/`notify_delete`). See `plugins/notify/README.md`. |
| `notes` | `plugins/notes/` | `PERMISSION_STORAGE`, `PERMISSION_EVENT_PUBLISH` | Note CRUD as a thin schema layer over `database` (`note:<id>` JSON docs, atomic id counter, tag filter/pagination), publishes `plugin.notes.changed`. Callers need no storage permission — `notes` holds it (T-19). See `plugins/notes/README.md`. |
| `calendar` | `plugins/calendar/` | `PERMISSION_STORAGE`, `PERMISSION_EVENT_PUBLISH`, `PERMISSION_NOTIFY` | Event CRUD + opt-in reminders (`remind_before_ms`): timer scan fires once at-most (`late` flag after downtime), publishes `plugin.calendar.changed`/`.due`, best-effort `notify_send`; rescheduling resets the fired flag. See `plugins/calendar/README.md`. |

Writing a new plugin? Start with [`docs/PLUGIN_AUTHORING.md`](docs/PLUGIN_AUTHORING.md) —
the single-reader loop / RPC-proxy pattern, kernel routing facts (T-19/T-04),
and the fake-kernel test harness.

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
