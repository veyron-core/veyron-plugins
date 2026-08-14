//! Request parameter parsing and validation for `secrets` actions.
//!
//! Callers pass JSON params like `{"name": "api_key", "value": "..."}`.
//! Secret names are restricted to `[a-zA-Z0-9_.-]` (never empty, length
//! capped); values are length-capped. Caps reject rather than truncate.

use serde::Deserialize;

pub const DEFAULT_MAX_NAME_BYTES: usize = 256;
pub const DEFAULT_MAX_VALUE_BYTES: usize = 256 * 1024;

#[derive(Debug, Deserialize)]
pub struct SetParams {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct GetParams {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteParams {
    pub name: String,
}

pub fn valid_name(name: &str, max_name_bytes: usize) -> Result<(), String> {
    if name.is_empty() {
        return Err("missing secret name".to_string());
    }
    if name.len() > max_name_bytes {
        return Err(format!(
            "secret name too long: {} bytes (max {max_name_bytes})",
            name.len()
        ));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
    {
        return Err(format!("invalid secret name: {name:?}"));
    }
    Ok(())
}

pub fn parse_set_params(json: &[u8], max_value_bytes: usize) -> Result<SetParams, String> {
    let params: SetParams =
        serde_json::from_slice(json).map_err(|e| format!("invalid params: {e}"))?;
    if params.value.len() > max_value_bytes {
        return Err(format!(
            "secret value too large: {} bytes (max {max_value_bytes})",
            params.value.len()
        ));
    }
    Ok(params)
}

pub fn parse_get_params(json: &[u8]) -> Result<GetParams, String> {
    serde_json::from_slice(json).map_err(|e| format!("invalid params: {e}"))
}

pub fn parse_delete_params(json: &[u8]) -> Result<DeleteParams, String> {
    serde_json::from_slice(json).map_err(|e| format!("invalid params: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_set_params() {
        let p = parse_set_params(br#"{"name":"api_key","value":"sk-x"}"#, 1024).unwrap();
        assert_eq!(p.name, "api_key");
        assert_eq!(p.value, "sk-x");
    }

    #[test]
    fn rejects_missing_fields() {
        assert!(parse_set_params(br#"{"name":"api_key"}"#, 1024).is_err());
        assert!(parse_get_params(b"{}").is_err());
        assert!(parse_delete_params(br#"{"name":123}"#).is_err());
    }

    #[test]
    fn rejects_oversized_value() {
        let big = "x".repeat(100);
        let json = serde_json::json!({ "name": "k", "value": big }).to_string();
        assert!(parse_set_params(json.as_bytes(), 10).is_err());
    }

    #[test]
    fn validates_names() {
        assert!(valid_name("api_key", 256).is_ok());
        assert!(valid_name("a.b-c_d", 256).is_ok());
        assert!(valid_name("", 256).is_err());
        assert!(valid_name("has space", 256).is_err());
        assert!(valid_name("sl/ash", 256).is_err());
        assert!(valid_name("qu\"ote", 256).is_err());
        let long = "a".repeat(257);
        assert!(valid_name(&long, 256).is_err());
    }
}
