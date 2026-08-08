# database plugin roadmap

v1 scope, isolation model, concurrency design, and kernel dependency are
fully specified in
`docs/superpowers/specs/2026-07-15-database-plugin-design.md` — this file
only tracks what's deliberately deferred.

## Non-goals (v1, from the design spec)

- No TTL/expiry on KV entries.
- No per-caller disk quotas.
- No cross-caller/admin actions (no list-namespaces, no admin query).
- No streaming query results — `max_response_bytes` cap instead.
- Not a vector store (`vector-db` is its own plugin per the root `ROADMAP.md`).

## Near-term follow-ups

- Swap the local path-override `veyron-wire`/`veyron-sdk` dependencies (see
  `Cargo.toml`) for published crates.io versions once the kernel changes in
  `docs/superpowers/plans/2026-07-17-database-plugin-kernel-support.md` are
  released.
- `notes`/`calendar` (root `ROADMAP.md` "Planned" table) become buildable
  once this ships — they're thin schema layers on top of `db_query`.
- `scheduler` depends on this plugin for persisting schedule state across
  kernel restarts (root `ROADMAP.md`).
