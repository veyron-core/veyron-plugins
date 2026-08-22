//! Pure schedule model for the `scheduler` plugin.
//!
//! A schedule is one JSON document (`sched:<id>` in the plugin's own
//! `database` namespace). Trigger and fire shapes are plain serde enums so
//! the stored document round-trips without a schema migration path; the
//! timing math ([`next_fire_ms`], [`next_cron_after_ms`]) is a pure function
//! so the scan loop stays trivial and testable (same split as calendar's
//! `reminders.rs`).

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// One schedule: what fires (`fire`) and when (`trigger`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleDoc {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Disabled schedules persist but never fire; `schedule_list` reports
    /// them with `next_fire_ms: null`.
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub trigger: Trigger,
    pub fire: Fire,
    /// One-shot guard — set BEFORE dispatching (at-most-once; see README).
    #[serde(default)]
    pub fired_once: bool,
    /// Total fires so far (one-shots cap at 1).
    #[serde(default)]
    pub fire_count: u64,
    #[serde(default)]
    pub last_fired_ms: Option<i64>,
    /// Best-effort diagnostics from the most recent dispatch attempt
    /// (truncated); cleared on the next successful fire.
    #[serde(default)]
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

fn default_true() -> bool {
    true
}

/// When a schedule fires. Stored resolved: `Once` always carries the
/// absolute `at_ms` (a `delay_ms` request is resolved at `schedule_set`
/// time), so restarts never re-derive a moving target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    Once {
        at_ms: i64,
    },
    Cron {
        expr: String,
        /// UTC offset in minutes (−720..=840) the expression evaluates in;
        /// 0 = UTC. Named IANA zones are a non-goal for v0.1.
        #[serde(default)]
        tz_offset_min: i32,
    },
}

/// What a fire delivers: a best-effort `plugin.scheduler.fired` event, or a
/// kernel-routed action call to whichever plugin serves `name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Fire {
    Event {
        #[serde(default)]
        payload: serde_json::Value,
    },
    Action {
        name: String,
        #[serde(default)]
        params: serde_json::Value,
    },
}

/// One due schedule snapshot handed to the dispatcher by [`due_schedules`].
#[derive(Debug, Clone, PartialEq)]
pub struct DueFire {
    pub doc: ScheduleDoc,
    /// The instant this fire was scheduled for: the stored `at_ms` for
    /// one-shots, the computed occurrence for cron.
    pub scheduled_for_ms: i64,
    /// True when `now_ms` already passed `scheduled_for_ms`. Normal for a
    /// one-shot that came due while the plugin was down (calendar's `late`
    /// precedent); for cron it only shows sub-scan-interval overshoot.
    pub late: bool,
}

/// Earliest instant the schedule should fire at, or `None` when it has
/// nothing pending (disabled, done one-shot, unparseable cron).
pub fn next_fire_ms(doc: &ScheduleDoc) -> Option<i64> {
    if !doc.enabled {
        return None;
    }
    match &doc.trigger {
        Trigger::Once { at_ms } => {
            if doc.fired_once {
                None
            } else {
                Some(*at_ms)
            }
        }
        Trigger::Cron {
            expr,
            tz_offset_min,
        } => {
            // Anchor to what we have already accounted for — the last
            // scheduled fire, or creation for fresh docs. Anchoring to
            // `now` instead chases its own tail whenever the expression's
            // granularity matches the scan interval: the "first occurrence
            // strictly after now" would sit an epsilon ahead of every tick
            // and never come due.
            let reference = doc.last_fired_ms.unwrap_or(doc.created_at_ms);
            match next_cron_after_ms(expr, *tz_offset_min, reference) {
                Ok(next) => next,
                Err(e) => {
                    // Corrupt stored expression: skip loudly instead of
                    // failing the whole scan (same policy as corrupt
                    // documents in list).
                    eprintln!("[scheduler] schedule {}: {e}", doc.id);
                    None
                }
            }
        }
    }
}

/// Select every schedule due at `now_ms`, preserving input order.
pub fn due_schedules(docs: &[ScheduleDoc], now_ms: i64) -> Vec<DueFire> {
    docs.iter()
        .filter_map(|doc| {
            let deadline = next_fire_ms(doc)?;
            if deadline > now_ms {
                return None;
            }
            Some(DueFire {
                doc: doc.clone(),
                scheduled_for_ms: deadline,
                late: now_ms > deadline,
            })
        })
        .collect()
}

/// Standard 5-field crontab expressions are accepted by prefixing the
/// seconds field (`0`); 6-field (seconds) expressions pass through as-is.
/// Whitespace is collapsed so odd input spacing can't reach the parser.
pub fn normalize_cron_expr(expr: &str) -> String {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() == 5 {
        format!("0 {}", fields.join(" "))
    } else {
        fields.join(" ")
    }
}

fn fixed_offset(tz_offset_min: i32) -> Result<FixedOffset, String> {
    // chrono's FixedOffset accepts ±24h, but crontab convention (and our
    // manifest docs) bound offsets to −12:00..+14:00 — enforce that here.
    if !(-720..=840).contains(&tz_offset_min) {
        return Err(format!(
            "tz_offset_min out of range (-720..=840): {tz_offset_min}"
        ));
    }
    FixedOffset::east_opt(tz_offset_min * 60)
        .ok_or_else(|| format!("tz_offset_min out of range (-720..=840): {tz_offset_min}"))
}

