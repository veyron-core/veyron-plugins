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
| `ai` | `plugins/ai/` | `network` | LLM chat completion (anthropic/openai-compatible), declares `network` — caller of `network`'s gated `http_request` (T-19) |
| `database` | `plugins/database/` | — | KV/SQL storage primitive, `PERMISSION_STORAGE`, per-caller SQLite file isolation |
| `tts` | `plugins/tts/` | `network` (cloud providers) | text-to-speech — local ONNX (sherpa: Kokoro/Piper) in-process + openai/elevenlabs via `network`, declares `network` (caller of gated `http_request`) |
| `stt` | `plugins/stt/` | `network` (cloud provider) | speech-to-text — local ONNX (sherpa: zipformer/whisper) in-process + openai audio via `network`, declares `network` (caller of gated `http_request`) |

## Planned

Dependency order — each row can start once everything in "depends on" ships.

| Plugin | Purpose | Depends on | Permissions |
|---|---|---|---|
| `secrets` | encrypted credential/API-key vault (`secret_get`/`secret_set`) | — | `PERMISSION_SECRETS` (defined, proto v1.4) |
| `filesystem` | sandboxed file read/write + read-only browse (`ls`/`cat` equivalents: `fs_list`/`fs_read`) — no exec, no shell | — | `PERMISSION_FILES_READ`/`PERMISSION_FILES_WRITE` (existing) |
| `scheduler` | fire an action/event once after a delay, or repeatedly on a cron expr | `database` (persist schedule state across restarts) | `PERMISSION_SCHEDULER` (existing) |
| `vector-db` | embedding upsert/similarity search (`vec_upsert`/`vec_query`) | — | own storage backend, standalone |
| `search` | web search (grounding, not just fetch) | `network` | `network` (caller of gated `http_request`) |
| `notify` | push/desktop/webhook notifications | — | `PERMISSION_NOTIFY` (existing) |
| `email` | send/receive mail (SMTP/IMAP) | `network`, `secrets` (mailbox creds) | `network` (caller of gated `http_request`) |
| `image` | image gen + vision (describe/OCR) | `network` (provider API), `secrets` | `network` (caller of gated `http_request`) |
| `clipboard` | read/write system clipboard | — | `PERMISSION_CLIPBOARD` (defined, proto v1.4) |
| `system` | query host info (battery, procs, volume, screen lock) | — | `PERMISSION_SYSTEM` (existing) — broad access, keep strict |
| `launcher` | launch apps/games by name — Steam (`steam://rungameid/<id>`, reads `libraryfolders.vdf`/`appmanifest_*.acf`) as one provider, generic app launch as another | `filesystem` (read manifests) | `PERMISSION_LAUNCH` (defined, proto v1.4) |
| `media` | control media playback (play/pause/skip/volume) — Spotify/YouTube API or MPRIS/media-keys locally | `network` (remote providers), `secrets` | `network` (caller of gated `http_request`) |
| `screenshot` | capture screen/window, optional OCR | `image` (OCR) | `PERMISSION_SCREEN` (defined, proto v1.4) |
| `window` | list/focus/switch/minimize/maximize open windows | — | `PERMISSION_SYSTEM` (existing, shares scope with `system`) |
| `home` | home automation over a custom protocol to bare-metal devices (ESP32/Arduino) — not Home Assistant/MQTT, own wire format | `network` (or serial/BLE transport, TBD) | `PERMISSION_HOME` (defined, proto v1.4) |
| `browser` | read/control active browser tab (url/title/DOM/screenshot) — native-messaging host (the actual plugin, built on `veyron-sdk-rust`) + a browser extension (Chrome/Firefox) as the tab-access side | — | `PERMISSION_BROWSER` (existing, unused today) |
| `notes` | note CRUD | `database` | none |
| `calendar` | event CRUD + reminders + `notify` on due | `database`, `scheduler`, `notify` | none |
| `agent` | multi-step goal loop: `ai` chat + tool-call dispatch to other plugins' actions, state persisted | `ai`, `database`, `vector-db`, `scheduler` | none itself — inherits from what it calls |
| `webclient` | browser chat UI + mic voice input/TTS playback, talks to kernel WS API | `agent` (Kairo), `stt`, `tts` | none itself — client only, auth via kernel JWT |
| `daemon` | headless background service: mic listen loop, TTS output, no window/browser | `agent` (Kairo), `stt`, `tts` | none itself — client only, auth via kernel JWT |
| `telegram` | third client: two-way chat + voice notes via Telegram bot API | `agent` (Kairo), `stt`, `tts`, `secrets` (bot token) | none itself — client only |

