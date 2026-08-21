//! `calendar` plugin library crate: event CRUD over the `database` plugin
//! plus pure reminder-selection logic.
//!
//! Thin schema layer (root `ROADMAP.md`): every event is one JSON document
//! stored under `event:<id>` in this plugin's own `database` namespace, ids
//! come from an atomic `db_incr` counter (`meta:next_id`). No local state —
//! restart-safe by construction; reminders that came due while the plugin
//! was down fire once on the next scan with `late: true`.
//!
//! Outbound calls go through [`Rpc`], a channel-fronted proxy: handler and
//! scan tasks never touch the `VeyronClient` directly, because `send_action`
//! discards every non-matching inbound frame while it waits — a scan started
//! by the timer would silently eat user requests arriving mid-scan. With the
//! proxy the serve loop stays the single reader: user requests, pings and
//! publish-acks are always matched and routed, nothing is dropped (same
//! rationale as sync-client's custom loop).

pub mod reminders;
pub mod request;
pub mod store;

use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use veyron_sdk::proto::{envelope, Envelope, EventPublish};

use reminders::DueFire;
use store::EventDoc;

/// Runtime configuration (environment-driven; see `config.example.yaml`).
#[derive(Debug, Clone)]
pub struct Config {
    /// Per-call timeout for `database` IPC round-trips.
    pub db_timeout_ms: u32,
    /// Reminder scan interval in seconds; `0` disables scanning entirely.
    pub scan_secs: u64,
    /// Deliver fired reminders through the `notify` plugin as well
    /// (best-effort — a notify failure never fails the scan).
    pub notify_enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self { db_timeout_ms: 5000, scan_secs: 30, notify_enabled: true }
    }
}

