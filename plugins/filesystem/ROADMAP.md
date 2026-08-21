# filesystem plugin roadmap

Sandboxed local file browse/read/write: `fs_list` / `fs_read` / `fs_write`
over an operator-configured allowlist of absolute directory roots. No
outbound RPC, no network, no secrets — plain `std::fs` behind the sandbox in
`src/sandbox.rs`.

## v1 scope (shipped, 0.1.0)

- `fs_list` — read-only browse: entries with lstat kinds (`file`/`dir`/
  `symlink`/`other`), dirs-first sort, dotfile filter, entry cap +
  `truncated` flag.
- `fs_read` — windowed read (`offset`/`max_bytes`, hard cap 8 MiB),
  utf8-or-base64 encoding rule (utf8 only for a whole-file read from
  offset 0 that is valid UTF-8), `truncated` flag.
- `fs_write` — full-file create-or-overwrite from `text` xor
  `content_base64`, opt-in `create_parents`.
- Sandbox: absolute paths only; deepest-existing-ancestor canonicalize;
  `..` surviving into the non-existing remainder rejected; containment
  check against canonical roots; symlink-final-component refusal on write.
- Default-deny: unset/empty `FILES_PLUGIN_ALLOWED_ROOTS` rejects every
  action.

## Non-goals (v1)

- **No exec, no shell** — ever. This plugin reads and writes bytes; running
  programs is what the kernel's supervisor model exists to prevent. This is
  the reason the previously considered `shell` plugin was rejected (root
  `ROADMAP.md`, "Considered and skipped").
- **No delete/rename/move/copy** in v1 — destructive or mutating-beyond-
  write operations want their own permission surface discussion first.
- **No append mode** — v1 writes are whole-file only; partial mutation is
  better served by `database`'s KV primitives.
- **No streaming/chunked write** — one request carries the whole payload;
  large-file upload wants a streaming envelope first.
- **TOCTOU** — the sandbox resolves a path, then I/O happens on it; a
  concurrent symlink swap between the two is not defended against in v1.
  Defending it needs openat2-style `RESOLVE_BENEATH` semantics (Linux-only)
  or re-verification after open; revisit if multi-writer roots become a
  real deployment shape.
- **No watch/notification** — file-change events belong to a future design,
  not bolted onto this plugin.

## Near-term ideas

- `fs_stat` — single-entry metadata probe without listing the parent dir.
- Root-scoped relative paths (`{"root": "data", "path": "a/b.txt"}`) so
  callers don't need to know host absolute paths — convenience only, the
  sandbox already enforces containment.
- Hash digest option on `fs_read` (`sha256`) for integrity checks by
  callers like a future sync engine.

## Unblocks

- `launcher` (root `ROADMAP.md` Planned table): reading app manifests /
  `.desktop` files via `fs_read` once its own permission model lands.
