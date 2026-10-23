use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use buglogin_domain::{errors::Problem, ids::WorkspaceId};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;

use crate::remote_api::RemoteApiClient;

const EVENT_NAME: &str = "buglogin://api-event";
const ERROR_EVENT_NAME: &str = "buglogin://api-event-error";
const MAX_SSE_BUFFER_BYTES: usize = 1_048_576;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventStreamInput {
    workspace_id: String,
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

pub fn parse_input(
    payload: Value,
    correlation_id: &str,
) -> Result<(WorkspaceId, Option<String>, usize), Problem> {
    let input: EventStreamInput = serde_json::from_value(payload).map_err(|_| {
        Problem::new(
            "event_stream_input_invalid",
            "Event stream input is invalid",
            400,
            correlation_id,
        )
    })?;
    let workspace_id = WorkspaceId::new(input.workspace_id).map_err(|_| {
        Problem::new(
            "event_stream_workspace_invalid",
            "Workspace ID is invalid",
            400,
            correlation_id,
        )
    })?;
    let limit = input.limit.unwrap_or(100);
    if !(1..=1_000).contains(&limit) {
        return Err(Problem::new(
            "invalid_event_read_limit",
            "Event limit is invalid",
            400,
            correlation_id,
        ));
    }
    if input
        .after
        .as_ref()
        .is_some_and(|cursor| !valid_cursor(cursor))
    {
        return Err(Problem::new(
            "invalid_event_cursor",
            "Event cursor is invalid",
            400,
            correlation_id,
        ));
    }
    Ok((workspace_id, input.after, limit))
}

#[derive(Default)]
pub struct EventStreamManager {
    subscriptions: Arc<Mutex<BTreeMap<String, watch::Sender<bool>>>>,
    sequence: AtomicU64,
}

impl EventStreamManager {
    pub fn start(
        &self,
        app: AppHandle,
        client: RemoteApiClient,
        workspace_id: WorkspaceId,
        after: Option<String>,
        limit: usize,
        correlation_id: String,
    ) -> Result<String, Problem> {
        if !(1..=1_000).contains(&limit) {
            return Err(Problem::new(
                "invalid_event_read_limit",
                "Event limit is invalid",
                400,
                &correlation_id,
            ));
        }
        if after.as_ref().is_some_and(|cursor| !valid_cursor(cursor)) {
            return Err(Problem::new(
                "invalid_event_cursor",
                "Event cursor is invalid",
                400,
                &correlation_id,
            ));
        }

        let key = format!("event-{}", self.sequence.fetch_add(1, Ordering::Relaxed));
        let (cancel, receiver) = watch::channel(false);
        self.subscriptions
            .lock()
            .map_err(|_| unavailable(&correlation_id))?
            .insert(key.clone(), cancel);

        let cleanup = SubscriptionCleanup {
            subscriptions: Arc::clone(&self.subscriptions),
            subscription_id: key.clone(),
        };
        tauri::async_runtime::spawn(async move {
            let _cleanup = cleanup;
            run_stream(
                app,
                client,
                workspace_id,
                after,
                limit,
                correlation_id,
                receiver,
            )
            .await;
        });
        Ok(key)
    }

    pub fn stop(&self, subscription_id: &str) -> Result<(), Problem> {
        let sender = self
            .subscriptions
            .lock()
            .map_err(|_| unavailable("event-stream-stop"))?
            .remove(subscription_id);
        if let Some(sender) = sender {
            let _ = sender.send(true);
        }
        Ok(())
    }
}

struct SubscriptionCleanup {
    subscriptions: Arc<Mutex<BTreeMap<String, watch::Sender<bool>>>>,
    subscription_id: String,
}

impl Drop for SubscriptionCleanup {
    fn drop(&mut self) {
        if let Ok(mut subscriptions) = self.subscriptions.lock() {
            subscriptions.remove(&self.subscription_id);
        }
    }
}

async fn run_stream(
    app: AppHandle,
    client: RemoteApiClient,
    workspace_id: WorkspaceId,
    mut cursor: Option<String>,
    limit: usize,
    correlation_id: String,
    mut cancel: watch::Receiver<bool>,
) {
    let mut retry_delay = Duration::from_secs(1);
    loop {
        if *cancel.borrow() {
            return;
        }
        let response = tokio::select! {
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() { return; }
                continue;
            }
            response = client.open_event_stream(&workspace_id, cursor.as_deref(), limit, &correlation_id) => response,
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                emit_error(&app, &workspace_id, &error);
                if !error.retryable {
                    return;
                }
                if pause_or_cancel(&mut cancel, retry_delay).await {
                    return;
                }
                retry_delay = (retry_delay * 2).min(Duration::from_secs(15));
                continue;
            }
        };

        retry_delay = Duration::from_secs(1);
        let mut body = response.bytes_stream();
        let mut buffer = Vec::<u8>::new();
        loop {
            let next = tokio::select! {
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow() { return; }
                    continue;
                }
                next = body.next() => next,
            };
            let Some(chunk) = next else { break };
            match chunk {
                Ok(chunk) => {
                    buffer.extend_from_slice(&chunk);
                    while let Some(frame) = take_frame(&mut buffer) {
                        if let Some((id, name, data)) = parse_frame(&frame) {
                            if let Some(id) = id {
                                cursor = Some(id.clone());
                            }
                            if let Ok(event_data) = serde_json::from_str::<Value>(&data) {
                                if name == "error" {
                                    let _ = app.emit(
                                        ERROR_EVENT_NAME,
                                        json!({
                                            "workspaceId": workspace_id.as_str(),
                                            "code": event_data.get("code"),
                                            "message": event_data.get("detail").or_else(|| event_data.get("title")),
                                            "status": event_data.get("status"),
                                            "retryable": event_data.get("retryable"),
                                        }),
                                    );
                                    continue;
                                }
                                let _ = app.emit(
                                    EVENT_NAME,
                                    json!({
                                        "workspaceId": workspace_id.as_str(),
                                        "streamId": cursor.clone(),
                                        "name": name,
                                        "event": event_data,
                                    }),
                                );
                            }
                        }
                    }
                    if buffer.len() > MAX_SSE_BUFFER_BYTES {
                        emit_error(
                            &app,
                            &workspace_id,
                            &Problem::new(
                                "event_frame_too_large",
                                "BugLogin event frame exceeds the client limit",
                                502,
                                &correlation_id,
                            ),
                        );
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        if pause_or_cancel(&mut cancel, retry_delay).await {
            return;
        }
    }
}

async fn pause_or_cancel(cancel: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    tokio::select! {
        changed = cancel.changed() => changed.is_err() || *cancel.borrow(),
        _ = tokio::time::sleep(delay) => *cancel.borrow(),
    }
}

fn take_frame(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let lf_end = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf_end = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    let (end, width) = match (lf_end, crlf_end) {
        (Some(lf), Some(crlf)) if crlf < lf => (crlf, 4),
        (Some(lf), _) => (lf, 2),
        (None, Some(crlf)) => (crlf, 4),
        (None, None) => return None,
    };
    let frame = buffer.drain(..end + width).collect();
    Some(frame)
}

fn parse_frame(frame: &[u8]) -> Option<(Option<String>, String, String)> {
    let frame = String::from_utf8_lossy(frame);
    let mut id = None;
    let mut name = "message".to_owned();
    let mut data = Vec::new();
    for line in frame.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(value) = line.strip_prefix("id:") {
            id = Some(value.trim_start().to_owned());
        } else if let Some(value) = line.strip_prefix("event:") {
            name = value.trim_start().to_owned();
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    if data.is_empty() {
        return None;
    }
    Some((id, name, data.join("\n")))
}

fn valid_cursor(cursor: &str) -> bool {
    let Some((milliseconds, sequence)) = cursor.split_once('-') else {
        return false;
    };
    !milliseconds.is_empty()
        && !sequence.is_empty()
        && milliseconds.bytes().all(|byte| byte.is_ascii_digit())
        && sequence.bytes().all(|byte| byte.is_ascii_digit())
}

fn emit_error(app: &AppHandle, workspace_id: &WorkspaceId, error: &Problem) {
    let _ = app.emit(
        ERROR_EVENT_NAME,
        json!({
            "workspaceId": workspace_id.as_str(),
            "code": error.code,
            "message": error.detail.as_deref().unwrap_or(&error.title),
            "status": error.status,
            "retryable": error.retryable,
        }),
    );
}

fn unavailable(correlation_id: &str) -> Problem {
    Problem::new(
        "event_stream_unavailable",
        "Desktop event stream manager is unavailable",
        503,
        correlation_id,
    )
}
