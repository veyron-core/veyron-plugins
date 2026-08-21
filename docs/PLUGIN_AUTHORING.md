# Plugin authoring notes

Practical lessons from building `notes` and `calendar` (2026-08), written so
the next plugin doesn't rediscover them the hard way. Every pattern here is
implemented and tested in `plugins/notes/src/` and `plugins/calendar/src/`;
the original custom-loop variant is `plugins/sync-client/src/lib.rs`.

## 1. The single-reader rule — `send_action` discards frames

`VeyronClient::send_action` loops on `recv_timeout` while awaiting its
response and **discards every inbound frame that does not match**
(see its doc: "Frames that arrive while waiting ... are discarded").
Consequences:

- Only a task that owns 100% of the connection's traffic may call it.
- Anything else — a spawned handler task, a timer-driven scan — must route
  outbound calls through a proxy owned by the loop task. Direct use will
  silently eat concurrent traffic: user requests arriving mid-scan, kernel
  pings during a slow database round-trip.

This bit for real: calendar's first version ran the reminder scan inline on
a timer tick; every test call landing inside the scan's first `db_keys`
round-trip vanished.

**The pattern** (single-reader loop + channel-fronted RPC proxy):

- The serve loop exclusively owns `VeyronClient`; `tokio::select!` over
  `client.recv()`, an outbound `mpsc<Envelope>` channel, an RPC request
  channel, and (calendar) the scan interval tick.
- Handler/scan tasks get a cloneable `Rpc { tx }` handle; each call sends
  `RpcCall { action, params_json, timeout_ms, reply: oneshot }`.
- The loop assigns `rpc-{seq}` action ids, keeps a pending map
  `id -> (action, reply)`, and completes entries when the matching
  `ActionResponse` arrives (decode `data_json` on `ACTION_OK`, else surface
  `error`). Nothing inbound is ever dropped.
- Replies and fire-and-forget event envelopes flow back through the outbound
  channel; FIFO preserves the response-before-event ordering contract.

Hot-path storage plugins making **no** outbound calls don't need this — they
implement the SDK's `ConcurrentHandler` instead (`database`, `network`).
Background-task plugins with fire-and-forget writes can push raw envelopes
into the loop's channel like sync-client's heartbeat. Choose by shape:

| Plugin shape | Loop |
|---|---|
| hot path, no outbound IPC | SDK `serve_concurrent` / `ConcurrentHandler` |
| thin wrapper calling other plugins | sequential loop + RPC proxy (notes) |
| timer/background activity + outbound RPC | single-reader select loop + RPC proxy + spawned tasks (calendar) |

## 2. Kernel routing facts — what a caller must declare

- Actions route by manifest declaration: the kernel resolves the provider
  via `find_action_provider` (`veyron/src/ipc/protocol.rs`) and refuses
  ambiguous declarations. Declare every action you serve.
- Manifest v2 per-action `permission` is enforced on **both provider and
  caller** (data-driven T-19 anti-laundering). A wrapper plugin that calls
  gated actions must itself hold those permissions: `notes` holds
  `PERMISSION_STORAGE`; `calendar` holds `STORAGE` + `NOTIFY`. Callers of
  the wrapper's own ungated actions need nothing.
- `PERMISSION_IPC_SEND` + `ipc_targets` gate **raw frame forwarding**
  (T-04) only — ordinary kernel-routed action calls need neither.
  Precedent: `notify` calls `tts_synthesize` with neither declared.
- `ActionRequest.caller_plugin_id` is stamped by the kernel from the
  authenticated sender (inbound value discarded). `database` namespaces
  storage per caller, so a wrapper plugin gets a private namespace free.

## 3. Testing against a fake kernel

`UnixStream::pair()` + `VeyronClient::from_stream` on both ends drives the
real serve loop without a live kernel (SDK test pattern, used by
`sync-client`, `notes`, `calendar`):

- **Handshake first.** The shim must answer `PluginRegister` before
  processing any test command: `register_full` treats the very next inbound
  frame as the ack, so a test command racing ahead kills the plugin with
  "expected PluginRegisterAck". Buffer commands in an mpsc channel and start
  draining only after acking registration.
- An in-memory `FakeDb` (BTreeMap KV) answering
  `db_incr/db_set/db_get/db_keys/db_batch_get/db_delete` exercises the real
  wire shapes end to end.
- Recorders make background activity assertable: collect `EventPublish`
  frames (respond `EventPublishAck` = `EVENT_PUBLISH_OK`) and
  `notify_send` requests.
- Assert events with polling helpers, not immediately: the plugin sends the
  `ActionResponse` BEFORE the event envelope, so a just-finished call races
  the recording. Timer-driven behavior (reminder scans) needs generous poll
  windows around the configured scan interval.

## 4. Thin-wrapper checklist

- Key layout `<entity>:<id>` JSON documents + `meta:next_id` id counter
  via `db_incr` (atomic, monotonic, survives restarts).
- Response first, then best-effort change event — a publish never delays or
  fails the caller's reply (database's contract).
- Validate loudly at parse time with shape-naming errors; serde enforces
  nothing beyond types (a manifest `"minimum": 0` is documentation, not a
  check).
- Missing-entity reads are `{found: false}` results; deletes are idempotent
  (`{deleted: false}`); updates of missing entities ARE errors.
- Manifest v2: object-form actions with input/output schemas,
  `config_schema`, env vars named `<PLUGIN>_PLUGIN_*`.
- Per-plugin docs: `README.md` (contract) + `ROADMAP.md` (non-goals) — see
  any shipped plugin for the pattern.
