use buglogin_domain::errors::Problem;
use serde::{Serialize, de::DeserializeOwned};

pub(crate) fn encode<T: Serialize>(value: &T) -> Result<String, Problem> {
    serde_json::to_string(value).map_err(|_| protocol_problem("coordination_encoding_failed"))
}

pub(crate) fn decode<T: DeserializeOwned>(encoded: &str) -> Result<T, Problem> {
    serde_json::from_str(encoded).map_err(|_| protocol_problem("coordination_decoding_failed"))
}

pub(crate) fn validate_identifier(field: &str, value: &str) -> Result<(), Problem> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(invalid("invalid_coordination_identifier").with_field("field", field));
    }
    Ok(())
}

pub(crate) fn validate_namespace(value: &str) -> Result<(), Problem> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(invalid("invalid_coordination_namespace"));
    }
    Ok(())
}

pub(crate) fn invalid(code: &'static str) -> Problem {
    Problem::new(code, "Invalid coordination input", 400, "sync-coordination")
}

fn protocol_problem(code: &'static str) -> Problem {
    Problem::new(
        code,
        "Coordination protocol failure",
        500,
        "sync-coordination",
    )
}
