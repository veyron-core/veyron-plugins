# database plugin — usage guide

Reference for plugin authors calling the `database` plugin through the kernel.
For configuration, isolation rationale, and concurrency design, see
[`README.md`](./README.md); for what's deferred, see [`ROADMAP.md`](./ROADMAP.md).

## The model in one minute

- **One private SQLite file per caller.** Your namespace is keyed by the
  kernel-stamped `caller_plugin_id` — you never pass it, and you can't read or
  query another plugin's data. A missing or malformed caller id is an error,
  never a shared/default bucket.
- **Two ways to store.** A **KV fast path** (`db_set` / `db_get` / `db_delete`
  / `db_batch_get` / `db_incr` / `db_keys` / `db_append` / `db_patch`) that
  stores arbitrary JSON values, and **raw SQL** (`db_query`) against your own
  file — including tables you create yourself.
- **The KV data is a real table** named `kv`:
  `kv(key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL,
  expires_at INTEGER)`.
  `db_query` can read and write it directly, but the `value` column holds JSON
  **text** (see the `db_query` note below). `expires_at` is the KV TTL column
  — see [TTL](#ttl--expiry).

## How a call looks

You send an `ActionRequest { action, params_json, caller_plugin_id }` (the
kernel stamps `caller_plugin_id`). You get back an `ActionResponse`:

- success → status `ACTION_OK`, `data_json` = the JSON result shown below.
- failure → status `ACTION_ERROR`, `error` = a plain-text message (every
  message a caller can hit is listed in [Errors](#errors)).

Examples below show **params** (what goes in `params_json`) and **result**
(the JSON returned in `data_json`).

## KV actions

### `db_set` — write a value

```jsonc
// params
{"key": "user:42", "value": {"name": "Ada", "tier": 3}}
// result
{"ok": true}
```

- Upsert: writing an existing key overwrites it.
- `value` is **any** JSON (object, array, string, number, bool, null). It's
  stored as JSON text and stamped with `updated_at` (unix milliseconds).
- Optional `ttl_ms` expires the key that many milliseconds after the set —
  see [TTL](#ttl--expiry). Absent, `0`, or negative = no expiry; re-setting a
  key without `ttl_ms` clears any previous expiry.
- Rejected if the serialized value exceeds `max_value_bytes`
  (`value exceeds max_value_bytes (N > M)`). This is a fast-path guard only —
  a raw `db_query` `INSERT` bypasses it; the hard ceiling is the per-caller
  disk quota (`max_db_bytes`). See the README's Config section.

### `db_get` — read a value

```jsonc
// params
{"key": "user:42"}
// result (present)
{"found": true, "value": {"name": "Ada", "tier": 3}}
// result (missing)
{"found": false, "value": null}
```

The stored JSON text is decoded back into a real JSON value — you get the
object/array/number you put in, not a string. (Contrast `db_query`, which
returns the raw text.)

### `db_delete` — remove a key

```jsonc
// params
{"key": "user:42"}
// result (a row was removed / nothing matched)
{"deleted": true}
{"deleted": false}
```

### `db_batch_get` — read many keys at once

```jsonc
// params
{"keys": ["user:42", "user:99"]}
// result (missing keys map to null)
{"values": {"user:42": {"name": "Ada", "tier": 3}, "user:99": null}}
```

- `keys` must be a non-empty array.
- The **whole** response is capped by `max_response_bytes`
  (`batch_get result exceeds max_response_bytes (> M)`) — you can't pull an
  unbounded blob back by batching many individually-legal values. Split the
  key set if you hit this.

### `db_incr` — atomic integer counter

```jsonc
// params (delta optional, defaults to 1; negative decrements)
{"key": "views", "delta": 1}
// result
{"ok": true, "value": 1}
```

- Runs in a single SQLite transaction: safe under concurrent writers — each
  `db_incr` sees the latest committed value and adds `delta` to it.
- A missing key starts the counter at `delta`.
- The stored value must be a JSON integer; anything else fails with
  `key '<k>' is not a counter: stored value is not an integer`. (Floats are
  not integers — store integers if you plan to `db_incr`.)
- A key's TTL, if any, is unaffected by an increment.

### `db_keys` — list key names

```jsonc
// params (prefix optional, defaults to "")
{"prefix": "user:"}
// result
{"keys": ["user:1", "user:42"]}
```

- Sorted ascending, keys only. No values.
- `prefix` filters with `key LIKE '<prefix>%'`, with `%`, `_`, and `\` in
  your prefix escaped — `{"prefix": "a%"}` matches the literal `a%x`, not
  `ax`.

### `db_append` — atomic append to a JSON-array value

```jsonc
// params
{"key": "events", "value": {"type": "click", "at": 1699900000000}}
// result
{"ok": true, "length": 3}
```

- Runs in a single transaction. A missing key starts a fresh array
  `[value]`; an existing key must hold a JSON array, otherwise
  `key '<k>' is not an array: cannot append`.
- `value` is any JSON — appended as one element, exactly like `db_set`'s
  value handling.
- The serialized result is re-checked against `max_value_bytes` (same
  rejection as `db_set`), so you can't grow an array past the cap one
  element at a time.

### `db_patch` — JSON-path update via `json_set`

```jsonc
// params
{"key": "user:42", "path": "$.settings.theme", "value": "dark"}
// result
{"ok": true, "value": {"name": "Ada", "settings": {"theme": "dark"}}}
```

- Runs in a single transaction. The key must exist (and not be expired):
  `key not found: '<k>'`.
- `path` is a SQLite JSON path — `$.a.b` for objects, `$[0]` for array
  indices (SQLite paths are 1-based for `$[1]` style subscripts on objects;
  array indices in `$[0]` form are 0-based). The new value is written at
  that path and the **full updated value** is returned.
- `value` is arbitrary JSON, injected as `json(?3)`. A malformed path
  surfaces as `invalid JSON path "<path>": …`; note SQLite silently ignores
  some syntactically odd paths (e.g. `$[`) and leaves the value unchanged —
  a returned `ok` with an unchanged value means the path didn't match
  anything.

## `db_query` — raw SQL against your own file

```jsonc
// params  (params is optional; defaults to [])
{"sql": "select key, updated_at from kv where key = ?1", "params": ["user:42"]}
// result
{"rows": [{"key": "user:42", "updated_at": 1699900000000}], "rows_affected": 0}
```

- **Positional binds only:** `?1`, `?2`, … map to `params[0]`, `params[1]`, ….
- **Any row-producing statement returns rows** — `SELECT`, `WITH … SELECT`,
  and `INSERT/UPDATE/DELETE … RETURNING`. Every statement also reports
  `rows_affected`. There is no `starts_with("select")` sniff, so a
  `RETURNING` write is never silently dropped:

```jsonc
// params
{"sql": "insert into kv (key, value, updated_at) values (?1, ?2, ?3) returning key, updated_at",
 "params": ["k", "\"v\"", 0]}
// result
{"rows": [{"key": "k", "updated_at": 0}], "rows_affected": 1}
```

- **`ATTACH` is rejected** (`ATTACH is not permitted in db_query statements`) —
  a whole-word, case-insensitive pre-check, so you cannot reach another
  caller's file. It's deliberately broad: `select 'attach' as label` is fine
  (quoted literal), but `select * from t /* attach */` is not — reword if you
  trip it.
- **Result size is capped** by `max_response_bytes`
  (`query result exceeds max_response_bytes (> M)`). The cap is enforced while
  streaming rows off the connection, so an oversized `SELECT` is rejected
  before it materializes in memory — but the check is server-side only. For
  large tables, paginate (`LIMIT`/`OFFSET` or a keyset cursor) rather than
  relying on the cap to fail you.

### Reading the `kv` table directly

`db_get` decodes stored JSON; `db_query` does **not**. The `value` column is
JSON text, so a stored string comes back quoted:

```jsonc
// after db_set {"key": "greeting", "value": "hi"}
{"sql": "select value from kv where key = ?1", "params": ["greeting"]}
// result — note the value is the JSON text "hi", quotes included
{"rows": [{"value": "\"hi\""}], "rows_affected": 0}
```

If you want typed, indexable columns, store them in your own table (see
[Recipes](#recipes)) instead of reaching into `kv`.

### Parameter binding — JSON → SQLite

| JSON param | Bound as | Note |
|---|---|---|
| `null` | SQL `NULL` | |
| `true` / `false` | integer `1` / `0` | **bools become integers** — `where flag = ?1` with `true` matches `1`, not a boolean column |
| integer (`42`) | INTEGER | |
| float (`3.5`) | REAL | |
| string (`"x"`) | TEXT | |
| array / object | TEXT | bound as its JSON string, e.g. `{"a":1}` → the text `{"a":1}` |

### Column decoding — SQLite → JSON

| SQLite column type | JSON in `rows` |
|---|---|
| `TEXT` | string |
| `INTEGER` | number |
| `REAL` | number |
| `NULL` (value is null) | `null` |
| `BLOB` | base64 string (standard alphabet) |
| anything else | error: `unsupported SQLite column type: <name>` |

Use `cast(col as text)` (or `hex(col)`, etc.) if a computed column comes back
with a type the plugin doesn't map.

## Errors

Every failure is an `ACTION_ERROR` with a plain-text `error`. The messages
below are stable enough to branch on by substring (`max_value_bytes`,
`SQLITE_FULL`, …) rather than exact-matching the whole string.

**Bad request (fix the params):**

| Message (shape) | Cause |
|---|---|
| `invalid params for db_get, expected {key}: …` | `params_json` didn't match the action's shape (same pattern for every action) |
| `params.key must be a non-empty string` | `db_get`/`db_set`/`db_delete`/`db_incr`/`db_append`/`db_patch` with empty `key` |
| `params.keys must be a non-empty array` | `db_batch_get` with `keys: []` |
| `params.path must be a non-empty string` | `db_patch` with empty/missing `path` |
| `params.sql must be a non-empty string` | `db_query` with blank `sql` |
| `unknown action: <name>` | action isn't one of the nine |

**Rejected by policy / limits:**

| Message (shape) | Cause |
|---|---|
| `missing caller_plugin_id (rejected before touching any database)` | no kernel-stamped caller id |
| `invalid caller_plugin_id: "<id>"` | caller id has chars outside `[a-zA-Z0-9_-]` |
| `value exceeds max_value_bytes (N > M)` | `db_set` value too large |
| `batch_get result exceeds max_response_bytes (> M)` | `db_batch_get` total too large |
| `query result exceeds max_response_bytes (> M)` | `db_query` rows too large |
| `ATTACH is not permitted in db_query statements` | `sql` contains the `attach` keyword |

**Storage / SQL runtime** (raw text from SQLite, surfaced as-is):

| Message (contains) | Cause | Recover by |
|---|---|---|
| `database or disk is full` / `SQLITE_FULL` (code 13) | write would cross the `max_db_bytes` quota | delete rows, or ask the operator to raise `max_db_bytes` |
| `near "…": syntax error` | malformed `sql` | fix the statement |
| `no such table` / `no such column` | querying something you never created | create your table first (see Recipes) |
| `UNIQUE constraint failed` | `INSERT` collides with a primary key / unique index | use upsert (`on conflict … do update`) |
| `corrupt stored value for key "<k>": …` | a `kv` `value` cell holds text that isn't valid JSON (e.g. written by a raw `INSERT` with a non-JSON string) | overwrite the key with `db_set`, or store valid JSON text via SQL |
| `key '<k>' is not a counter: stored value is not an integer` | `db_incr` on a value that isn't a JSON integer | store integers if you plan to `db_incr` |
| `key '<k>' is not an array: cannot append` | `db_append` on a value that isn't a JSON array | store arrays if you plan to `db_append` |
| `key not found: '<k>'` | `db_patch` on a missing or expired key | set the key first |
| `invalid JSON path "<p>": …` | `db_patch` with a malformed SQLite JSON path | fix the path; note some odd paths are silently ignored instead (see `db_patch`) |
| `handler panicked: …` | a bug in the plugin — should never happen; the loop converts the panic into this error instead of dropping your reply | report it |

## Recipes

### Your own tables

You're not limited to `kv`. `db_query` runs DDL, so give structured data real,
indexable columns:

```jsonc
{"sql": "create table if not exists sessions (id TEXT PRIMARY KEY, user_id INTEGER NOT NULL, expires_at INTEGER NOT NULL)"}
{"sql": "create index if not exists sessions_by_user on sessions (user_id)"}
```

Use `if not exists` so re-running on an already-initialized file is a no-op —
there's no separate "migrate" hook, so callers self-initialize on startup.

### Upsert from raw SQL

```jsonc
{"sql": "insert into sessions (id, user_id, expires_at) values (?1, ?2, ?3) on conflict(id) do update set expires_at = excluded.expires_at",
 "params": ["sess_abc", 42, 1699999999000]}
```

### Atomic multi-statement transactions

Separate `db_query` calls may each run on a **different** pooled connection, so
they are **not** one transaction. To make several writes atomic, put them in a
single `db_query` — the whole block runs on one connection, in order:

```jsonc
{"sql": "begin; update accounts set bal = bal - 10 where id = 1; update accounts set bal = bal + 10 where id = 2; commit;"}
```

If any statement fails, the `commit` isn't reached and the `begin` is rolled
back when the connection returns to the pool.

### TTL / expiry

The KV layer has built-in TTL: `db_set {key, value, ttl_ms}` stamps
`expires_at = now + ttl_ms`. Semantics:

- **Enforced everywhere.** Before every action (including raw `db_query`),
  the plugin sweeps `DELETE FROM kv WHERE expires_at <= now`, and the KV
  accessors (`db_get`, `db_batch_get`, `db_incr`, `db_append`, `db_patch`)
  additionally filter on `expires_at IS NULL OR expires_at > now` — so an
  expired key reads as missing, and a batch mixing expired and live keys
  returns `null` for the expired ones. `db_incr`/`db_append` on an expired
  key start fresh, `db_patch` reports `key not found`.
- **Absent/`0`/negative `ttl_ms` = no expiry**, and re-setting without
  `ttl_ms` clears a previous expiry (the upsert overwrites `expires_at`).
- **`expires_at` is visible to raw SQL** — it's a plain nullable INTEGER
  column on `kv` (unix ms). Your own tables don't get TTL; keep filtering
  them yourself (the sweep only touches `kv`).

```jsonc
// params — agent cache entry that dies in five minutes
{"key": "cache:weather", "value": {"temp": 21}, "ttl_ms": 300000}
```

### Change events

Every mutation — `db_set`, `db_delete` (only when a row was actually
deleted), `db_incr`, `db_append`, `db_patch` — publishes a best-effort
event of type `plugin.database.changed` (the kernel prepends the
`plugin.<sender_id>.` namespace, so subscribe to the fully-qualified type).
The payload is one JSON object:

```json
{"caller": "notes", "action": "db_set", "key": "user:42"}
```

Reads (`db_get`, `db_batch_get`, `db_keys`, `db_query`) publish nothing.
Publishing is fire-and-forget: the `ActionResponse` always goes out first,
and a dropped event never delays or fails your reply. The event fires only
if the `database` plugin holds `PERMISSION_EVENT_PUBLISH` (its manifest
declares it).

### Paginate large reads

Don't lean on `max_response_bytes` to stop a huge `SELECT` — page explicitly:

```jsonc
{"sql": "select id from sessions where id > ?1 order by id limit 100", "params": [""]}
```

Feed the last `id` back as `?1` for the next page (keyset pagination beats
`OFFSET` on big tables).

### Recovering from a full quota

A write that crosses `max_db_bytes` fails with a `SQLITE_FULL`-flavored error.
Free space by deleting rows: SQLite keeps the file at its high-water mark but
reuses the freed pages for later writes, so a full caller can write again once
it deletes enough. (`VACUUM` shrinks the file on disk, but it rebuilds the
whole database and can transiently need roughly double the pages — under a
tight `max_db_bytes` it may itself fail with `SQLITE_FULL`, so don't rely on
it to recover from a full quota.)

## FAQ

**Do I pass my plugin id?** No. The kernel stamps `caller_plugin_id`; the
plugin reads only that. There's no way to spoof another caller's namespace.

**Can I read another plugin's data?** No — separate files, and `ATTACH` is
blocked. Cross-caller access is an explicit non-goal.

**Is `db_get` faster than `db_query` on `kv`?** Marginally — same table, but
`db_get` skips the SQL parse and JSON-decodes for you. Use KV for
blob-by-key; use `db_query` when you need `WHERE`/joins/aggregates or your own
schema.

**Why did my boolean bind not match?** JSON `true`/`false` bind as integer
`1`/`0`. Compare against `1`/`0`, or store booleans as integers.

**Why is my stored string double-quoted in `db_query` output?** The `kv.value`
column is JSON text. `db_get` decodes it; raw SQL returns it verbatim. Store
plain columns in your own table if you want raw strings back.

**Are responses ordered?** Per SQL statement, yes. Across concurrent
`db_query` calls, no ordering is guaranteed — the kernel matches replies by
`action_id`, and calls may run on different pooled connections.

**What happens on restart?** Files persist under `data_dir`. Pools and the
`kv` table are (re)initialized lazily on the first call per caller, so there's
nothing to migrate by hand.
