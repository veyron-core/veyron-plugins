# veyron-plugins roadmap

Plugin ideas beyond what's shipped, and the order/dependencies between them.
Each plugin gets its own `plugins/<name>/ROADMAP.md` once work starts (see
`plugins/ai/ROADMAP.md`, `plugins/network/ROADMAP.md` for the pattern) — this
file is the cross-plugin picture only.

## Shipped

| Plugin | Path | Depends on | Notes |
|---|---|---|---|
| `ping-pong-rs` | `plugins/ping-pong-rs/` | — | example plugin, no real capability |
| `network` | `plugins/network/` | — | outbound HTTP, `PERMISSION_NETWORK`, SSRF-guarded |
| `ai` | `plugins/ai/` | `network` | LLM chat completion (anthropic/openai-compatible), zero permissions itself |

## Planned

Dependency order — each row can start once everything in "depends on" ships.

| Plugin | Purpose | Depends on | Permissions |
|---|---|---|---|
| `secrets` | encrypted credential/API-key vault (`secret_get`/`secret_set`) | — | `PERMISSION_SECRETS` (new) |
| `filesystem` | sandboxed file read/write + read-only browse (`ls`/`cat` equivalents: `fs_list`/`fs_read`) — no exec, no shell | — | `PERMISSION_FILES_READ`/`PERMISSION_FILES_WRITE` (existing) |
| `stt` | speech-to-text | — | `PERMISSION_AUDIO` (existing) |
| `tts` | text-to-speech | — | `PERMISSION_AUDIO` (existing) |
| `database` | KV/SQL storage primitive (`db_get`/`db_set`/`db_query`/`db_delete`) | — | `PERMISSION_STORAGE` (new) |
| `scheduler` | fire an action/event once after a delay, or repeatedly on a cron expr | `database` (persist schedule state across restarts) | `PERMISSION_SCHEDULER` (existing) |
| `vector-db` | embedding upsert/similarity search (`vec_upsert`/`vec_query`) | — | own storage backend, standalone |
| `search` | web search (grounding, not just fetch) | `network` | none beyond `network`'s |
| `notify` | push/desktop/webhook notifications | — | `PERMISSION_NOTIFY` (existing) |
| `email` | send/receive mail (SMTP/IMAP) | `network`, `secrets` (mailbox creds) | none beyond `network`'s |
| `image` | image gen + vision (describe/OCR) | `network` (provider API), `secrets` | none beyond `network`'s |
| `clipboard` | read/write system clipboard | — | `PERMISSION_CLIPBOARD` (new) |
| `system` | query host info (battery, procs, volume, screen lock) | — | `PERMISSION_SYSTEM` (existing) — broad access, keep strict |
| `launcher` | launch apps/games by name — Steam (`steam://rungameid/<id>`, reads `libraryfolders.vdf`/`appmanifest_*.acf`) as one provider, generic app launch as another | `filesystem` (read manifests) | `PERMISSION_LAUNCH` (new) |
| `media` | control media playback (play/pause/skip/volume) — Spotify/YouTube API or MPRIS/media-keys locally | `network` (remote providers), `secrets` | none beyond `network`'s |
| `screenshot` | capture screen/window, optional OCR | `image` (OCR) | `PERMISSION_SCREEN` (new) |
| `window` | list/focus/switch/minimize/maximize open windows | — | `PERMISSION_SYSTEM` (existing, shares scope with `system`) |
| `home` | home automation over a custom protocol to bare-metal devices (ESP32/Arduino) — not Home Assistant/MQTT, own wire format | `network` (or serial/BLE transport, TBD) | `PERMISSION_HOME` (new) |
| `browser` | read/control active browser tab (url/title/DOM/screenshot) — native-messaging host (the actual plugin, built on `veyron-sdk-rust`) + a browser extension (Chrome/Firefox) as the tab-access side | — | `PERMISSION_BROWSER` (existing, unused today) |
| `notes` | note CRUD | `database` | none |
| `calendar` | event CRUD + reminders + `notify` on due | `database`, `scheduler`, `notify` | none |
| `agent` | multi-step goal loop: `ai` chat + tool-call dispatch to other plugins' actions, state persisted | `ai`, `database`, `vector-db`, `scheduler` | none itself — inherits from what it calls |
| `webclient` | browser chat UI + mic voice input/TTS playback, talks to kernel WS API | `agent` (Kairo), `stt`, `tts` | none itself — client only, auth via kernel JWT |
| `daemon` | headless background service: mic listen loop, TTS output, no window/browser | `agent` (Kairo), `stt`, `tts` | none itself — client only, auth via kernel JWT |
| `telegram` | third client: two-way chat + voice notes via Telegram bot API | `agent` (Kairo), `stt`, `tts`, `secrets` (bot token) | none itself — client only |