`notes`/`calendar` are thin once `database` exists — just schema + validation
on top of it, same relationship `ai` has to `network`.

Under the Manifest v2 data-driven permission model (§3), any plugin that
invokes `network`'s gated `http_request` — the shipped `ai`/`tts`/`stt` and
the planned `search`/`email`/`image`/`media` — declares `PERMISSION_NETWORK`
itself: T-19 requires the *caller* of a gated action to hold its permission,
and the per-action `permission` in `network`'s manifest makes that check
data-driven ("any caller without the permission is denied").

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
`wire/proto/veyron_protocol.proto:107-124` and cover `filesystem`, `system`/
`window`, `notify`, `scheduler`, `browser` respectively. `stt`/`tts` shipped
with **zero** kernel changes (no declared permissions — local ONNX runs
in-process, cloud providers route through `network`).

What's actually new, in `veyron`:

- **Proto enum addition — protocol v1.4.** **Shipped** (wire housekeeping,
  `veyron-wire` 0.2.1): 5 new `PermissionType` values **15–19** defined
  (`PERMISSION_STORAGE = 14` shipped with `database`; 7 and old
  `PERMISSION_AI` are `reserved`, don't reuse):

  | Value | Permission | Plugin |
  |---|---|---|
  | 15 | `PERMISSION_SECRETS` | `secrets` |
  | 16 | `PERMISSION_CLIPBOARD` | `clipboard` |
  | 17 | `PERMISSION_LAUNCH` | `launcher` |
  | 18 | `PERMISSION_SCREEN` | `screenshot` |
  | 19 | `PERMISSION_HOME` | `home` |

  Values are **contiguous (15–19)** — the installer's
  `known_permissions()` probe (`veyron/src/marketplace/installer.rs:25`)
  walks enum codes and stops after 4 consecutive misses, so a gap ≥4 would
  silently reject installs of any plugin declaring a later value. The
  `// v 1.4` header bump landed in the same change. The kernel's own `M9`
  (zero-value enum renumber, wire-breaking) was gated on this protocol bump
  and lands with it.
- **Regenerate `veyron-wire` prost types.** **Shipped** — the generated
  `PermissionType` (prost, build-time from the proto) includes the new
  values; `known_permissions()` (kernel `R8-01`) and the JWT `permissions`
  claims (free-form strings) adopt them automatically, no kernel Rust
  source change needed. `vyn plugin install` now accepts manifests
  declaring the new permissions (e.g. `PERMISSION_SECRETS`).
- **Proto-copy sync — all three copies on v1.4.** **Shipped** — the kernel
  repo vendors no proto (`src/proto.rs` is
  `pub use veyron_wire::proto::veyron;`), so the crate is the single source
  of protocol truth for kernel + SDK-rust. The R8-05 byte-identity test
  (`tests/unit/test_proto_sync.rs`) guards the remaining copies:
  - `veyron-wire/proto/veyron_protocol.proto` — the source of regeneration;
  - `veyron-sdk-python/proto/...` + `veyron-sdk-cpp/proto/...` — synced to
    v1.4 (previously on v1.2/v1.3); the Python binding
    (`veyron-sdk-python/veyron/veyron_protocol_pb2.py`) was regenerated via
    `scripts/gen_proto_python.py` and the R8-05 marker check extended to
    the five new permission values.
  `pub const PROTOCOL_VERSION` (`"1.4"`) was added to `veyron-wire`
  alongside — it mirrors the proto header comment (`// v 1.4`). Long-term:
  vendor the .proto as an asset inside the veyron-wire crate and have SDK
  build scripts generate from the *installed package* — removes vendoring
  entirely, so the SDKs can't drift even in principle.
  (`scripts/gen_proto_python.py` was repaired earlier — it regenerates the
  Python binding from `../veyron-wire/proto/` and works.)
- **`src/auth/permissions.rs::required_permission_for_action`** — only
  needs an entry if a new plugin's action is *providable through another
  plugin* (the anti-laundering pattern that exists for `http_request` →
  `PermissionNetwork` today). None of the planned plugins expose a
  primitive like that, so no additions expected — evaluate per-plugin as
  each one lands, not a bulk change now.
- **`daemon`'s always-on lifecycle** — found no autostart/enabled concept
  in `config.yaml` or the plugin manager; every plugin today looks
  spawned the same way. Needs a real look (supervisor or config change)
  once `daemon` design starts — open question, not yet scoped.

No new Envelope payloads, IPC, framing, or orchestrator changes needed:
every planned plugin fits the existing `ActionRequest`/`Event`/
`EventPublish`/IPC/streaming/`AudioStreamChunk` + WebSocket surfaces.

## Infrastructure Evolution: Plugin Distribution & Registry

A single distribution format for plugins, built so the format itself never
needs a breaking change (additive fields, lenient parsing) and the artifact
host is swappable (relative URLs). **Normative schema:** `veyron/docs/
PLUGIN_REGISTRY_SCHEMA.md` (kernel repo) — this file is the plan, that doc is
the contract. `scripts/package.sh` is the one tool that writes both sides and
must stay in sync with the schema.

### Roles (delivery vs execution separation)

- **`plugin.json`** (manifest) — *execution*: what runs, what it can do.
  Lives inside the archive; the in-archive copy is authoritative.
- **`registry.json`** — *delivery index*: what's available, where, per-version
  sha256/signature. The machine source of truth the kernel reads.
- **`dist/`** — *artifact store*: co-located per-version files for humans,
  ops, and at-rest audit. **Not consumed by the kernel.** `package.sh`
  generates the registry entry and the dist files from one computation, so
  the two representations cannot drift.

### 1. Distribution Store (`dist/`) — hierarchical

```
dist/{slug}/
├── latest.json                    # {"version": "0.2.0"} — host-agnostic pointer
├── assets/                        # version-agnostic: icon.png, setup.md, dependencies.json
└── versions/{version}/
    ├── {slug}-{version}.zip       # binary archive
    ├── {slug}-{version}-src.zip   # source archive (audit)
    ├── plugin.json                # manifest of this version (browse without downloading)
    ├── checksum.sha256
    └── signature.sig
```

- **Version isolation**: one folder per release → retention, rollback,
  partial mirroring, per-folder CDN cache control, per-plugin storage
  management on a self-hosted VPS.
- **`latest.json` instead of a symlink**: GitHub raw / static CDNs do not
  follow symlinks. The kernel does not depend on it either — it resolves
  latest as semver-max over the registry `versions` map **among entries with
  `status: stable` (or absent status), falling back to any version when no
  stable exists** (zero drift). `latest.json` is for humans/ops/mirroring.
- **`assets/`** (not `resources/`) to avoid confusion with the manifest's
  `files` field. `dependencies.json` here lists *system* packages for the
  kernel's optional auto-check — distinct from the registry's plugin
  `dependencies`.
- The per-version `plugin.json`/`checksum.sha256`/`signature.sig` are for
  manual verification and browsing; the registry carries the same values
  (same computation, two outputs).
- **The browse-copy `plugin.json` is NOT covered by the entry signature**
  (only the zip's sha256 is signed). The kernel must never read it — the
  authoritative manifest is the one inside the zip, which IS covered via the
  signed zip hash. Browse copy is humans-only.

### 2. Registry Evolution (`registry.json`)

Array → object map keyed by slug. The kernel parser already accepts this form
and the R10-03 cache is ready:

```json
{
  "meta": { "apiVersion": 2, "lastUpdated": "2026-08-13" },
  "revoked": ["evil@1.0.0"],
  "ai": {
    "name": "AI",
    "description": "Provider-agnostic LLM chat completion.",
    "category": "ai",
    "tags": ["llm"],
    "status": "stable",
    "source_url": "https://github.com/veyron-core/veyron-plugins/tree/main/plugins/ai",
    "versions": {
      "0.1.0": {
        "archive_url": "dist/ai/versions/0.1.0/ai-0.1.0.zip",
        "sha256": "<hex>",
        "signature": "<hex>",
        "min_kernel_version": "0.1.0",
        "max_kernel_version": "*",
        "dependencies": { "network": ">=0.1.0" }
      }
    }
  }
}
```

- **Relative `archive_url`** — resolved against the registry's own base URL.
  Moving the store GitHub → own VPS → Cloudflare R2, or pointing at a
  community marketplace, is a one-line `registry_url` change in config.yaml.
  Nothing gets re-published.
- **No permissions in the registry** — execution metadata lives in the
  manifest (inside the archive); duplicating it in the registry would only
  drift. `vyn plugin search` surfaces name/description/category instead.
- **`status`** (`stable`/`beta`/`deprecated`/`hidden`/`revoked`) — only
  `revoked` is kernel-enforced (R10-03): the root `revoked: ["slug",
  "slug@version"]` list folds into entries, `vyn install` refuses, entries
  stay listed with a `[revoked]` marker, and revocation outlives the cache
  TTL. Default at slug level; an optional `versions[].status` overrides it
  per version (e.g. `0.1.0` stable, `0.2.0` beta).
- **`dependencies: { "slug": ">=semver" }`** — install-time, transitive:
  `vyn install` resolves and installs prerequisites first, refuses on version
  mismatch. **Kernel-enforced** — a plugin whose deps aren't installed is not
  installable. Load-time ordering stays the manifest's existing `requires`
  (already enforced: missing deps / cycles refuse the plugin).
  **Range syntax is deliberately limited to `>=x.y.z` or exact `x.y.z`** —
  no caret/tilde/AND-OR. The resolver stays a simple recursive walk with
  cycle detection (same shape as `requires`); a full npm-style resolver is
  out of scope for the dumb kernel.
- **`meta`** — lastUpdated + apiVersion for cache invalidation (R10-03
  echoes it into `registry-cache.json`, accepts `apiVersion`/`api_version`).
- **One active registry per install** — `registry_url` + `marketplace_public_key`
  config.yaml overrides already exist. A community marketplace is another URL
  + key the operator chooses to pin (entries must verify against that key).
  Multi-registry aggregation/search is future work and not blocked by the
  format.

### 3. Manifest Optimization (`plugin.json`)

- **Clean-up**: remove delivery data (`archive_url`, `sha256`) from the
  manifest — it lives in the registry now.
- **Per-action specification**: `actions` become objects, not strings:
  `[{ "name": "http_request", "permission": "network", "input": {...}, "output": {...} }]`.
  The per-action `permission` makes the kernel's anti-laundering check
  (`required_permission_for_action`, today hardcoded for `http_request` →
  `PERMISSION_NETWORK`) **data-driven**: any caller without the permission is
  denied, whatever the action. Input/output schemas serve Veyron Web and the
  future `agent` tool dispatch. The declared `permissions` set stays as-is —
  kernel Steps 3/4 (unknown permission, config-grant cross-check) unchanged.
- **`config_schema`** — JSON Schema (draft-07 subset), not a custom format.
  Veyron Web auto-generates settings forms; the plugin validates its own
  config. The kernel does not validate (dumb core).
- **`files`** (renamed from `resources`) — explicit list of files extracted
  from the archive into the plugin's working directory. Doubles as the
  **extraction allowlist**: the installer extracts only the declared files
  and ignores the rest (tighter than "extract everything" on top of the
  zip-bomb limits). Renamed to avoid confusion with `dist/{slug}/assets/`.
- **No `api_level`** — decided against. The kernel is a dumb router; its
  plugin-visible contract is the wire format + the permission enum, both of
  which live in `veyron-wire` (below). Compatibility is fully covered by:
  `kernel_compatibility_range` (semver — the gate), the installer's
  `known_permissions()` probe (new permissions are adopted automatically when
  the kernel bumps its veyron-wire dependency), and additive/lenient manifest
  parsing (unknown fields ignored). A separate api_level axis would need a
  mapping table maintained forever — YAGNI. If a plugin-visible kernel
  behavior ever genuinely needs gating, add one optional manifest field then.

### 4. Protocol single source (`veyron-wire`)

The kernel already consumes every protocol type from the crate: `src/proto.rs`
is `pub use veyron_wire::proto::veyron;` and `known_permissions()` probes the
generated `PermissionType`. A protocol/permission change is therefore already
"bump veyron-wire → kernel + SDK-rust adopt via the dependency." Remaining work:

- ~~Add `pub const PROTOCOL_VERSION` to veyron-wire~~ — **done** (0.2.1,
  `"1.4"`); it now mirrors the proto header comment.
- ~~Sync the vendored copies + fix `gen_proto_python.py`~~ — **done**:
  `veyron-sdk-python/proto` and `veyron-sdk-cpp/proto` are on v1.4 (they
  were on v1.2/v1.3), the Python binding was regenerated, and the R8-05
  byte-identity test + pb2 marker check guard them (markers extended to the
  new permission values). `gen_proto_python.py` was repaired earlier and
  verified working.
- Long-term: vendor the .proto as an asset inside the veyron-wire crate and
  have SDK build scripts generate from the *installed package* — removes
  vendoring entirely, so the SDKs cannot drift even in principle.

### 5. Signing

Trust model unchanged (T-11): Ed25519 over `{slug}:{version}:{sha256}`,
verified against the pinned `MAINTAINER_PUBLIC_KEY_HEX` (or the
`marketplace_public_key` override for private/community registries). The
maintainer signs locally with a personal offline key; `scripts/package.sh`
gains a sign step (key from env/file, never committed). Host migration never
touches keys — only `registry_url`.

### Sequencing

1. **Registry v2 + dist/ hierarchy + package.sh** — map form, relative URLs,
   `dependencies`, new dist layout, signing step. Kernel is already tolerant;
   one PR in this repo. **Shipped** (PR #5).
2. **wire housekeeping** — `PROTOCOL_VERSION` const, sync SDK copies to v1.4,
   fix `gen_proto_python.py`. **Shipped** — wire is at protocol v1.4
   (5 new `PermissionType` values 15–19) with `veyron_wire::PROTOCOL_VERSION`;
   `veyron-sdk-python` + `veyron-sdk-cpp` vendored copies and the Python `pb2`
   binding synced (R8-05 byte-identity + marker checks pass); Rust
   `veyron-sdk` restored to the published 0.1.2 API surface (streaming methods
   had gone missing from the repo) and bumped to 0.1.3; kernel consumes
   `veyron-wire 0.2.1` / `veyron-sdk 0.1.3` via `[patch.crates-io]` git
   overrides until the crates are published (`gen_proto_python.py` had already
   been repaired in an earlier PR — verified working, no change needed).
3. **Manifest v2** — per-action permissions + `config_schema`; touches every
   plugin, kernel load-time checks, and Veyron Web. **Shipped for plugins +
   kernel** (all 6 manifests are v2, kernel parses object-form `actions`,
   enforces `files` extraction allowlist, and the anti-laundering check is
   data-driven from per-action `permission`). Veyron Web consuming
   `input`/`output`/`config_schema` for form generation is still open.


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
