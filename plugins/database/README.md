# database plugin

Per-caller-namespaced KV + raw SQL storage for Veyron plugins, gated by
`PERMISSION_STORAGE`. One SQLite file per calling plugin — callers cannot
see or query each other's data. See
`docs/superpowers/specs/2026-07-15-database-plugin-design.md` for the full
design and `ROADMAP.md` in this directory for what's deferred.

Identity is taken only from the kernel-stamped `ActionRequest.caller_plugin_id`,
never from params, and is sanitized to `[a-zA-Z0-9_-]` before use as a
filename — an empty or malformed caller id is an `ACTION_ERROR`, never a
shared/default namespace.

## Actions

| Action | Params | Result |
|---|---|---|
| `db_get` | `{key}` | `{found, value}` |
| `db_set` | `{key, value}` | `{ok: true}` |
| `db_delete` | `{key}` | `{deleted}` |
| `db_batch_get` | `{keys: [..]}` | `{values: {key: value, ..}}` (missing keys map to `null`) |
| `db_query` | `{sql, params: [..]}` | `{rows: [..], rows_affected}` |

`db_query` runs against the caller's own database file only — `ATTACH` is
rejected (whole-word, case-insensitive keyword pre-check). Positional binds
use `?1`, `?2`, …. `SELECT`/`WITH`/`PRAGMA` return typed `rows`;
everything else returns `rows_affected`.

Values are stored as JSON text; `db_get`/`db_batch_get` return the decoded
JSON value, not the raw string.

## Config

Config is read from the environment (the kernel's plugin supervisor
translates the `config.yaml` `env:` entry into these before spawning the
plugin). Copy `config.example.yaml` for the documented defaults.

| Env var | Default | Meaning |
|---|---|---|
| `DATABASE_PLUGIN_DATA_DIR` | — (required) | directory holding per-caller `.db` files |
| `DATABASE_PLUGIN_POOL_SIZE` | `4` | connections per caller pool |
| `DATABASE_PLUGIN_BUSY_TIMEOUT_MS` | `5000` | SQLite busy timeout |
| `DATABASE_PLUGIN_MAX_VALUE_BYTES` | `1048576` (1 MiB) | `db_set` value size cap |
| `DATABASE_PLUGIN_MAX_RESPONSE_BYTES` | `4194304` (4 MiB) | `db_query` result size cap |

Size caps reject — they never truncate. An oversized `db_set` value or
`db_query` result is an `ACTION_ERROR`.

## Concurrency

Unlike `ai`/`network` (sequential `Plugin::run`), this plugin hand-rolls a
concurrent loop per the roadmap's "hot-path plugins" pattern: one task owns
the `VeyronClient` and `tokio::select!`s between inbound frames and an mpsc
channel of completed responses that spawned handler tasks push into. The
client is never behind a lock, so a handler replying can't deadlock against
the loop parked in `recv()`. Each caller gets a cached `sqlx::SqlitePool`
(WAL mode) for real parallelism. Replies may come back out of order — the
kernel matches on `action_id`.

## Status

v1. Depends on kernel support for `ActionRequest.caller_plugin_id` and
`PERMISSION_STORAGE` (see
`docs/superpowers/plans/2026-07-17-database-plugin-kernel-support.md` in the
`veyron` repo). Currently built against a local path override of
`veyron-wire`/`veyron-sdk` rather than a published crates.io release — see
this plugin's `Cargo.toml`.
