#![allow(clippy::expect_used)]

//! End-to-end tests for the locally executed `web_search` tool on the
//! chat-completions wire: the user's server requests a search, owl runs the
//! query against the user's own `[search]` endpoint, and the results go back
//! to the server as an ordinary tool result.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_core::config::types::WebSearchBackend;
use codex_core::protocol::AskForApproval;
use codex_core::protocol::Op;
use codex_core::protocol::SandboxPolicy;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::protocol::EventMsg;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event_with_timeout;
use serde_json::json;

use wiremock::Mock;
use wiremock::Request;
use wiremock::Respond;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

type RequestSink = std::sync::Arc<std::sync::Mutex<Vec<Request>>>;

fn new_sink() -> RequestSink {
    std::sync::Arc::new(std::sync::Mutex::new(Vec::new()))
}

fn seen_requests(sink: &RequestSink) -> Vec<Request> {
    sink.lock().expect("recorded requests").clone()
}

struct ChatResponder {
    call_count: AtomicUsize,
    bodies: Vec<String>,
    sink: RequestSink,
}

impl Respond for ChatResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        self.sink
            .lock()
            .expect("chat requests")
            .push(request.clone());
        let index = self.call_count.fetch_add(1, Ordering::SeqCst);
        match self.bodies.get(index) {
            Some(body) => ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body.clone()),
            None => ResponseTemplate::new(500),
        }
    }
}

struct SearchResponder {
    body: serde_json::Value,
    sink: RequestSink,
}

impl Respond for SearchResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        self.sink
            .lock()
            .expect("search requests")
            .push(request.clone());
        ResponseTemplate::new(200).set_body_json(self.body.clone())
    }
}

fn sse_tool_call(call_id: &str, name: &str, arguments: &str) -> String {
    let chunk = json!({
        "choices": [
            {
                "delta": {
                    "tool_calls": [
                        {
                            "id": call_id,
                            "type": "function",
                            "function": {"name": name, "arguments": arguments}
                        }
                    ]
                },
                "finish_reason": "tool_calls"
            }
        ]
    });
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        serde_json::to_string(&chunk).expect("sse tool call")
    )
}

