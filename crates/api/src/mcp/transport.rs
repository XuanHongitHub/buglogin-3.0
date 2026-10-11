use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex, RwLock},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use buglogin_domain::errors::Problem;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    McpDispatcher, McpRequest, ProductionMcpExecutor, ProductionMcpPolicy, ToolMetadata, tools,
};

pub type ProductionMcpDispatcher = McpDispatcher<ProductionMcpPolicy, ProductionMcpExecutor>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct McpTransportConfig {
    pub enabled: bool,
    pub max_body_bytes: usize,
}

impl Default for McpTransportConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_body_bytes: 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssuedTransportBearer(String);

impl IssuedTransportBearer {
    pub fn expose_once(self) -> String {
        self.0
    }
}

#[derive(Clone, Default)]
pub struct McpTransportControl {
    hashes: Arc<RwLock<BTreeSet<[u8; 32]>>>,
}

impl McpTransportControl {
    pub fn activate(&self, bearer: &str) -> Result<(), Problem> {
        if bearer.len() < 32
            || !bearer.is_ascii()
            || bearer.bytes().any(|byte| byte.is_ascii_whitespace())
        {
            return Err(problem(
                "mcp_invalid_token",
                "MCP transport bearer is invalid",
                400,
            ));
        }
        self.hashes
            .write()
            .map_err(|_| problem("mcp_token_unavailable", "MCP token store unavailable", 503))?
            .insert(hash(bearer.as_bytes()));
        Ok(())
    }

    pub fn issue(&self) -> Result<IssuedTransportBearer, Problem> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|_| problem("mcp_token_unavailable", "MCP token generation failed", 503))?;
        let raw = hex(&bytes);
        self.hashes
            .write()
            .map_err(|_| problem("mcp_token_unavailable", "MCP token store unavailable", 503))?
            .insert(hash(raw.as_bytes()));
        Ok(IssuedTransportBearer(raw))
    }

    pub fn rotate(&self, current: &str) -> Result<IssuedTransportBearer, Problem> {
        self.revoke(current)?;
        self.issue()
    }

    pub fn revoke(&self, bearer: &str) -> Result<(), Problem> {
        let removed = self
            .hashes
            .write()
            .map_err(|_| problem("mcp_token_unavailable", "MCP token store unavailable", 503))?
            .remove(&hash(bearer.as_bytes()));
        if removed {
            Ok(())
        } else {
            Err(problem(
                "mcp_transport_unauthorized",
                "MCP transport bearer is invalid",
                401,
            ))
        }
    }

    pub fn authenticate(&self, bearer: &str) -> Result<(), Problem> {
        if self
            .hashes
            .read()
            .map_err(|_| problem("mcp_token_unavailable", "MCP token store unavailable", 503))?
            .contains(&hash(bearer.as_bytes()))
        {
            Ok(())
        } else {
            Err(problem(
                "mcp_transport_unauthorized",
                "MCP transport bearer is invalid",
                401,
            ))
        }
    }
}

#[derive(Clone)]
struct ProductionTransportState {
    config: McpTransportConfig,
    control: McpTransportControl,
    dispatcher: Arc<Mutex<ProductionMcpDispatcher>>,
}

pub fn production_router(
    state: crate::http::ApiState,
    config: McpTransportConfig,
    control: McpTransportControl,
) -> Router {
    let dispatcher = McpDispatcher::new(
        ProductionMcpPolicy::new(state.clone()),
        ProductionMcpExecutor::new(state),
        config.enabled,
    );
    Router::new()
        .route("/mcp", post(handle))
        .with_state(ProductionTransportState {
            config,
            control,
            dispatcher: Arc::new(Mutex::new(dispatcher)),
        })
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    data: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
    bearer_token: Option<String>,
    idempotency_key: Option<String>,
    correlation_id: String,
}

