//! Action handlers: turn `ActionRequest.params_json` into the response
//! `data_json` the caller gets back.

use crate::inbox::{self, Inbox};
use crate::providers::{self, ProviderKind};
use crate::request::{IdParams, ListParams, NotifyParams};
use vynkor_sdk::VynkorClient;

/// `notify_send`: parse + validate the request, resolve the provider kind,
/// then either store it silently or deliver it (optionally with tts
/// озвучка) and record an inbox audit entry.
///
/// - `silent: true` → store-only: an inbox entry with `delivered: false`,
///   `spoken: false`. `provider`/`speak` are ignored on this path.
/// - Otherwise deliver exactly as before, then speak best-effort (a failed
///   озвучка logs to stderr and returns `spoken: false` + `speak_error`,
///   never failing the delivered notification), and record an audit entry
///   when the inbox is available — a missing `NOTIFY_PLUGIN_DATA_DIR` must
///   not fail a normal notification.
pub async fn handle_notify_send(
    client: &mut VynkorClient,
    params_json: &[u8],
) -> Result<Vec<u8>, String> {
    let params = NotifyParams::parse(params_json)?;
    let kind = ProviderKind::parse(&params.provider)?;

    if params.silent {
        let mut inbox = Inbox::open()?;
        let id = inbox.push(inbox::InboxEntry {
            id: String::new(),
            created_at_ms: 0,
            title: params.title.clone(),
            message: params.message.clone(),
            provider: kind.as_str().to_string(),
            delivered: false,
            silent: true,
            spoken: false,
            read: false,
        })?;
        return serde_json::to_vec(&serde_json::json!({
            "id": id,
            "stored": true,
            "silent": true,
            "delivered": false,
        }))
        .map_err(|e| format!("failed to encode response: {e}"));
    }

    let delivered = providers::deliver(kind, &params).await?;

    let mut spoken = false;
    let mut speak_error = String::new();
    if params.speak {
        match providers::speak_via_tts(client, &providers::full_text(&params)).await {
            Ok(()) => spoken = true,
            Err(e) => {
                eprintln!("[notify] speak failed: {e}");
                speak_error = e;
            }
        }
    }

    let mut id = String::new();
    let entry = inbox::InboxEntry {
        id: String::new(),
        created_at_ms: 0,
        title: params.title.clone(),
        message: params.message.clone(),
        provider: kind.as_str().to_string(),
        delivered: true,
        silent: false,
        spoken,
        read: false,
    };
    match Inbox::open().and_then(|mut inbox| inbox.push(entry)) {
        Ok(entry_id) => id = entry_id,
        Err(e) => eprintln!("[notify] inbox unavailable ({e}), skipping store"),
    }

    serde_json::to_vec(&serde_json::json!({
        "id": id,
        "delivered": true,
        "provider": delivered.provider,
        "command": delivered.command,
        "detail": delivered.detail,
        "spoken": spoken,
        "speak_error": speak_error,
    }))
    .map_err(|e| format!("failed to encode response: {e}"))
}

/// `notify_providers`: serialize the three providers with their
/// availability.
pub fn handle_notify_providers() -> Result<Vec<u8>, String> {
    serde_json::to_vec(&providers::list_providers())
        .map_err(|e| format!("failed to encode response: {e}"))
}

/// `notify_list`: newest-first listing of stored notifications.
pub fn handle_notify_list(params_json: &[u8]) -> Result<Vec<u8>, String> {
    let params = ListParams::parse(params_json)?;
    let inbox = Inbox::open()?;
    list_response_json(&inbox, params.include_read)
}

/// Serialize an inbox listing — separated from [`handle_notify_list`] so the
/// shape is testable without a live kernel or data dir.
fn list_response_json(inbox: &Inbox, include_read: bool) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&serde_json::json!({ "notifications": inbox.list(include_read) }))
        .map_err(|e| format!("failed to encode response: {e}"))
}

/// `notify_mark_read`: mark one stored notification read.
pub fn handle_notify_mark_read(params_json: &[u8]) -> Result<Vec<u8>, String> {
    let params = IdParams::parse(params_json)?;
    let mut inbox = Inbox::open()?;
    let updated = inbox.mark_read(&params.id)?;
    serde_json::to_vec(&serde_json::json!({ "updated": updated }))
        .map_err(|e| format!("failed to encode response: {e}"))
}

/// `notify_delete`: delete one stored notification.
pub fn handle_notify_delete(params_json: &[u8]) -> Result<Vec<u8>, String> {
    let params = IdParams::parse(params_json)?;
    let mut inbox = Inbox::open()?;
    let deleted = inbox.delete(&params.id)?;
    serde_json::to_vec(&serde_json::json!({ "deleted": deleted }))
        .map_err(|e| format!("failed to encode response: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(message: &str) -> inbox::InboxEntry {
        inbox::InboxEntry {
            id: String::new(),
            created_at_ms: 0,
            title: String::new(),
            message: message.to_string(),
            provider: "notify-send".to_string(),
            delivered: true,
            silent: false,
            spoken: false,
            read: false,
        }
    }

    #[test]
    fn notify_providers_returns_json_array_of_three() {
        let data = handle_notify_providers().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&data).unwrap();
        let arr = v.as_array().expect("expected a JSON array");
        assert_eq!(arr.len(), 3);
        for entry in arr {
            for key in ["id", "name", "available", "description"] {
                assert!(
                    entry.get(key).is_some(),
                    "provider entry missing '{key}': {entry}"
                );
            }
        }
        let ids: Vec<&str> = arr.iter().map(|e| e["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["notify-send", "wall", "espeak"]);
    }

    #[test]
    fn notify_list_response_shape_filters_read() {
        let dir = tempfile::tempdir().unwrap();
        let mut inbox = Inbox::open_at(dir.path().join("inbox.json")).unwrap();
        let id1 = inbox.push(entry("m1")).unwrap();
        let id2 = inbox.push(entry("m2")).unwrap();
        inbox.mark_read(&id2).unwrap();

        let data = list_response_json(&inbox, false).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&data).unwrap();
        let arr = v["notifications"].as_array().expect("notifications array");
        assert_eq!(arr.len(), 1, "read entry filtered out");
        assert_eq!(arr[0]["id"], serde_json::json!(id1));
        for key in [
            "id",
            "created_at_ms",
            "title",
            "message",
            "provider",
            "delivered",
            "silent",
            "spoken",
            "read",
        ] {
            assert!(
                arr[0].get(key).is_some(),
                "notification missing '{key}': {}",
                arr[0]
            );
        }

        let data = list_response_json(&inbox, true).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(v["notifications"].as_array().unwrap().len(), 2);
    }
}
