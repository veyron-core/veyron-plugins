//! Action handler for the `secrets` plugin.
//!
//! One encrypted vault file per kernel-stamped `caller_plugin_id`
//! (`{data_dir}/{caller_id}.vault`), isolated exactly like `database`'s
//! per-caller SQLite files. Vaults are cached decrypted in memory behind a
//! per-caller lock; every mutation re-encrypts and atomically persists the
//! whole file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::request::{self, valid_name};
use crate::vault::Vault;

/// Reject callers whose id is missing or contains characters that would make
/// an unsafe filename — identical policy to `database::db::sanitize_caller_id`.
pub fn sanitize_caller_id(caller_plugin_id: &str) -> Result<&str, String> {
    if caller_plugin_id.is_empty() {
        return Err("missing caller_plugin_id".to_string());
    }
    if !caller_plugin_id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(format!("invalid caller_plugin_id: {caller_plugin_id:?}"));
    }
    Ok(caller_plugin_id)
}

pub struct Handler {
    data_dir: PathBuf,
    key: [u8; 32],
    max_name_bytes: usize,
    max_value_bytes: usize,
    /// caller_id -> cached decrypted vault (lazy-loaded on first access).
    vaults: Mutex<HashMap<String, Arc<Mutex<Vault>>>>,
}

impl Handler {
    pub fn new(
        data_dir: PathBuf,
        key: [u8; 32],
        max_name_bytes: usize,
        max_value_bytes: usize,
    ) -> Self {
        Self {
            data_dir,
            key,
            max_name_bytes,
            max_value_bytes,
            vaults: Mutex::new(HashMap::new()),
        }
    }

    async fn get_vault(&self, caller_id: &str) -> Result<Arc<Mutex<Vault>>, String> {
        let caller_id = sanitize_caller_id(caller_id)?;

        // Fast path: already cached.
        {
            let guard = self
                .vaults
                .lock()
                .map_err(|_| "vault registry lock poisoned".to_string())?;
            if let Some(v) = guard.get(caller_id) {
                return Ok(v.clone());
            }
        }

        // Slow path: load (or create in memory) and cache.
        let path: PathBuf = self.data_dir.join(format!("{caller_id}.vault"));
        let vault = Vault::load_or_create(path, &self.key)?;
        let arc = Arc::new(Mutex::new(vault));
        self.vaults
            .lock()
            .map_err(|_| "vault registry lock poisoned".to_string())?
            .insert(caller_id.to_string(), arc.clone());
        Ok(arc)
    }

    pub async fn handle(
        &self,
        caller_plugin_id: &str,
        action: &str,
        params_json: &[u8],
    ) -> Result<Vec<u8>, String> {
        match action {
            "secret_set" => self.secret_set(caller_plugin_id, params_json).await,
            "secret_get" => self.secret_get(caller_plugin_id, params_json).await,
            "secret_delete" => self.secret_delete(caller_plugin_id, params_json).await,
            "secret_list" => self.secret_list(caller_plugin_id).await,
            _ => Err(format!("unknown action: {action}")),
        }
    }