`notes`/`calendar` are thin once `database` exists — just schema + validation
on top of it, same relationship `ai` has to `network`.

`secrets` should ship early — `network`/`ai` need somewhere to keep API
keys/tokens that isn't plaintext config. Any plugin holding a credential
today should migrate to it once it exists.

`agent` ships last: it's the integration point for everything else, so it's
the plugin most likely to change shape once the others exist and their real
action surfaces are known.

`webclient`/`daemon`/`telegram` are all thin clients to `agent` — no business
logic of their own, just UI surface (browser, headless mic/speaker, bot chat)
over the same kernel WS API. Separate plugins because their lifecycle
differs: `webclient` opened on demand, `daemon` runs always-on in
background, `telegram` is driven by bot API polling/webhook — different
supervisor/resource-limit config per README's "separate processes" model.
`telegram` is a client, not a `notify` channel — it's two-way (replies,
voice notes in), `notify` stays one-way alert delivery only.

Considered and skipped: `contacts` (fold into `database` as a schema
convention, not its own CRUD/permission), `translate` (`ai` chat completion
already does this via prompt, no dedicated plugin needed), `sms` (external
per-message cost for uncertain payoff — `telegram`/`notify` cover the
notification-to-phone case already), `shell` (arbitrary command exec breaks
the narrow-permission-per-plugin model every other plugin follows —
`filesystem`'s read-only `fs_list`/`fs_read` actions cover the "just let it
browse files" use case without an exec surface).

`home` is deliberately not Home Assistant/MQTT — custom wire protocol
talking directly to ESP32/Arduino-class devices, so transport (serial/BLE/
raw socket) needs deciding before real design starts.

`browser` is an extension, not CDP-driven — works against the user's real
browser/profile/logged-in sessions, no `--remote-debugging-port` launch
flag, permissions surfaced through the browser's own extension-permission
UI. Extensions can't open a UDS socket directly, so the plugin has two
halves: a native-messaging host (stdio, spawned by the browser, this is the
real `veyron-sdk-rust` plugin talking to the kernel) and the extension
itself (JS, `tabs`/`scripting` permissions) relaying over
`chrome.runtime.connectNative`.

## Concurrency model for hot-path plugins

The kernel protocol already supports multiple in-flight `ActionRequest`s per
plugin connection — `action_id` is the correlation key end-to-end (see
`ActionRequest`/`ActionResponse` in `wire/proto/veyron_protocol.proto`), the
pending-action registry tracks them independently
(`src/ipc/protocol.rs:568-577`), and there's already a per-caller concurrency
cap (R6-03). Responses do not need to come back in request order.

What's *not* concurrent today is the plugin side. The Rust SDK's default
`serve()` loop (`veyron-sdk-rust/src/plugin.rs:117-147`) does
`recv().await` → `on_message().await` → reply → next `recv()` — fully
sequential, one request finishes before the next frame is even read off the
socket. `ai` and `network` use a custom loop for an unrelated reason (need
`&mut VeyronClient` inside the handler) but it's still sequential — fine for
them, call volume is low and latency is network-bound anyway.

`database` will be called far more often and needs real concurrency. Also:
the kernel currently rejects a second connection registered under the same
`plugin_id`, so multiplexing across sockets isn't an available escape hatch —
concurrency has to happen within one connection.

Plan for `database`, `vector-db`, and `scheduler` (and anything else on the
hot path):

- Don't use the SDK's sequential `serve()`. Custom loop: one task reads
  frames off the UDS connection and `tokio::spawn`s a handler per incoming
  `ActionRequest`; a single writer (mutex-guarded write-half, or an mpsc
  channel funneled to one writer task) sends `ActionResponse`s back as they
  complete, matched by `action_id`. Out-of-order replies are fine — the
  kernel already handles that.
- Internally, use an async connection pool (`sqlx::SqlitePool` or
  `deadpool`) sized to N so concurrent requests get real parallelism, not
  serialized await chains.
- Batched actions where round-trip count matters more than payload size
  (e.g. `db_batch_get`).
- `notes`/`calendar` inherit this for free — they just call `database`,
  they don't need their own concurrency handling.
- Rust only for these — no Python/C++ SDK versions of `database` or
  `vector-db`; hot-path plugins stay in the SDK with the async pool story.

No kernel or protocol change needed for any of this — it's purely a
plugin-implementation pattern change from the sequential loop `ai`/`network`
established.

## Kernel-side changes needed (veyron repo, not this one)

Most of the above needs **no** kernel change — `PERMISSION_NETWORK`,
`PERMISSION_FILES_READ`/`WRITE`, `PERMISSION_SYSTEM`, `PERMISSION_AUDIO`,
`PERMISSION_NOTIFY`, `PERMISSION_SCHEDULER`, `PERMISSION_BROWSER`,
`PERMISSION_IPC_SEND` already exist in
`wire/proto/veyron_protocol.proto:107-123` and cover `filesystem`, `stt`/
`tts`, `system`/`window`, `notify`, `scheduler`, `browser` respectively.

What's actually new, in `veyron`:

- **Proto enum addition** — 6 new `PermissionType` values, next free number
  is 14 (7 and old `PERMISSION_AI` are `reserved`, don't reuse): `SECRETS`
  (`secrets`), `STORAGE` (`database`), `CLIPBOARD` (`clipboard`), `LAUNCH`
  (`launcher`), `SCREEN` (`screenshot`), `HOME` (`home`).
- **Manual sync to `sdk/python/proto/veyron_protocol.proto` and
  `sdk/cpp/proto/veyron_protocol.proto`** — these are plain copies of
  `wire/proto/veyron_protocol.proto`, not symlinks, no sync tooling exists.
  Every enum addition above needs copying into both by hand. (Separately:
  `scripts/gen_proto_python.py` points at a `proto/` path that no longer
  holds the file — pre-existing breakage, not part of this batch.)
- **`src/auth/permissions.rs::required_permission_for_action`** — only
  needs an entry if a new plugin's action is *providable through another
  plugin* (the anti-laundering pattern that exists for `http_request` →
  `PermissionNetwork` today). Evaluate per-plugin as each one lands, not a
  bulk change now.
- **`daemon`'s always-on lifecycle** — found no autostart/enabled concept
  in `config.yaml` or the plugin manager; every plugin today looks
  spawned the same way. Needs a real look (supervisor or config change)
  once `daemon` design starts — open question, not yet scoped.

No IPC/framing/orchestrator changes needed beyond that.

## Non-goals

- No plugin-to-plugin direct calls — everything routes through the kernel,
  same as `ai` → `network` today.
- No new kernel-level scheduling/timer primitive — `scheduler` is an
  ordinary plugin publishing to the event bus / firing actions on a timer,
  matching the "zero-AI/zero-scheduling in kernel core" precedent already
  set for `ai` (`plugins/ai/ROADMAP.md`, "Non-goals" section).
- `vector-db` stays a separate plugin from `database`, not a mode of it —
  different backend, different access pattern (similarity search vs
  relational/KV), same reasoning that kept `ai` from reinventing `network`'s
  HTTP client.
