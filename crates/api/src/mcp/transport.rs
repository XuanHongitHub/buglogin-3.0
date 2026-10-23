use std::sync::Arc;

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

#[derive(Clone)]
struct ProductionTransportState {
    config: McpTransportConfig,
    api_state: crate::http::ApiState,
    dispatcher: Arc<tokio::sync::Mutex<ProductionMcpDispatcher>>,
}

pub fn production_router(state: crate::http::ApiState, config: McpTransportConfig) -> Router {
    let dispatcher = McpDispatcher::new(
        ProductionMcpPolicy::new(state.clone()),
        ProductionMcpExecutor::new(state.clone()),
        config.enabled,
    );
    Router::new()
        .route("/mcp", post(handle))
        .with_state(ProductionTransportState {
            config,
            api_state: state,
            dispatcher: Arc::new(tokio::sync::Mutex::new(dispatcher)),
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
    correlation_id: Option<String>,
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
    if !bearer.starts_with("blapi_") {
        return response_error(
            fallback_id,
            problem(
                "mcp_transport_unauthorized",
                "MCP requires a scoped BugLogin API token",
                401,
            ),
        );
    }
    let now = match crate::clock::unix_time_seconds("mcp-transport-auth") {
        Ok(now) => now,
        Err(error) => return response_error(fallback_id, error),
    };
    if let Err(error) = state.api_state.authenticate_token(bearer, now) {
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
        "tools/call" if !id.is_null() => {
            call_tool_with_snapshot(&state, request.params, bearer, &id).await
        }
        "tools/call" => Err(problem(
            "mcp_invalid_request",
            "MCP tool calls require a JSON-RPC request ID",
            400,
        )),
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

async fn call_tool_with_snapshot(
    state: &ProductionTransportState,
    params: Value,
    transport_bearer: &str,
    request_id: &Value,
) -> Result<Value, Problem> {
    let mut params: ToolCallParams = serde_json::from_value(params)
        .map_err(|_| problem("mcp_invalid_params", "MCP tool parameters are invalid", 400))?;
    if params
        .bearer_token
        .as_deref()
        .is_some_and(|provided| provided != transport_bearer)
    {
        return Err(problem(
            "mcp_transport_unauthorized",
            "MCP tool bearer must match the authenticated transport bearer",
            401,
        ));
    }
    params.bearer_token = Some(transport_bearer.to_owned());
    let call_hash = standard_call_hash(request_id, &params.name);
    if params.correlation_id.is_none() {
        params.correlation_id = Some(format!("cor_mcp_{call_hash}"));
    }
    if params.idempotency_key.is_none()
        && super::dispatch_metadata(&params.name)
            .is_ok_and(|metadata| metadata.command.idempotency_required)
    {
        // JSON-RPC request IDs are stable across a transport retry. Use one as
        // the fallback idempotency identity so standard MCP clients can call
        // mutation tools without BugLogin-specific parameters.
        params.idempotency_key = Some(format!("mcp_{call_hash}"));
    }
    // ProductionMcpPolicy holds the read snapshot gate through execution and
    // releases it before persisting the resulting audit event. Mutation
    // commands hold the write gate through execution and idempotency commit.
    dispatch_tool(&state.dispatcher, params).await
}

async fn dispatch_tool(
    dispatcher: &Arc<tokio::sync::Mutex<ProductionMcpDispatcher>>,
    params: ToolCallParams,
) -> Result<Value, Problem> {
    let mut dispatcher = dispatcher.lock().await;
    dispatcher
        .dispatch_async(McpRequest {
            tool_name: params.name,
            correlation_id: params.correlation_id.expect("generated when absent"),
            bearer_token: params.bearer_token,
            idempotency_key: params.idempotency_key,
            input: params.arguments,
        })
        .await
}

fn standard_call_hash(request_id: &Value, tool_name: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"buglogin-mcp-call-v1\0");
    digest.update(tool_name.as_bytes());
    digest.update([0]);
    digest.update(request_id.to_string().as_bytes());
    let bytes = digest.finalize();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn tool_list(items: Vec<ToolMetadata>) -> Vec<Value> {
    items
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.command_id,
                "inputSchema": tool.input_schema,
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

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "mcp-transport")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_is_disabled_by_default() {
        assert!(!McpTransportConfig::default().enabled);
    }
}
