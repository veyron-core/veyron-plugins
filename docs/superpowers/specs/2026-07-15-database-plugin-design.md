# `database` plugin — design spec

Date: 2026-07-15
Status: approved for planning

## Goal

KV/SQL storage primitive for Veyron plugins (`ROADMAP.md` "Planned" table).
One blessed storage path, same relationship to callers that `network` has for
HTTP. Downstream consumers: `scheduler`, `notes`, `calendar`, `agent`.

## Scope decisions (settled with user)

- **Query surface:** namespaced KV + raw SQL. Each caller gets its own
  SQLite database file; `db_query` accepts raw SQL against that file only.
- **Kernel changes included:** yes — this work also touches the sibling
  `veyron` repo (permission enum + caller stamping).
- **Caller identity:** kernel-stamped `caller_plugin_id` field on
  `ActionRequest`. Kernel overwrites it when forwarding; inbound values are
  ignored, so it cannot be spoofed.
- **`ATTACH` blocked:** callers must not be able to open other database
  files from inside SQL (would bypass per-caller isolation).

## Plugin layout

`plugins/database/` — mirrors `ai`/`network`:

```
plugins/database/
  plugin.json          # id "database", actions below, permissions: ["PERMISSION_STORAGE"]
  Cargo.toml           # veyron-sdk, veyron-wire, tokio, serde, serde_json, sqlx (sqlite, runtime-tokio)
  config.example.yaml
  README.md
  ROADMAP.md
  src/
```

Rust only (roadmap: hot-path plugins stay in the Rust SDK; no python/cpp
ports).

## Actions

All actions operate within the caller's namespace (see Isolation). Values
are arbitrary JSON.

| Action | Params (`params_json`) | Result (`data_json`) |
|---|---|---|
| `db_get` | `{key}` | `{found: bool, value: <json or null>}` |
| `db_set` | `{key, value}` | `{ok: true}` |
| `db_delete` | `{key}` | `{deleted: bool}` |
| `db_batch_get` | `{keys: [..]}` | `{values: {key: <json or null>, ..}}` |
| `db_query` | `{sql, params: [..]}` | `{rows: [{col: val, ..}], rows_affected}` |

`db_query` params bind positionally (`?1`, `?2`, …). Rows come back as JSON
objects keyed by column name. SQLite types map: NULL→null, INTEGER→number,
REAL→number, TEXT→string, BLOB→base64 string.

## Isolation model

- One SQLite file per caller: `<data_dir>/<caller_plugin_id>.db`.
  Filename derives from the kernel-stamped identity only — never from
  params. `caller_plugin_id` is sanitized to `[a-zA-Z0-9_-]` before use as
  a path component; anything else is rejected (defense in depth — the
  kernel registry is the real gate on plugin ids).
- Empty `caller_plugin_id` (old kernel, direct socket poke) → action
  rejected with `ACTION_ERROR`, never a shared/default namespace.
- KV actions use a reserved table in the same file:
  `kv(key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL)`
  — created on first open. Caller SQL may read/join it; that's their own
  data.
- **`ATTACH DATABASE` blocked:** on every pooled connection, set
  `SQLITE_LIMIT_ATTACHED = 0` via the raw sqlite3 handle in the pool's
  `after_connect` hook (sqlx exposes the handle via `lock_handle()`).
  If the limit turns out not to reach 0 on the bundled SQLite build,
  fallback is a keyword pre-check that rejects statements containing
  `ATTACH` — decided at implementation time, spec requires only that
  `ATTACH` fails.
- No cross-caller action surface at all in v1 (no admin/list-namespaces
  action).

## Concurrency (roadmap hot-path pattern)

- Do **not** use SDK sequential `serve()`. Custom loop:
  - one reader task pulls frames off the UDS connection and
    `tokio::spawn`s a handler per `ActionRequest`;
  - handlers send completed `ActionResponse`s into an mpsc channel;
  - one writer task drains the channel to the socket. Out-of-order replies
    are fine (kernel matches on `action_id`).
- Per-caller `sqlx::SqlitePool` (WAL mode, `busy_timeout`), cached in a
  `HashMap<String, SqlitePool>` behind an async lock. Pool created lazily
  on first action from that caller.

## Config (`config.example.yaml`)

| Key | Default | Meaning |
|---|---|---|
| `data_dir` | required | directory holding per-caller `.db` files |
| `pool_size` | 4 | connections per caller pool |
| `busy_timeout_ms` | 5000 | SQLite busy timeout |
| `max_value_bytes` | 1 MiB | reject `db_set` values larger than this |
| `max_response_bytes` | 4 MiB | `db_query` result cap; exceeding → error, not truncation |

No per-caller disk quota in v1 (revisit if a consumer plugin proves
abusive).

## Error handling

- Malformed params / unknown action → `ACTION_ERROR` with a message naming
  the bad field.
- SQL errors → `ACTION_ERROR` with SQLite's message passed through. Never
  partial rows, never silent truncation.
- Missing/empty caller id → `ACTION_ERROR` (see Isolation).

## Kernel changes (sibling `veyron` repo)

1. `wire/proto/veyron_protocol.proto`: add `PERMISSION_STORAGE = 14`
   (7 and old `PERMISSION_AI` are reserved — do not reuse).
2. Same file: add `string caller_plugin_id = 6;` to `ActionRequest`.
   In `src/ipc/protocol.rs` (forwarding at ~line 619), set it from the
   already-computed `sender_id`; ignore any inbound value.
3. Manual copy of the proto to `sdk/python/proto/veyron_protocol.proto`
   and `sdk/cpp/proto/veyron_protocol.proto` (plain copies, no tooling).
4. **No** `required_permission_for_action` entry for `db_*` actions:
   roadmap gives `notes`/`calendar` "none" for permissions, and per-caller
   namespacing removes the laundering concern the T-19 pattern guards
   against.

`veyron-sdk-rust` needs the regenerated proto too so the plugin can read
`caller_plugin_id`.

## Testing

- Unit tests on the handler with temp-dir SQLite files: each action's happy
  path, isolation (two callers, same key, different values), `ATTACH`
  rejection, size caps, missing-caller-id rejection, malformed params.
- Concurrency smoke test: N concurrent `db_set`/`db_get` through the
  handler, assert all succeed and no cross-talk.
- Kernel-side: existing veyron test suite plus a routing test asserting
  `caller_plugin_id` is stamped (and inbound spoof value overwritten).

## Non-goals (v1)

- No TTL/expiry on KV entries.
- No per-caller disk quotas.
- No cross-caller/admin actions.
- No streaming query results (`max_response_bytes` cap instead).
- Not a vector store (`vector-db` is its own plugin per roadmap).