    async fn secret_set(
        &self,
        caller_plugin_id: &str,
        params_json: &[u8],
    ) -> Result<Vec<u8>, String> {
        let params = request::parse_set_params(params_json, self.max_value_bytes)?;
        valid_name(&params.name, self.max_name_bytes)?;

        let vault = self.get_vault(caller_plugin_id).await?;
        let mut guard = vault
            .lock()
            .map_err(|_| "vault lock poisoned".to_string())?;
        guard.insert(&params.name, params.value);
        guard.persist(&self.key)?;
        Ok(br#"{"stored":true}"#.to_vec())
    }

    async fn secret_get(
        &self,
        caller_plugin_id: &str,
        params_json: &[u8],
    ) -> Result<Vec<u8>, String> {
        let params = request::parse_get_params(params_json)?;
        valid_name(&params.name, self.max_name_bytes)?;

        let vault = self.get_vault(caller_plugin_id).await?;
        let guard = vault
            .lock()
            .map_err(|_| "vault lock poisoned".to_string())?;
        match guard.get(&params.name) {
            Some(value) => Ok(format!(
                r#"{{"found":true,"value":{}}}"#,
                serde_json::to_string(value)
                    .map_err(|e| format!("failed to serialize value: {e}"))?
            )
            .into_bytes()),
            None => Ok(br#"{"found":false}"#.to_vec()),
        }
    }

    async fn secret_delete(
        &self,
        caller_plugin_id: &str,
        params_json: &[u8],
    ) -> Result<Vec<u8>, String> {
        let params = request::parse_delete_params(params_json)?;
        valid_name(&params.name, self.max_name_bytes)?;

        let vault = self.get_vault(caller_plugin_id).await?;
        let mut guard = vault
            .lock()
            .map_err(|_| "vault lock poisoned".to_string())?;
        let removed = guard.remove(&params.name);
        if removed {
            guard.persist(&self.key)?;
        }
        Ok(if removed {
            br#"{"deleted":true}"#.to_vec()
        } else {
            br#"{"deleted":false}"#.to_vec()
        })
    }

    async fn secret_list(&self, caller_plugin_id: &str) -> Result<Vec<u8>, String> {
        let vault = self.get_vault(caller_plugin_id).await?;
        let guard = vault
            .lock()
            .map_err(|_| "vault lock poisoned".to_string())?;
        let names = guard.names();
        let body = serde_json::json!({ "names": names }).to_string();
        Ok(body.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_key() -> [u8; 32] {
        [7u8; 32]
    }

    fn test_handler() -> (Handler, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let h = Handler::new(
            dir.path().to_path_buf(),
            test_key(),
            request::DEFAULT_MAX_NAME_BYTES,
            request::DEFAULT_MAX_VALUE_BYTES,
        );
        (h, dir)
    }

    #[tokio::test]
    async fn set_get_delete_roundtrip() {
        let (h, _d) = test_handler();
        let out = h
            .handle("caller-a", "secret_set", br#"{"name":"api_key","value":"sk-x"}"#)
            .await
            .unwrap();
        assert_eq!(out, br#"{"stored":true}"#);

        let out = h
            .handle("caller-a", "secret_get", br#"{"name":"api_key"}"#)
            .await
            .unwrap();
        assert_eq!(
            std::str::from_utf8(&out).unwrap(),
            r#"{"found":true,"value":"sk-x"}"#
        );

        let out = h
            .handle("caller-a", "secret_get", br#"{"name":"missing"}"#)
            .await
            .unwrap();
        assert_eq!(out, br#"{"found":false}"#);

        let out = h
            .handle("caller-a", "secret_delete", br#"{"name":"api_key"}"#)
            .await
            .unwrap();
        assert_eq!(out, br#"{"deleted":true}"#);

        let out = h
            .handle("caller-a", "secret_delete", br#"{"name":"api_key"}"#)
            .await
            .unwrap();
        assert_eq!(out, br#"{"deleted":false}"#);
    }

    #[tokio::test]
    async fn per_caller_isolation() {
        let (h, _d) = test_handler();
        h.handle("caller-a", "secret_set", br#"{"name":"k","value":"va"}"#)
            .await
            .unwrap();
        let out = h
            .handle("caller-b", "secret_get", br#"{"name":"k"}"#)
            .await
            .unwrap();
        assert_eq!(out, br#"{"found":false}"#);

        // Read misses never materialize a file — only a mutation writes it.
        let files: Vec<_> = std::fs::read_dir(_d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(files.contains(&"caller-a.vault".to_string()));
        assert!(!files.contains(&"caller-b.vault".to_string()));

        h.handle("caller-b", "secret_set", br#"{"name":"k","value":"vb"}"#)
            .await
            .unwrap();
        let out = h
            .handle("caller-a", "secret_get", br#"{"name":"k"}"#)
            .await
            .unwrap();
        assert_eq!(
            std::str::from_utf8(&out).unwrap(),
            r#"{"found":true,"value":"va"}"#
        );
        let files: Vec<_> = std::fs::read_dir(_d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(files.contains(&"caller-b.vault".to_string()));
    }

    #[tokio::test]
    async fn list_returns_sorted_names() {
        let (h, _d) = test_handler();
        h.handle("caller-a", "secret_set", br#"{"name":"zeta","value":"1"}"#)
            .await
            .unwrap();
        h.handle("caller-a", "secret_set", br#"{"name":"alpha","value":"2"}"#)
            .await
            .unwrap();
        let out = h.handle("caller-a", "secret_list", b"").await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["names"], serde_json::json!(["alpha", "zeta"]));
    }

    #[tokio::test]
    async fn rejects_bad_caller_id() {
        let (h, _d) = test_handler();
        let out = h
            .handle("", "secret_list", b"")
            .await
            .unwrap_err();
        assert!(out.contains("missing caller_plugin_id"));
        let out = h
            .handle("../evil", "secret_list", b"")
            .await
            .unwrap_err();
        assert!(out.contains("invalid caller_plugin_id"));
    }

    #[tokio::test]
    async fn rejects_bad_name_and_oversize_value() {
        let (h, _d) = test_handler();
        let out = h
            .handle("c", "secret_set", br#"{"name":"bad name","value":"v"}"#)
            .await
            .unwrap_err();
        assert!(out.contains("invalid secret name"));

        let big = "x".repeat(1024);
        let json = serde_json::json!({ "name": "k", "value": big }).to_string();
        let small_handler = Handler::new(
            _d.path().to_path_buf(),
            test_key(),
            256,
            10, // tiny cap
        );
        let out = small_handler
            .handle("c", "secret_set", json.as_bytes())
            .await
            .unwrap_err();
        assert!(out.contains("too large"));
    }

    #[tokio::test]
    async fn unknown_action_rejected() {
        let (h, _d) = test_handler();
        let out = h.handle("c", "nope", b"{}").await.unwrap_err();
        assert!(out.contains("unknown action"));
    }

    #[tokio::test]
    async fn tampered_vault_fails_after_cache_eviction() {
        let (h, d) = test_handler();
        h.handle("c", "secret_set", br#"{"name":"k","value":"v"}"#)
            .await
            .unwrap();

        // Corrupt the file behind the plugin's back.
        let path = d.path().join("c.vault");
        let mut raw = std::fs::read(&path).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0x01;
        std::fs::write(&path, raw).unwrap();

        // A fresh handler (new process) must refuse to load it.
        let h2 = Handler::new(
            d.path().to_path_buf(),
            test_key(),
            request::DEFAULT_MAX_NAME_BYTES,
            request::DEFAULT_MAX_VALUE_BYTES,
        );
        let out = h2.handle("c", "secret_list", b"").await.unwrap_err();
        assert!(out.contains("decryption failed"));
    }
}
