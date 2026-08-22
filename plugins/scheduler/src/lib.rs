//! `scheduler` plugin library crate: once/cron schedules over the
//! `database` plugin plus pure timing logic (`model.rs`).
//!
//! Thin schema layer (root `ROADMAP.md`): every schedule is one JSON
//! document stored under `sched:<id>` in this plugin's own `database`
//! namespace; generated ids come from an atomic `db_incr` counter
//! (`meta:next_id`). No local state — restart-safe by construction;
//! one-shots that came due while the plugin was down fire once on the next
//! scan with `late: true`, missed cron occurrences resume from "first
//! occurrence after now".
//!
//! Outbound calls go through [`Rpc`], a channel-fronted proxy: handler and
//! scan tasks never touch the `VynkorClient` directly, because
//! `send_action` discards every non-matching inbound frame while it waits —
//! a scan started by the timer would silently eat user requests arriving
//! mid-scan. With the proxy the serve loop stays the single reader
//! (calendar/sync-client rationale).

pub mod model;
pub mod request;
pub mod store;

use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use vynkor_sdk::proto::{envelope, Envelope, EventPublish};

use model::{DueFire, ScheduleDoc};
use request::{NewSchedule, SchedulerRequest};

/// Runtime configuration (environment-driven; see `config.example.yaml`).
#[derive(Debug, Clone)]
pub struct Config {
    /// Per-call timeout for `database` and fired-action IPC round-trips.
    pub db_timeout_ms: u32,
    /// Scan interval in seconds; `0` disables scanning entirely. Bounds
    /// firing precision: a due schedule fires within one interval.
    pub scan_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_timeout_ms: 5000,
            scan_secs: 30,
        }
    }
}

