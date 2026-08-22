# scheduler roadmap

Delivered in v0.1 (this release): `schedule_set`/`get`/`list`/`delete`,
absolute and delay-resolved one-shots, 5/6-field cron expressions with fixed
UTC offsets, event- and action-mode fires, at-most-once marks before
dispatch, late-flag catch-up after downtime, `plugin.scheduler.fired` /
`.changed` events, `last_error` diagnostics on failed action dispatches.

Non-exhaustive notes on what v0.1 deliberately does NOT do:

## Non-goals (for now)

- **Deadline-heap engine.** Firing precision is bounded by the scan interval
  (`SCHEDULER_PLUGIN_SCAN_SECS`); the scan model is the proven calendar/
  sync-client shape and keeps the serve loop trivially correct. A
  sleep-until-next-deadline loop (min-heap over pending schedules, woken by
  mutations through the RPC proxy) would tighten precision to milliseconds —
  worth it only when a consumer actually needs sub-interval timing.
- **Named IANA timezones.** `tz_offset_min` fixed offsets only. chrono-tz
  would pull a large data crate for a need nobody has expressed yet;
  DST-correct wall-clock crons are the actual feature and can be layered on
  later without a wire/manifest change (additive trigger field).
- **Backfilling missed cron occurrences.** Downtime collapses into at most
  one catch-up fire (anchored resume). Backfill semantics ("run every missed
  tick") are a different contract and easy to add as an opt-in trigger flag
  if something needs it.
- **Jitter / randomization** for recurring schedules (thundering-herd
  avoidance matters at fleet scale, not personal-kernel scale).
- **Retention pruning.** Done one-shots accumulate until deleted. They are
  small JSON docs and double as audit history; prune-by-age is a candidate
  follow-up if listing gets noisy.
- **Retry / redelivery.** A failed action dispatch records `last_error` and
  moves on — at-most-once means exactly that. Retries belong to the caller
  (or a future explicit policy field), not hidden inside the scanner.
- **Per-target allowlists.** The kernel's T-19 check already prevents
  privilege laundering: fired calls carry scheduler's own claims, so gated
  targets fail unless the operator grants them. An env-var target allowlist
  would duplicate that decision one level down.

## Open options

- Migrating `calendar`'s reminder firing onto `scheduler` stays an open
  option (root `ROADMAP.md`); nothing in this plugin assumes it.
- `agent`'s tool-call loop may want scheduled tool invocations; if its shape
  demands richer triggers (intervals, dependencies between schedules), add
  them here rather than inside `agent`.

Considered and skipped: webhook/HTTP fire mode (that's `network` +
`ai`-style composition via action mode already), distributed locks (single
kernel, single scheduler instance), second-precision guarantees under the
scan model (contradicts the design above — switch engines first).
