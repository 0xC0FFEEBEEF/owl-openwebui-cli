use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::telemetry::SseTelemetry;
use codex_client::StreamResponse;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use eventsource_stream::Eventsource;
use futures::Stream;
use futures::StreamExt;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;
use tracing::warn;

pub(crate) fn spawn_chat_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    _turn_state: Option<Arc<OnceLock<String>>>,
) -> ResponseStream {
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        process_chat_sse(stream_response.bytes, tx_event, idle_timeout, telemetry).await;
    });
    ResponseStream { rx_event }
}

/// Processes Server-Sent Events from the legacy Chat Completions streaming API.
///
/// The upstream protocol terminates a streaming response with a final sentinel event
/// (`data: [DONE]`). Historically, some of our test stubs have emitted `data: DONE`
/// (without brackets) instead.
///
/// `eventsource_stream` delivers these sentinels as regular events rather than signaling
/// end-of-stream. If we try to parse them as JSON, we log and skip them, then keep
/// polling for more events.
///
/// On servers that keep the HTTP connection open after emitting the sentinel (notably
/// wiremock on Windows), skipping the sentinel means we never emit `ResponseEvent::Completed`.
/// Higher-level workflows/tests that wait for completion before issuing subsequent model
/// calls will then stall, which shows up as "expected N requests, got 1" verification
/// failures in the mock server.
pub async fn process_chat_sse<S>(
    stream: S,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<std::sync::Arc<dyn SseTelemetry>>,
) where
    S: Stream<Item = Result<bytes::Bytes, codex_client::TransportError>> + Unpin,
{
    let mut stream = stream.eventsource();

    #[derive(Default, Debug)]
    struct ToolCallState {
        id: Option<String>,
        name: Option<String>,
        arguments: String,
    }

    let mut tool_calls: HashMap<usize, ToolCallState> = HashMap::new();
    let mut tool_call_order: Vec<usize> = Vec::new();
    let mut tool_call_order_seen: HashSet<usize> = HashSet::new();
    let mut tool_call_index_by_id: HashMap<String, usize> = HashMap::new();
    let mut next_tool_call_index = 0usize;
    let mut last_tool_call_index: Option<usize> = None;
    let mut assistant_item: Option<ResponseItem> = None;
    let mut reasoning_item: Option<ResponseItem> = None;
    let mut completed_sent = false;

    async fn flush_tool_calls(
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
        tool_calls: &mut HashMap<usize, ToolCallState>,
        tool_call_order: &mut Vec<usize>,
        tool_call_order_seen: &mut HashSet<usize>,
    ) {
        for index in tool_call_order.drain(..) {
            let Some(state) = tool_calls.remove(&index) else {
                continue;
            };
            tool_call_order_seen.remove(&index);
            let ToolCallState {
                id,
                name,
                arguments,
            } = state;
            let Some(name) = name else {
                debug!("Skipping tool call at index {index} because name is missing");
                continue;
            };
            let arguments = if arguments.trim().is_empty() {
                "{}".to_string()
            } else {
                arguments
            };
            if serde_json::from_str::<serde_json::Value>(&arguments).is_err() {
                // Likely cut off mid-arguments by an output-token limit in the server. Dispatch
                // anyway: core routes the handler's parse failure back to the model as a failed
                // tool call (router.rs::failure_response -> "failed to parse function
                // arguments: ..."), so the model can retry in smaller pieces instead of the
                // turn ending silently after the call was announced (the "announced then
                // nothing" report the user saw on long tool calls).
                warn!(
                    "Dispatching truncated tool call {name} at index {index}: arguments are not \
                     valid JSON (the response was probably cut off by the server); reporting the \
                     failure to the model instead of dropping the call"
                );
            }
            let item = ResponseItem::FunctionCall {
                id: None,
                name,
                arguments,
                call_id: id.unwrap_or_else(|| format!("tool-call-{index}")),
            };
            let _ = tx_event.send(Ok(ResponseEvent::OutputItemDone(item))).await;
        }
    }

    async fn flush_and_complete(
        tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
        reasoning_item: &mut Option<ResponseItem>,
        assistant_item: &mut Option<ResponseItem>,
        tool_calls: &mut HashMap<usize, ToolCallState>,
        tool_call_order: &mut Vec<usize>,
        tool_call_order_seen: &mut HashSet<usize>,
    ) {
        if let Some(reasoning) = reasoning_item.take() {
            let _ = tx_event
                .send(Ok(ResponseEvent::OutputItemDone(reasoning)))
                .await;
        }

        if let Some(assistant) = assistant_item.take() {
            let _ = tx_event
                .send(Ok(ResponseEvent::OutputItemDone(assistant)))
                .await;
        }

        // Owl-local fix: many OpenAI-compatible backends (Open WebUI's bridges included) end the
        // stream with `finish_reason: "stop"`, plain `[DONE]`, or a bare connection close even
        // when the model emitted tool calls in the deltas. Before this, buffered tool calls were
        // only dispatched on an explicit `finish_reason: "tool_calls"` chunk and were otherwise
        // silently dropped - the announced action simply never happened.
        flush_tool_calls(tx_event, tool_calls, tool_call_order, tool_call_order_seen).await;

        let _ = tx_event
            .send(Ok(ResponseEvent::Completed {
                response_id: String::new(),
                token_usage: None,
            }))
            .await;
    }

    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => {
                if !completed_sent {
                    flush_and_complete(
                        &tx_event,
                        &mut reasoning_item,
                        &mut assistant_item,
                        &mut tool_calls,
                        &mut tool_call_order,
                        &mut tool_call_order_seen,
                    )
                    .await;
                }
                return;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream("idle timeout waiting for SSE".into())))
                    .await;
                return;
            }
        };

        trace!("SSE event: {}", sse.data);

        let data = sse.data.trim();

        if data.is_empty() {
            continue;
        }

        if data == "[DONE]" || data == "DONE" {
            if !completed_sent {
                flush_and_complete(
                    &tx_event,
                    &mut reasoning_item,
                    &mut assistant_item,
                    &mut tool_calls,
                    &mut tool_call_order,
                    &mut tool_call_order_seen,
                )
                .await;
            }
            return;
        }

        let value: serde_json::Value = match serde_json::from_str(data) {
            Ok(val) => val,
            Err(err) => {
                debug!(
                    "Failed to parse ChatCompletions SSE event: {err}, data: {}",
                    data
                );
                continue;
            }
        };

        let Some(choices) = value.get("choices").and_then(|c| c.as_array()) else {
            continue;
        };

        for choice in choices {
            if let Some(delta) = choice.get("delta") {
                if let Some(reasoning) = delta.get("reasoning") {
                    if let Some(text) = reasoning.as_str() {
                        append_reasoning_text(&tx_event, &mut reasoning_item, text.to_string())
                            .await;
                    } else if let Some(text) = reasoning.get("text").and_then(|v| v.as_str()) {
                        append_reasoning_text(&tx_event, &mut reasoning_item, text.to_string())
                            .await;
                    } else if let Some(text) = reasoning.get("content").and_then(|v| v.as_str()) {
                        append_reasoning_text(&tx_event, &mut reasoning_item, text.to_string())
                            .await;
                    }
                }

                if let Some(content) = delta.get("content") {
                    if content.is_array() {
                        for item in content.as_array().unwrap_or(&vec![]) {
                            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                append_assistant_text(
                                    &tx_event,
                                    &mut assistant_item,
                                    text.to_string(),
                                )
                                .await;
                            }
                        }
                    } else if let Some(text) = content.as_str() {
                        append_assistant_text(&tx_event, &mut assistant_item, text.to_string())
                            .await;
                    }
                }

                if let Some(tool_call_values) = delta.get("tool_calls").and_then(|c| c.as_array()) {
                    for tool_call in tool_call_values {
                        let mut index = tool_call
                            .get("index")
                            .and_then(serde_json::Value::as_u64)
                            .map(|i| i as usize);

                        let mut call_id_for_lookup = None;
                        if let Some(call_id) = tool_call.get("id").and_then(|i| i.as_str()) {
                            call_id_for_lookup = Some(call_id.to_string());
                            if let Some(existing) = tool_call_index_by_id.get(call_id) {
                                index = Some(*existing);
                            }
                        }

                        if index.is_none() && call_id_for_lookup.is_none() {
                            index = last_tool_call_index;
                        }

                        let index = index.unwrap_or_else(|| {
                            while tool_calls.contains_key(&next_tool_call_index) {
                                next_tool_call_index += 1;
                            }
                            let idx = next_tool_call_index;
                            next_tool_call_index += 1;
                            idx
                        });

                        let call_state = tool_calls.entry(index).or_default();
                        if tool_call_order_seen.insert(index) {
                            tool_call_order.push(index);
                        }

                        if let Some(id) = tool_call.get("id").and_then(|i| i.as_str()) {
                            call_state.id.get_or_insert_with(|| id.to_string());
                            tool_call_index_by_id.entry(id.to_string()).or_insert(index);
                        }

                        if let Some(func) = tool_call.get("function") {
                            if let Some(fname) = func.get("name").and_then(|n| n.as_str())
                                && !fname.is_empty()
                            {
                                call_state.name.get_or_insert_with(|| fname.to_string());
                            }
                            if let Some(arguments) = func.get("arguments").and_then(|a| a.as_str())
                            {
                                call_state.arguments.push_str(arguments);
                            }
                        }

                        last_tool_call_index = Some(index);
                    }
                }
            }

            if let Some(message) = choice.get("message")
                && let Some(reasoning) = message.get("reasoning")
            {
                if let Some(text) = reasoning.as_str() {
                    append_reasoning_text(&tx_event, &mut reasoning_item, text.to_string()).await;
                } else if let Some(text) = reasoning.get("text").and_then(|v| v.as_str()) {
                    append_reasoning_text(&tx_event, &mut reasoning_item, text.to_string()).await;
                } else if let Some(text) = reasoning.get("content").and_then(|v| v.as_str()) {
                    append_reasoning_text(&tx_event, &mut reasoning_item, text.to_string()).await;
                }
            }

            let finish_reason = choice.get("finish_reason").and_then(|r| r.as_str());
            if finish_reason == Some("stop") {
                if !completed_sent {
                    completed_sent = true;
                    flush_and_complete(
                        &tx_event,
                        &mut reasoning_item,
                        &mut assistant_item,
                        &mut tool_calls,
                        &mut tool_call_order,
                        &mut tool_call_order_seen,
                    )
                    .await;
                }
                continue;
            }

            if finish_reason == Some("length") {
                // Output was truncated by a token limit. Dispatch every announced tool call
                // before the error - complete ones run as usual, cut-off ones are answered by
                // core with a parse-failure tool result so the model can retry in smaller
                // pieces rather than the turn stopping after an announcement.
                flush_tool_calls(
                    &tx_event,
                    &mut tool_calls,
                    &mut tool_call_order,
                    &mut tool_call_order_seen,
                )
                .await;
                let _ = tx_event.send(Err(ApiError::ContextWindowExceeded)).await;
                return;
            }

            if finish_reason == Some("tool_calls") {
                if let Some(reasoning) = reasoning_item.take() {
                    let _ = tx_event
                        .send(Ok(ResponseEvent::OutputItemDone(reasoning)))
                        .await;
                }

                for index in tool_call_order.drain(..) {
                    let Some(state) = tool_calls.remove(&index) else {
                        continue;
                    };
                    tool_call_order_seen.remove(&index);
                    let ToolCallState {
                        id,
                        name,
                        arguments,
                    } = state;
                    let Some(name) = name else {
                        debug!("Skipping tool call at index {index} because name is missing");
                        continue;
                    };
                    let item = ResponseItem::FunctionCall {
                        id: None,
                        name,
                        arguments,
                        call_id: id.unwrap_or_else(|| format!("tool-call-{index}")),
                    };
                    let _ = tx_event.send(Ok(ResponseEvent::OutputItemDone(item))).await;
                }
            }
        }
    }
}