impl Config {
    pub fn from_env() -> Self {
        let db_timeout_ms = std::env::var("CALENDAR_PLUGIN_DB_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5000);
        let scan_secs = std::env::var("CALENDAR_PLUGIN_SCAN_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);
        let notify_enabled = std::env::var("CALENDAR_PLUGIN_NOTIFY")
            .ok()
            .map(|s| {
                let s = s.trim().to_ascii_lowercase();
                !s.is_empty() && s != "false" && s != "0"
            })
            .unwrap_or(true);
        Self { db_timeout_ms, scan_secs, notify_enabled }
    }
}

/// One pending kernel-routed call handed from a task to the serve loop,
/// which sends it and correlates the `ActionResponse` by `action_id`.
pub struct RpcCall {
    pub action: String,
    pub params_json: Vec<u8>,
    pub timeout_ms: u32,
    pub reply: oneshot::Sender<Result<Value, String>>,
}

/// Cloneable handle for kernel-routed actions into other plugins
/// (`database`, `notify`). Every [`Rpc::call`] round-trips through the serve
/// loop's single `recv()` point.
#[derive(Clone)]
pub struct Rpc {
    tx: mpsc::Sender<RpcCall>,
}

impl Rpc {
    pub fn new(tx: mpsc::Sender<RpcCall>) -> Self {
        Self { tx }
    }

    /// One kernel-routed action round-trip. Resolves to the decoded
    /// `data_json` payload on `ACTION_OK`; transport failures, non-OK
    /// statuses and timeouts all surface as `Err` naming the target action.
    pub async fn call(
        &self,
        action: &str,
        params: Value,
        timeout_ms: u32,
    ) -> Result<Value, String> {
        let params_json = serde_json::to_vec(&params)
            .map_err(|e| format!("failed to encode {action} params: {e}"))?;
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(RpcCall { action: action.to_string(), params_json, timeout_ms, reply })
            .await
            .map_err(|_| format!("{action} aborted: serve loop is shutting down"))?;
        let effective = if timeout_ms == 0 { 30_000 } else { timeout_ms };
        match tokio::time::timeout(std::time::Duration::from_millis(effective as u64), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(format!("{action} aborted: serve loop is shutting down")),
            Err(_) => Err(format!("{action} timed out after {effective} ms")),
        }
    }
}

/// Best-effort change event to publish AFTER the action response is sent
/// (the kernel namespaces the type to `plugin.calendar.changed` / `.due`).
#[derive(Debug)]
pub struct ChangeEvent {
    pub event_type: &'static str,
    pub payload: Value,
}

/// One handled action: the response payload plus an optional change event.
#[derive(Debug)]
pub struct ActionResult {
    pub data: Vec<u8>,
    pub event: Option<ChangeEvent>,
}

/// Handle one kernel-routed action. Storage failures (including "not found"
/// on update) surface as `Err` → `ACTION_ERROR`; reading a missing event is
/// a `{"found": false}` result, deleting a missing one a `{"deleted": false}`
/// result — neither is an error.
pub async fn handle_action(
    rpc: Rpc,
    config: &Config,
    action: &str,
    params_json: &[u8],
) -> Result<ActionResult, String> {
    let req = request::parse_request(action, params_json)?;
    let db = store::Db::new(rpc, config.db_timeout_ms);
    match req {
        request::CalendarRequest::Create(e) => {
            let id = db.next_id().await?.to_string();
            let now = store::now_ms();
            let doc = EventDoc {
                id: id.clone(),
                title: e.title,
                description: e.description,
                start_ms: e.start_ms,
                end_ms: e.end_ms,
                all_day: e.all_day,
                remind_before_ms: e.remind_before_ms,
                reminder_fired: false,
                tags: e.tags,
                created_at_ms: now,
                updated_at_ms: now,
            };
            db.put(&doc).await?;
            ok(json!({"id": id, "event": doc}), Some(changed("created", &doc.id)))
        }
        request::CalendarRequest::Get { id } => match db.get(&id).await? {
            Some(doc) => ok(json!({"found": true, "event": doc}), None),
            None => ok(json!({"found": false, "event": null}), None),
        },
        request::CalendarRequest::List { from_ms, to_ms, tag, limit, offset } => {
            let mut events = db.list().await?;
            if let Some(from) = from_ms {
                events.retain(|e| e.start_ms >= from);
            }
            if let Some(to) = to_ms {
                events.retain(|e| e.start_ms <= to);
            }
            if let Some(tag) = tag {
                events.retain(|e| e.tags.iter().any(|t| t == &tag));
            }
            // Chronological order; deterministic tie-break on numeric id asc.
            events.sort_by(|a, b| {
                a.start_ms.cmp(&b.start_ms).then_with(|| id_num(a).cmp(&id_num(b)))
            });
            let total = events.len();
            let page: Vec<&EventDoc> = events.iter().skip(offset).take(limit).collect();
            ok(json!({"events": page, "total": total}), None)
        }
        request::CalendarRequest::Update { id, patch } => {
            let mut doc =
                db.get(&id).await?.ok_or_else(|| format!("event not found: {id}"))?;
            if let Some(t) = patch.title {
                doc.title = t;
            }
            if let Some(d) = patch.description {
                doc.description = d;
            }
            // Any change to the time shape invalidates a previously fired
            // reminder — the new schedule deserves a new notification.
            let mut times_changed = false;
            if let Some(s) = patch.start_ms {
                if doc.start_ms != s {
                    times_changed = true;
                }
                doc.start_ms = s;
            }
            if let Some(e) = patch.end_ms {
                if doc.end_ms != Some(e) {
                    times_changed = true;
                }
                doc.end_ms = Some(e);
            }
            if let Some(a) = patch.all_day {
                doc.all_day = a;
            }
            if let Some(rb) = patch.remind_before_ms {
                if doc.remind_before_ms != Some(rb) {
                    times_changed = true;
                }
                doc.remind_before_ms = Some(rb);
            }
            if let Some(t) = patch.tags {
                doc.tags = t;
            }
            if let Some(end) = doc.end_ms {
                if end < doc.start_ms {
                    return Err(format!(
                        "resulting end_ms must be >= start_ms ({} < {})",
                        end, doc.start_ms
                    ));
                }
            }
            if times_changed {
                doc.reminder_fired = false;
            }
            doc.updated_at_ms = store::now_ms();
            db.put(&doc).await?;
            ok(
                json!({"updated": true, "event": doc}),
                Some(changed("updated", &doc.id)),
            )
        }
        request::CalendarRequest::Delete { id } => {
            let deleted = db.delete(&id).await?;
            let event = deleted.then(|| changed("deleted", &id));
            ok(json!({"deleted": deleted}), event)
        }
    }
}

/// Scan all events once and fire every due reminder. Per reminder, in order:
/// mark fired FIRST (at-most-once — a crash between mark and publish loses
/// one notification, while the reverse order would re-fire forever), queue
/// the `due` event fire-and-forget, then best-effort `notify_send`. A failure
/// in publish/notify never aborts the remaining reminders.
pub async fn scan_due(
    rpc: Rpc,
    outbound: mpsc::Sender<Envelope>,
    config: &Config,
    now_ms: i64,
) -> Result<usize, String> {
    let fires = {
        let db = store::Db::new(rpc.clone(), config.db_timeout_ms);
        let events = db.list().await?;
        reminders::due_events(&events, now_ms)
    };

    for fire in &fires {
        {
            let db = store::Db::new(rpc.clone(), config.db_timeout_ms);
            let mut doc = fire.event.clone();
            doc.reminder_fired = true;
            db.put(&doc).await?;
        }

        let payload = json!({
            "event_id": fire.event.id,
            "title": fire.event.title,
            "start_ms": fire.event.start_ms,
            "remind_at_ms": fire.remind_at_ms,
            "late": fire.late,
        });
        let publish = Envelope {
            payload: Some(envelope::Payload::EventPublish(EventPublish {
                event_type: "due".into(),
                payload_json: payload.to_string().into_bytes(),
            })),
            ..Default::default()
        };
        if let Err(e) = outbound.send(publish).await {
            eprintln!("[calendar] due publish failed: {e}");
        }

        if config.notify_enabled {
            let params = json!({
                "title": "Calendar",
                "message": reminder_message(fire, now_ms),
            });
            if let Err(e) = send_notify(&rpc, config.db_timeout_ms, &params).await {
                eprintln!("[calendar] notify_send failed: {e}");
            }
        }
    }
    Ok(fires.len())
}

async fn send_notify(rpc: &Rpc, timeout_ms: u32, params: &Value) -> Result<(), String> {
    rpc.call("notify_send", params.clone(), timeout_ms).await.map(|_| ())
}

fn reminder_message(fire: &DueFire, now_ms: i64) -> String {
    if fire.late {
        format!("{} — missed reminder (started earlier)", fire.event.title)
    } else if fire.event.remind_before_ms == Some(0) {
        format!("{} starts now", fire.event.title)
    } else {
        let minutes = (fire.event.start_ms - now_ms) / 60_000;
        if minutes >= 1 {
            format!("{} starts in ~{minutes} min", fire.event.title)
        } else {
            format!("{} starts now", fire.event.title)
        }
    }
}

fn id_num(e: &EventDoc) -> u64 {
    e.id.parse::<u64>().unwrap_or(0)
}

fn ok(data: Value, event: Option<ChangeEvent>) -> Result<ActionResult, String> {
    let data =
        serde_json::to_vec(&data).map_err(|e| format!("failed to encode response: {e}"))?;
    Ok(ActionResult { data, event })
}

fn changed(op: &'static str, id: &str) -> ChangeEvent {
    ChangeEvent { event_type: "changed", payload: json!({"op": op, "id": id}) }
}