impl Config {
    pub fn from_env() -> Self {
        let db_timeout_ms = std::env::var("SCHEDULER_PLUGIN_DB_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5000);
        let scan_secs = std::env::var("SCHEDULER_PLUGIN_SCAN_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);
        Self {
            db_timeout_ms,
            scan_secs,
        }
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
/// (`database`, fired-action targets). Every [`Rpc::call`] round-trips
/// through the serve loop's single `recv()` point.
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
            .send(RpcCall {
                action: action.to_string(),
                params_json,
                timeout_ms,
                reply,
            })
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
/// (the kernel namespaces the type to `plugin.scheduler.changed`/`.fired`).
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

/// Handle one kernel-routed action. Storage failures surface as `Err` →
/// `ACTION_ERROR`; reading a missing schedule is `{"found": false}`,
/// deleting a missing one `{"deleted": false}` — neither is an error.
pub async fn handle_action(
    rpc: Rpc,
    config: &Config,
    action: &str,
    params_json: &[u8],
    now_ms: i64,
) -> Result<ActionResult, String> {
    let req = request::parse_request(action, params_json, now_ms)?;
    let db = store::Db::new(rpc, config.db_timeout_ms);
    match req {
        SchedulerRequest::Set(new) => {
            let (id, created, doc) = match &new.id {
                Some(id) => {
                    let existing = db.get(id).await?;
                    let created = existing.is_none();
                    let created_at = existing.as_ref().map(|d| d.created_at_ms).unwrap_or(now_ms);
                    (
                        id.clone(),
                        created,
                        build_doc(id.clone(), new, created_at, now_ms),
                    )
                }
                None => {
                    let id = db.next_id().await?.to_string();
                    (id.clone(), true, build_doc(id, new, now_ms, now_ms))
                }
            };
            db.put(&doc).await?;
            ok(
                json!({"id": id, "created": created, "schedule": doc}),
                Some(changed(
                    if created { "created" } else { "updated" },
                    &doc.id,
                )),
            )
        }
        SchedulerRequest::Get { id } => match db.get(&id).await? {
            Some(doc) => ok(json!({"found": true, "schedule": doc}), None),
            None => ok(json!({"found": false, "schedule": null}), None),
        },
        SchedulerRequest::List { limit, offset } => {
            let mut docs = db.list().await?;
            let total = docs.len();
            // Soonest deadline first; pending-less (done/disabled) last,
            // numeric-id tie-break for determinism. Rust orders `None`
            // before `Some`, so the none-ness flag must lead the key.
            let key = |doc: &ScheduleDoc| {
                let next = model::next_fire_ms(doc);
                (next.is_none(), next, id_num(doc))
            };
            docs.sort_by_key(key);
            let page: Vec<&ScheduleDoc> = docs.iter().skip(offset).take(limit).collect();
            ok(json!({"schedules": page, "total": total}), None)
        }
        SchedulerRequest::Delete { id } => {
            let deleted = db.delete(&id).await?;
            let event = deleted.then(|| changed("deleted", &id));
            ok(json!({"deleted": deleted}), event)
        }
    }
}

fn id_num(doc: &ScheduleDoc) -> u64 {
    doc.id.parse::<u64>().unwrap_or(0)
}

fn build_doc(id: String, new: NewSchedule, created_at_ms: i64, now_ms: i64) -> ScheduleDoc {
    ScheduleDoc {
        id,
        name: new.name,
        enabled: new.enabled,
        trigger: new.trigger,
        fire: new.fire,
        fired_once: false,
        fire_count: 0,
        last_fired_ms: None,
        last_error: None,
        created_at_ms,
        updated_at_ms: now_ms,
    }
}

/// Scan all schedules once and fire everything due. Per fire, in order:
/// persist the marks FIRST (`fired_once`, counters — at-most-once, a crash
/// between mark and dispatch loses that one fire), then dispatch the event
/// or action. A failed action dispatch records `last_error` on the document
/// and never aborts the remaining fires.
pub async fn scan_due(
    rpc: Rpc,
    outbound: mpsc::Sender<Envelope>,
    config: &Config,
    now_ms: i64,
) -> Result<usize, String> {
    let fires = {
        let db = store::Db::new(rpc.clone(), config.db_timeout_ms);
        let docs = db.list().await?;
        model::due_schedules(&docs, now_ms)
    };

    for fire in &fires {
        let marked = mark_fired(&rpc, config, fire, now_ms).await;

        match &marked.doc.fire {
            model::Fire::Event { payload } => {
                let envelope_payload = json!({
                    "schedule_id": marked.doc.id,
                    "name": marked.doc.name,
                    "scheduled_for_ms": marked.scheduled_for_ms,
                    "late": marked.late,
                    "fire_count": marked.fire_count,
                    "fire": marked.doc.fire,
                    "payload": payload,
                });
                let publish = Envelope {
                    payload: Some(envelope::Payload::EventPublish(EventPublish {
                        event_type: "fired".into(),
                        payload_json: envelope_payload.to_string().into_bytes(),
                    })),
                    ..Default::default()
                };
                if let Err(e) = outbound.send(publish).await {
                    eprintln!("[scheduler] fired-event publish failed: {e}");
                }
            }
            model::Fire::Action { name, params } => {
                if let Err(e) = rpc.call(name, params.clone(), config.db_timeout_ms).await {
                    eprintln!(
                        "[scheduler] scheduled action {:?} failed: {e}",
                        marked.doc.id
                    );
                    let _ = record_last_error(&rpc, config, &marked.doc.id, &e).await;
                }
            }
        }
    }
    Ok(fires.len())
}

struct MarkedFire {
    doc: ScheduleDoc,
    scheduled_for_ms: i64,
    late: bool,
    fire_count: u64,
}

async fn mark_fired(rpc: &Rpc, config: &Config, fire: &DueFire, now_ms: i64) -> MarkedFire {
    let db = store::Db::new(rpc.clone(), config.db_timeout_ms);
    let mut doc = fire.doc.clone();
    if matches!(doc.trigger, model::Trigger::Once { .. }) {
        doc.fired_once = true;
    }
    doc.fire_count += 1;
    // Record the SCHEDULED instant, not wall-clock now: cron anchoring
    // resumes from here, and one-shot audit reads stay truthful.
    doc.last_fired_ms = Some(fire.scheduled_for_ms);
    doc.last_error = None;
    doc.updated_at_ms = now_ms;
    if let Err(e) = db.put(&doc).await {
        // At-most-once degrades here: the mark didn't stick, so a crash
        // before the next successful put could re-fire. Loud over silent.
        eprintln!(
            "[scheduler] failed to persist fire mark for {}: {e}",
            doc.id
        );
    }
    MarkedFire {
        fire_count: doc.fire_count,
        doc,
        scheduled_for_ms: fire.scheduled_for_ms,
        late: fire.late,
    }
}

async fn record_last_error(
    rpc: &Rpc,
    config: &Config,
    id: &str,
    error: &str,
) -> Result<(), String> {
    let db = store::Db::new(rpc.clone(), config.db_timeout_ms);
    let mut doc = db
        .get(id)
        .await?
        .ok_or_else(|| format!("schedule vanished: {id}"))?;
    let mut msg = error.to_string();
    msg.truncate(256);
    doc.last_error = Some(msg);
    doc.updated_at_ms = store::now_ms();
    db.put(&doc).await
}

fn ok(data: Value, event: Option<ChangeEvent>) -> Result<ActionResult, String> {
    let data = serde_json::to_vec(&data).map_err(|e| format!("failed to encode response: {e}"))?;
    Ok(ActionResult { data, event })
}

fn changed(op: &'static str, id: &str) -> ChangeEvent {
    ChangeEvent {
        event_type: "changed",
        payload: json!({"op": op, "id": id}),
    }
}