async fn append_assistant_text(
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    assistant_item: &mut Option<ResponseItem>,
    text: String,
) {
    if assistant_item.is_none() {
        let item = ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![],
            end_turn: None,
        };
        *assistant_item = Some(item.clone());
        let _ = tx_event
            .send(Ok(ResponseEvent::OutputItemAdded(item)))
            .await;
    }

    if let Some(ResponseItem::Message { content, .. }) = assistant_item {
        content.push(ContentItem::OutputText { text: text.clone() });
        let _ = tx_event
            .send(Ok(ResponseEvent::OutputTextDelta(text.clone())))
            .await;
    }
}

async fn append_reasoning_text(
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    reasoning_item: &mut Option<ResponseItem>,
    text: String,
) {
    if reasoning_item.is_none() {
        let item = ResponseItem::Reasoning {
            id: String::new(),
            summary: Vec::new(),
            content: Some(vec![]),
            encrypted_content: None,
        };
        *reasoning_item = Some(item.clone());
        let _ = tx_event
            .send(Ok(ResponseEvent::OutputItemAdded(item)))
            .await;
    }

    if let Some(ResponseItem::Reasoning {
        content: Some(content),
        ..
    }) = reasoning_item
    {
        let content_index = content.len() as i64;
        content.push(ReasoningItemContent::ReasoningText { text: text.clone() });

        let _ = tx_event
            .send(Ok(ResponseEvent::ReasoningContentDelta {
                delta: text.clone(),
                content_index,
            }))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use codex_protocol::models::ResponseItem;
    use futures::TryStreamExt;
    use serde_json::json;
    use tokio::sync::mpsc;
    use tokio_util::io::ReaderStream;

    fn build_body(events: &[serde_json::Value]) -> String {
        let mut body = String::new();
        for e in events {
            body.push_str(&format!("event: message\ndata: {e}\n\n"));
        }
        body
    }

    /// Regression test: the stream should complete when we see a `[DONE]` sentinel.
    ///
    /// This is important for tests/mocks that don't immediately close the underlying
    /// connection after emitting the sentinel.
    #[tokio::test]
    async fn completes_on_done_sentinel_without_json() {
        let events = collect_events("event: message\ndata: [DONE]\n\n").await;
        assert_matches!(&events[..], [ResponseEvent::Completed { .. }]);
    }

    fn tool_call_delta(
        id: Option<&str>,
        index: i64,
        name: Option<&str>,
        arguments: Option<&str>,
    ) -> serde_json::Value {
        let mut function = serde_json::Map::new();
        if let Some(name) = name {
            function.insert("name".to_string(), json!(name));
        }
        if let Some(arguments) = arguments {
            function.insert("arguments".to_string(), json!(arguments));
        }
        let mut call = serde_json::Map::new();
        call.insert("index".to_string(), json!(index));
        call.insert("function".to_string(), serde_json::Value::Object(function));
        if let Some(id) = id {
            call.insert("id".to_string(), json!(id));
        }
        json!({ "choices": [{ "delta": { "tool_calls": [call] } }] })
    }

    fn assert_function_call(events: &[ResponseEvent], name: &str, arguments: &str, call_id: &str) {
        let found = events.iter().find(|event| {
            matches!(
                event,
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { name: n, arguments: a, call_id: c, .. })
                    if n == name && a == arguments && c == call_id
            )
        });
        assert!(
            found.is_some(),
            "expected FunctionCall {name} with complete arguments in {events:?}"
        );
    }

    /// A gateway that ends the stream with `finish_reason: "stop"` (instead of `"tool_calls"`,
    /// as many OpenAI-compatible bridges do) must still dispatch the buffered tool calls.
    #[tokio::test]
    async fn dispatches_buffered_tool_calls_when_stream_finishes_with_stop() {
        let body = build_body(&[
            json!({ "choices": [{ "delta": { "content": "checking a couple of details" } }] }),
            tool_call_delta(Some("call_1"), 0, Some("shell"), Some("{\"command\":")),
            tool_call_delta(None, 0, None, Some("\"ls -l\"}")),
            json!({ "choices": [{ "finish_reason": "stop" }] }),
        ]);
        let events = collect_events(&body).await;
        assert_matches!(events.last(), Some(ResponseEvent::Completed { .. }));
        assert_function_call(&events, "shell", "{\"command\":\"ls -l\"}", "call_1");
    }

    /// A gateway that closes the stream (or sends `[DONE]`) without any finish_reason chunk must
    /// also dispatch the buffered tool calls instead of dropping them silently.
    #[tokio::test]
    async fn dispatches_buffered_tool_calls_when_stream_ends_without_finish_reason() {
        let body = build_body(&[tool_call_delta(
            Some("call_2"),
            0,
            Some("shell"),
            Some("{\"command\":\"pwd\"}"),
        )]);
        let events = collect_events(&body).await;
        assert_matches!(events.last(), Some(ResponseEvent::Completed { .. }));
        assert_function_call(&events, "shell", "{\"command\":\"pwd\"}", "call_2");
    }

    /// When the response is truncated (`finish_reason: "length"`), all announced tool calls are
    /// dispatched before the error - a call whose arguments were cut off goes through too, so
    /// core reports it back to the model as a failed call instead of ending the turn silently.
    #[tokio::test]
    async fn truncated_stream_dispatches_complete_and_cut_off_calls() {
        let body = build_body(&[
            tool_call_delta(
                Some("call_ok"),
                0,
                Some("shell"),
                Some("{\"command\":\"id\"}"),
            ),
            tool_call_delta(Some("call_cut"), 1, Some("shell"), Some("{\"comma")),
            json!({ "choices": [{ "finish_reason": "length" }] }),
        ]);
        let reader = ReaderStream::new(std::io::Cursor::new(body.to_string()))
            .map_err(|err| codex_client::TransportError::Network(err.to_string()));
        let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(16);
        tokio::spawn(process_chat_sse(
            reader,
            tx,
            Duration::from_millis(1000),
            None,
        ));

        let mut events = Vec::new();
        let mut error = None;
        while let Some(ev) = rx.recv().await {
            match ev {
                Ok(ev) => events.push(ev),
                Err(err) => error = Some(err),
            }
        }
        assert_function_call(&events, "shell", "{\"command\":\"id\"}", "call_ok");
        let cut_call = events.iter().find_map(|event| match event {
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            }) if call_id == "call_cut" => Some((name.clone(), arguments.clone())),
            _ => None,
        });
        assert_matches!(
            cut_call,
            Some((name, arguments)) if name == "shell" && arguments == "{\"comma",
            "the cut-off call must still be dispatched so core reports it to the model"
        );
        assert_matches!(error, Some(ApiError::ContextWindowExceeded));
    }

    async fn collect_events(body: &str) -> Vec<ResponseEvent> {
        let reader = ReaderStream::new(std::io::Cursor::new(body.to_string()))
            .map_err(|err| codex_client::TransportError::Network(err.to_string()));
        let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(16);
        tokio::spawn(process_chat_sse(
            reader,
            tx,
            Duration::from_millis(1000),
            None,
        ));

        let mut out = Vec::new();
        while let Some(ev) = rx.recv().await {
            out.push(ev.expect("stream error"));
        }
        out
    }

    #[tokio::test]
    async fn concatenates_tool_call_arguments_across_deltas() {
        let delta_name = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "id": "call_a",
                        "index": 0,
                        "function": { "name": "do_a" }
                    }]
                }
            }]
        });

        let delta_args_1 = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "{ \"foo\":" }
                    }]
                }
            }]
        });

        let delta_args_2 = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "1}" }
                    }]
                }
            }]
        });

        let finish = json!({
            "choices": [{
                "finish_reason": "tool_calls"
            }]
        });

        let body = build_body(&[delta_name, delta_args_1, delta_args_2, finish]);
        let events = collect_events(&body).await;
        assert_matches!(
            &events[..],
            [
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id, name, arguments, .. }),
                ResponseEvent::Completed { .. }
            ] if call_id == "call_a" && name == "do_a" && arguments == "{ \"foo\":1}"
        );
    }

    #[tokio::test]
    async fn emits_multiple_tool_calls() {
        let delta_a = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "id": "call_a",
                        "function": { "name": "do_a", "arguments": "{\"foo\":1}" }
                    }]
                }
            }]
        });

        let delta_b = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "id": "call_b",
                        "function": { "name": "do_b", "arguments": "{\"bar\":2}" }
                    }]
                }
            }]
        });

        let finish = json!({
            "choices": [{
                "finish_reason": "tool_calls"
            }]
        });

        let body = build_body(&[delta_a, delta_b, finish]);
        let events = collect_events(&body).await;
        assert_matches!(
            &events[..],
            [
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id: call_a, name: name_a, arguments: args_a, .. }),
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id: call_b, name: name_b, arguments: args_b, .. }),
                ResponseEvent::Completed { .. }
            ] if call_a == "call_a" && name_a == "do_a" && args_a == "{\"foo\":1}" && call_b == "call_b" && name_b == "do_b" && args_b == "{\"bar\":2}"
        );
    }

    #[tokio::test]
    async fn emits_tool_calls_for_multiple_choices() {
        let payload = json!({
            "choices": [
                {
                    "delta": {
                        "tool_calls": [{
                            "id": "call_a",
                            "index": 0,
                            "function": { "name": "do_a", "arguments": "{}" }
                        }]
                    },
                    "finish_reason": "tool_calls"
                },
                {
                    "delta": {
                        "tool_calls": [{
                            "id": "call_b",
                            "index": 0,
                            "function": { "name": "do_b", "arguments": "{}" }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        });

        let body = build_body(&[payload]);
        let events = collect_events(&body).await;
        assert_matches!(
            &events[..],
            [
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id: call_a, name: name_a, arguments: args_a, .. }),
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id: call_b, name: name_b, arguments: args_b, .. }),
                ResponseEvent::Completed { .. }
            ] if call_a == "call_a" && name_a == "do_a" && args_a == "{}" && call_b == "call_b" && name_b == "do_b" && args_b == "{}"
        );
    }

    #[tokio::test]
    async fn merges_tool_calls_by_index_when_id_missing_on_subsequent_deltas() {
        let delta_with_id = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_a",
                        "function": { "name": "do_a", "arguments": "{ \"foo\":" }
                    }]
                }
            }]
        });

        let delta_without_id = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "1}" }
                    }]
                }
            }]
        });

        let finish = json!({
            "choices": [{
                "finish_reason": "tool_calls"
            }]
        });

        let body = build_body(&[delta_with_id, delta_without_id, finish]);
        let events = collect_events(&body).await;
        assert_matches!(
            &events[..],
            [
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id, name, arguments, .. }),
                ResponseEvent::Completed { .. }
            ] if call_id == "call_a" && name == "do_a" && arguments == "{ \"foo\":1}"
        );
    }

    #[tokio::test]
    async fn preserves_tool_call_name_when_empty_deltas_arrive() {
        let delta_with_name = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "id": "call_a",
                        "function": { "name": "do_a" }
                    }]
                }
            }]
        });

        let delta_with_empty_name = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "id": "call_a",
                        "function": { "name": "", "arguments": "{}" }
                    }]
                }
            }]
        });

        let finish = json!({
            "choices": [{
                "finish_reason": "tool_calls"
            }]
        });

        let body = build_body(&[delta_with_name, delta_with_empty_name, finish]);
        let events = collect_events(&body).await;
        assert_matches!(
            &events[..],
            [
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { name, arguments, .. }),
                ResponseEvent::Completed { .. }
            ] if name == "do_a" && arguments == "{}"
        );
    }

    #[tokio::test]
    async fn emits_tool_calls_even_when_content_and_reasoning_present() {
        let delta_content_and_tools = json!({
            "choices": [{
                "delta": {
                    "content": [{"text": "hi"}],
                    "reasoning": "because",
                    "tool_calls": [{
                        "id": "call_a",
                        "function": { "name": "do_a", "arguments": "{}" }
                    }]
                }
            }]
        });

        let finish = json!({
            "choices": [{
                "finish_reason": "tool_calls"
            }]
        });

        let body = build_body(&[delta_content_and_tools, finish]);
        let events = collect_events(&body).await;

        assert_matches!(
            &events[..],
            [
                ResponseEvent::OutputItemAdded(ResponseItem::Reasoning { .. }),
                ResponseEvent::ReasoningContentDelta { .. },
                ResponseEvent::OutputItemAdded(ResponseItem::Message { .. }),
                ResponseEvent::OutputTextDelta(delta),
                ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. }),
                ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { call_id, name, .. }),
                ResponseEvent::OutputItemDone(ResponseItem::Message { .. }),
                ResponseEvent::Completed { .. }
            ] if delta == "hi" && call_id == "call_a" && name == "do_a"
        );
    }

    /// Semantic update (B10): the old owl test asserted that a tool call streamed before a
    /// `finish_reason: "stop"` chunk was always dropped. That dropped exactly the Open WebUI
    /// case users reported: backends that put tool calls in deltas but finish with "stop",
    /// making owl announce an action and then silently do nothing. Valid calls are dispatched;
    /// B13: calls whose arguments were cut off mid-stream are dispatched too (core answers them
    /// with a "failed to parse function arguments" tool result, so the model continues on its
    /// own instead of the turn stopping after the announcement). Only calls with no name are
    /// dropped.
    #[tokio::test]
    async fn on_stop_finish_dispatches_valid_indexless_and_truncated_calls() {
        let valid_indexless = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "id": "call_a",
                        "function": { "name": "do_a", "arguments": "{}" }
                    }]
                }
            }]
        });

        let invalid = json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "id": "call_b",
                        "index": 1,
                        "function": { "name": "do_b", "arguments": "{\"unclosed" }
                    }]
                }
            }]
        });

        let finish_stop = json!({
            "choices": [{
                "finish_reason": "stop"
            }]
        });

        let body = build_body(&[valid_indexless, invalid, finish_stop]);
        let events = collect_events(&body).await;

        assert_function_call(&events, "do_a", "{}", "call_a");
        let truncated_call = events.iter().find_map(|ev| match ev {
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            }) if call_id == "call_b" => Some((name.clone(), arguments.clone())),
            _ => None,
        });
        assert_matches!(
            truncated_call,
            Some((name, arguments)) if name == "do_b" && arguments == "{\"unclosed",
            "the truncated call must be dispatched so core reports the failure to the model"
        );
        assert_matches!(events.last(), Some(ResponseEvent::Completed { .. }));
    }
}