/// Parse + validate a cron expression without computing anything
/// (`schedule_set` rejects invalid expressions up front).
pub fn validate_cron_expr(expr: &str, tz_offset_min: i32) -> Result<(), String> {
    let normalized = normalize_cron_expr(expr);
    Schedule::from_str(&normalized)
        .map(|_| ())
        .map_err(|e| format!("invalid cron expression {expr:?}: {e}"))?;
    fixed_offset(tz_offset_min)?;
    Ok(())
}

/// First cron occurrence strictly after `after_ms`, evaluated in the given
/// fixed UTC offset. Missed occurrences during downtime are never backfilled
/// — the next scan resumes from "first occurrence after now".
pub fn next_cron_after_ms(
    expr: &str,
    tz_offset_min: i32,
    after_ms: i64,
) -> Result<Option<i64>, String> {
    let offset = fixed_offset(tz_offset_min)?;
    let normalized = normalize_cron_expr(expr);
    let sched = Schedule::from_str(&normalized)
        .map_err(|e| format!("invalid cron expression {expr:?}: {e}"))?;
    let after: DateTime<FixedOffset> = offset
        .timestamp_millis_opt(after_ms)
        .single()
        .ok_or_else(|| format!("timestamp out of range: {after_ms}"))?;
    Ok(sched
        .after(&after)
        .next()
        .map(|dt| dt.with_timezone(&Utc).timestamp_millis()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn once(at_ms: i64) -> ScheduleDoc {
        ScheduleDoc {
            id: "1".into(),
            name: None,
            enabled: true,
            trigger: Trigger::Once { at_ms },
            fire: Fire::Event { payload: json!({}) },
            fired_once: false,
            fire_count: 0,
            last_fired_ms: None,
            last_error: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn once_next_follows_lifecycle() {
        assert_eq!(next_fire_ms(&once(1_000)), Some(1_000));
        // Done one-shot has nothing pending.
        let mut done = once(1_000);
        done.fired_once = true;
        assert_eq!(next_fire_ms(&done), None);
        // Disabled never fires, even when overdue.
        let mut off = once(1_000);
        off.enabled = false;
        assert_eq!(next_fire_ms(&off), None);
    }

    #[test]
    fn due_selects_overdue_and_flags_late() {
        let docs = [once(900), once(5_000)];
        let fires = due_schedules(&docs, 1_000);
        assert_eq!(fires.len(), 1);
        assert_eq!(fires[0].scheduled_for_ms, 900);
        assert!(fires[0].late);

        // Exactly at the deadline: due but not late. The earlier schedule is
        // marked done first — selection is pure and keeps no fire state.
        let mut done = once(900);
        done.fired_once = true;
        let docs = [done, once(5_000)];
        let fires = due_schedules(&docs, 5_000);
        assert_eq!(fires.len(), 1);
        assert_eq!(fires[0].scheduled_for_ms, 5_000);
        assert!(!fires[0].late);
    }

    #[test]
    fn five_field_cron_is_prefixed_with_seconds() {
        assert_eq!(normalize_cron_expr("*/5 * * * *"), "0 */5 * * * *");
        assert_eq!(normalize_cron_expr("*/5 * * * * *"), "*/5 * * * * *");
        assert_eq!(normalize_cron_expr("  0   9 * * MON "), "0 0 9 * * MON");
    }

    #[test]
    fn cron_next_after_computes_in_fixed_offset() {
        let offset = fixed_offset(180).unwrap();
        // Daily at 01:30 wall clock; reference instant 00:10+03 → next fire
        // is the same day's 01:30+03.
        let base = offset
            .with_ymd_and_hms(2026, 1, 15, 0, 10, 0)
            .unwrap()
            .timestamp_millis();
        let next = next_cron_after_ms("30 1 * * *", 180, base)
            .unwrap()
            .unwrap();
        let next_dt: DateTime<FixedOffset> = offset.timestamp_millis_opt(next).single().unwrap();
        assert_eq!(
            next_dt.format("%Y-%m-%d %H:%M:%S %:z").to_string(),
            "2026-01-15 01:30:00 +03:00"
        );

        // The identical wall-clock rule under UTC yields a different
        // absolute instant — proof the offset actually participates.
        // (Minute-boundary expressions would NOT differ: whole-hour offsets
        // share every minute tick, hence a hour-anchored expression here.)
        let utc_next = next_cron_after_ms("30 1 * * *", 0, base).unwrap().unwrap();
        assert_ne!(utc_next, next);
    }

    #[test]
    fn cron_seconds_expression_advances_by_one_second() {
        let base = 1_755_000_000_123i64;
        let next = next_cron_after_ms("* * * * * *", 0, base).unwrap().unwrap();
        assert_eq!(next, base - (base % 1000) + 1000);
    }

    #[test]
    fn cron_validation_rejects_garbage_and_bad_offsets() {
        assert!(validate_cron_expr("not a cron", 0).is_err());
        assert!(validate_cron_expr("*/5 * * * *", 0).is_ok());
        assert!(validate_cron_expr("*/5 * * * *", 180).is_ok());
        assert!(validate_cron_expr("*/5 * * * *", 900).is_err());
        // 6-field with names works too.
        assert!(validate_cron_expr("0 0 9 * JAN MON", 0).is_ok());
    }
}