fn sse_text(text: &str) -> String {
    let chunk = json!({
        "choices": [
            {"delta": {"content": text}, "finish_reason": "stop"}
        ]
    });
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        serde_json::to_string(&chunk).expect("sse text")
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_wire_web_search_executes_locally_and_returns_results() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let call_id = "ws-call-1";

    let search_sink = new_sink();
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(SearchResponder {
            body: json!({
                "web": [
                    {"title": "Owls", "url": "https://example.com/owl", "content": "Nocturnal raptors."},
                    {"title": "Barn Owls", "url": "https://example.com/barn", "content": "Barn dwellers."},
                    {"title": "Beyond Limit", "url": "https://example.com/limit", "content": "Must be truncated."}
                ]
            }),
            sink: search_sink.clone(),
        })
        .mount(&server)
        .await;

    let chat_sink = new_sink();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ChatResponder {
            call_count: AtomicUsize::new(0),
            bodies: vec![
                sse_tool_call(
                    call_id,
                    "web_search",
                    &serde_json::to_string(&json!({"query": "owls nocturnal"})).expect("args json"),
                ),
                sse_text("search complete"),
            ],
            sink: chat_sink.clone(),
        })
        .mount(&server)
        .await;

    let search_url = format!("{}/search", server.uri());
    let fixture = test_codex()
        .with_config(move |config| {
            config.model_provider.wire_api = codex_core::WireApi::Chat;
            config.web_search_mode = Some(WebSearchMode::Live);
            config.web_search_backend = Some(WebSearchBackend {
                url: search_url,
                api_key: Some("test-search-key".to_string()),
                params: BTreeMap::from([("format".to_string(), "json".to_string())]),
                max_results: 2,
                timeout: Duration::from_secs(10),
            });
        })
        .build(&server)
        .await
        .expect("create test owl conversation");

    let session_model = fixture.session_configured.model.clone();
    fixture
        .codex
        .submit(Op::UserTurn {
            items: vec![UserInput::Text {
                text: "search for owls".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            cwd: fixture.cwd.path().to_path_buf(),
            approval_policy: AskForApproval::Never,
            sandbox_policy: SandboxPolicy::ReadOnly,
            model: session_model,
            effort: None,
            summary: ReasoningSummary::Auto,
            collaboration_mode: None,
            personality: None,
        })
        .await
        .expect("submit turn");

    // The turn must surface the dedicated web search events so the UI can show
    // the search activity.
    let begin = wait_for_event_with_timeout(
        &fixture.codex,
        |ev| matches!(ev, EventMsg::WebSearchBegin(_)),
        Duration::from_secs(3),
    )
    .await;
    assert!(matches!(begin, EventMsg::WebSearchBegin(_)));
    let end = wait_for_event_with_timeout(
        &fixture.codex,
        |event| matches!(event, EventMsg::WebSearchEnd(end) if end.query == "owls nocturnal"),
        Duration::from_secs(3),
    )
    .await;
    let EventMsg::WebSearchEnd(end) = end else {
        unreachable!("web search end");
    };
    assert_eq!(end.call_id, call_id);

    // The turn must wrap up before the outbound requests can be inspected.
    let complete = wait_for_event_with_timeout(
        &fixture.codex,
        |ev| matches!(ev, EventMsg::TurnComplete(_)),
        Duration::from_secs(3),
    )
    .await;
    assert!(matches!(complete, EventMsg::TurnComplete(_)));

    // The search went to the user's endpoint with the configured query and key.
    let search_requests = seen_requests(&search_sink);
    assert_eq!(search_requests.len(), 1, "owl executes the search itself");
    let search_request = &search_requests[0];
    assert_eq!(search_request.url.path(), "/search");
    let query_pairs: BTreeMap<String, String> = search_request
        .url
        .query_pairs()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    assert_eq!(
        query_pairs.get("q").map(String::as_str),
        Some("owls nocturnal")
    );
    assert_eq!(query_pairs.get("format").map(String::as_str), Some("json"));
    assert_eq!(
        search_request
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        Some("Bearer test-search-key")
    );

    // First model request offered web_search as a plain chat function tool...
    let chat_requests = seen_requests(&chat_sink);
    assert_eq!(
        chat_requests.len(),
        2,
        "tool result must follow in a second request"
    );
    let first_body: serde_json::Value =
        serde_json::from_slice(&chat_requests[0].body).expect("first chat body");
    let web_search_tool = first_body["tools"]
        .as_array()
        .expect("chat tools array")
        .iter()
        .find(|tool| tool.get("name").and_then(serde_json::Value::as_str) == Some("web_search"))
        .expect("chat wire must offer web_search as a function tool");
    assert_eq!(web_search_tool["type"], "function");

    // ...and the second request carried the truncated results as a tool message.
    let second_body: serde_json::Value =
        serde_json::from_slice(&chat_requests[1].body).expect("second chat body");
    let tool_message = second_body["messages"]
        .as_array()
        .expect("chat messages array")
        .iter()
        .find(|message| {
            message
                .get("tool_call_id")
                .and_then(serde_json::Value::as_str)
                == Some(call_id)
        })
        .expect("tool result message for the web_search call");
    let content = tool_message["content"]
        .as_str()
        .expect("tool message content string");
    assert!(content.contains("Title: Owls"), "got: {content}");
    assert!(
        content.contains("https://example.com/owl"),
        "got: {content}"
    );
    assert!(content.contains("Barn Owls"), "got: {content}");
    assert!(
        !content.contains("Beyond Limit"),
        "max_results must truncate: {content}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_wire_without_web_search_mode_offers_no_web_search_tool() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let search_sink = new_sink();
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(SearchResponder {
            body: json!({"web": []}),
            sink: search_sink.clone(),
        })
        .mount(&server)
        .await;

    let chat_sink = new_sink();
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ChatResponder {
            call_count: AtomicUsize::new(0),
            bodies: vec![sse_text("no search needed")],
            sink: chat_sink.clone(),
        })
        .mount(&server)
        .await;

    let fixture = test_codex()
        .with_config(move |config| {
            config.model_provider.wire_api = codex_core::WireApi::Chat;
        })
        .build(&server)
        .await
        .expect("create test owl conversation");

    fixture.submit_turn("hello").await.expect("submit turn");

    let chat_requests = seen_requests(&chat_sink);
    assert_eq!(chat_requests.len(), 1);
    let body: serde_json::Value =
        serde_json::from_slice(&chat_requests[0].body).expect("chat body");
    assert!(
        body["tools"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .all(|tool| tool.get("name").and_then(serde_json::Value::as_str) != Some("web_search")),
        "web_search must not be offered when the mode is unset"
    );
    assert!(seen_requests(&search_sink).is_empty());
}