async fn handle(
    State(state): State<ProductionTransportState>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, Json<JsonRpcResponse>) {
    let fallback_id = Value::Null;
    if !state.config.enabled {
        return response_error(
            fallback_id,
            problem("mcp_disabled", "MCP transport is disabled", 503),
        );
    }
    if body.len() > state.config.max_body_bytes {
        return response_error(
            fallback_id,
            problem("mcp_request_too_large", "MCP request is too large", 413),
        );
    }
    if let Err(error) = validate_origin(&headers) {
        return response_error(fallback_id, error);
    }
    let bearer = match transport_bearer(&headers) {
        Ok(value) => value,
        Err(error) => return response_error(fallback_id, error),
    };
    if let Err(error) = state.control.authenticate(bearer) {
        return response_error(fallback_id, error);
    }
    let request: JsonRpcRequest = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return response_error(
                fallback_id,
                problem("mcp_parse_error", "Invalid JSON-RPC request", 400),
            );
        }
    };
    let id = request.id.clone();
    if request.jsonrpc != "2.0" {
        return response_error(
            id,
            problem("mcp_invalid_request", "JSON-RPC version must be 2.0", 400),
        );
    }
    let result = match request.method.as_str() {
        "initialize" => Ok(json!({
            "protocolVersion": "2025-06-18",
            "serverInfo": { "name": "buglogin", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": { "tools": { "listChanged": false } },
        })),
        "tools/list" => Ok(json!({ "tools": tool_list(tools()) })),
        "tools/call" => call_tool(&state, request.params),
        _ => Err(problem(
            "mcp_method_not_found",
            "JSON-RPC method not found",
            404,
        )),
    };
    match result {
        Ok(value) => (
            StatusCode::OK,
            Json(JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(value),
                error: None,
            }),
        ),
        Err(error) => response_error(id, error),
    }
}

fn call_tool(state: &ProductionTransportState, params: Value) -> Result<Value, Problem> {
    let params: ToolCallParams = serde_json::from_value(params)
        .map_err(|_| problem("mcp_invalid_params", "MCP tool parameters are invalid", 400))?;
    let mut dispatcher = state
        .dispatcher
        .lock()
        .map_err(|_| problem("mcp_unavailable", "MCP dispatcher unavailable", 503))?;
    dispatcher.dispatch(McpRequest {
        tool_name: params.name,
        correlation_id: params.correlation_id,
        bearer_token: params.bearer_token,
        idempotency_key: params.idempotency_key,
        input: params.arguments,
    })
}

fn tool_list(items: Vec<ToolMetadata>) -> Vec<Value> {
    items
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.command_id,
                "inputSchema": { "type": "object", "additionalProperties": true },
                "annotations": {
                    "readOnlyHint": matches!(tool.action, buglogin_domain::permissions::Action::Read),
                    "destructiveHint": matches!(tool.action, buglogin_domain::permissions::Action::Delete),
                },
            })
        })
        .collect()
}

fn transport_bearer(headers: &HeaderMap) -> Result<&str, Problem> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            problem(
                "mcp_transport_unauthorized",
                "MCP transport bearer is required",
                401,
            )
        })
}

fn validate_origin(headers: &HeaderMap) -> Result<(), Problem> {
    let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok()) else {
        return Ok(());
    };
    if origin.starts_with("http://127.0.0.1")
        || origin.starts_with("http://localhost")
        || origin.starts_with("http://[::1]")
    {
        Ok(())
    } else {
        Err(problem(
            "mcp_origin_forbidden",
            "MCP origin must be loopback",
            403,
        ))
    }
}

fn response_error(id: Value, error: Problem) -> (StatusCode, Json<JsonRpcResponse>) {
    let status = StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        Json(JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code: json_rpc_code(error.status),
                message: error.title.clone(),
                data: serde_json::to_value(error).unwrap_or(Value::Null),
            }),
        }),
    )
}

fn json_rpc_code(status: u16) -> i64 {
    match status {
        400 => -32602,
        404 => -32601,
        401 | 403 => -32001,
        413 => -32002,
        _ => -32603,
    }
}

fn hash(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "mcp-transport")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_rotation_revokes_previous_value() {
        let control = McpTransportControl::default();
        let first = control.issue().unwrap().expose_once();
        assert!(control.authenticate(&first).is_ok());
        let second = control.rotate(&first).unwrap().expose_once();
        assert_eq!(control.authenticate(&first).unwrap_err().status, 401);
        assert!(control.authenticate(&second).is_ok());
    }

    #[test]
    fn transport_is_disabled_by_default() {
        assert!(!McpTransportConfig::default().enabled);
    }
}
