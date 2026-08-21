# notes plugin roadmap

v0.1 scope lives in `README.md`; this file tracks what's deliberately
deferred and what comes next.

## Implemented (v0.1)

- CRUD over `database`'s KV: `note:<id>` JSON documents, atomic id counter,
  prefix-scan listing with tag filter / sort / pagination.
- Best-effort `plugin.notes.changed` events after every mutation.
- Validation caps (title/body/tags/limit) rejected loudly at parse time.
- Channel-fronted RPC proxy so the serve loop stays the single reader of the
  connection (no frame-discarding hazard).

## Non-goals (v1)

- **No full-text/fuzzy search.** Tag filter + structured listing only;
  semantic search belongs to the planned `vector-db`/`search` plugins, not
  to a CRUD wrapper.
- **No per-user namespaces.** Single-kernel-user model; `database` already
  isolates per caller, and `notes` has exactly one caller identity.
- **No attachments/binary payloads.** `body` is text; binaries would blow
  through the value-size caps and belong in object storage, not KV docs.

## Near-term ideas

- `note_tags` — distinct tag list with usage counts (cheap client-side today,
  worth an action once agents need discovery).
- Pinned/archived flags with list filtering.
- Bulk export/import action for backup/migration.

## Scale ceiling

List operations materialize every stored note (one `db_batch_get`, capped by
`DATABASE_PLUGIN_MAX_RESPONSE_BYTES`, 4 MiB by default ≈ several thousand
typical notes). Beyond that, move listing to raw SQL via `db_query` or add a
secondary index-key scheme — decided when it hurts, not before.
