//! Locally executed `web_search` tool.
//!
//! Chat-completions servers cannot host a search themselves, so owl offers
//! `web_search` as an ordinary function tool and runs the query against the
//! user's own search endpoint (the `[search]` table in config.toml), then
//! hands the results back to the model as a normal tool result.

use crate::config::types::WebSearchBackend;
use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use async_trait::async_trait;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WebSearchBeginEvent;
use codex_protocol::protocol::WebSearchEndEvent;
use serde::Deserialize;
use serde_json::Value;

pub struct WebSearchHandler;

#[derive(Debug, Deserialize)]
struct WebSearchArgs {
    query: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SearchResult {
    title: String,
    url: String,
    content: String,
}

#[async_trait]
impl ToolHandler for WebSearchHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            call_id,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "web_search handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: WebSearchArgs = parse_arguments(&arguments)?;
        if args.query.trim().is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "web_search requires a non-empty query".to_string(),
            ));
        }

        let Some(backend) = turn.tools_config.web_search_backend.clone() else {
            return Err(FunctionCallError::RespondToModel(
                "web_search is enabled but no endpoint is configured; add `url` under `[search]` in config.toml"
                    .to_string(),
            ));
        };

        session
            .send_event(
                &turn,
                EventMsg::WebSearchBegin(WebSearchBeginEvent {
                    call_id: call_id.clone(),
                }),
            )
            .await;

        let outcome = perform_web_search(&backend, args.query.trim()).await;

        session
            .send_event(
                &turn,
                EventMsg::WebSearchEnd(WebSearchEndEvent {
                    call_id,
                    query: args.query.trim().to_string(),
                }),
            )
            .await;

        let content = outcome?;

        Ok(ToolOutput::Function {
            content,
            content_items: None,
            success: Some(true),
        })
    }
}

async fn perform_web_search(
    backend: &WebSearchBackend,
    query: &str,
) -> Result<String, FunctionCallError> {
    let request_error = |err: String| {
        FunctionCallError::RespondToModel(format!("web search request failed: {err}"))
    };

    let url = build_search_url(backend, query).map_err(request_error)?;

    let client = reqwest::Client::builder()
        .timeout(backend.timeout)
        .build()
        .map_err(|err| request_error(err.to_string()))?;

    let mut request = client.get(url);
    if let Some(api_key) = backend.api_key.as_deref() {
        request = request.bearer_auth(api_key);
    }

    let response = request.send().await.map_err(|err| {
        if err.is_timeout() {
            request_error(format!(
                "search endpoint timed out after {} ms",
                backend.timeout.as_millis()
            ))
        } else {
            request_error(err.to_string())
        }
    })?;

    let status = response.status();
    if !status.is_success() {
        return Err(request_error(format!("HTTP {status}")));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|err| request_error(format!("search response was not valid JSON: {err}")))?;

    let results = parse_search_results(&body, backend.max_results);
    Ok(format_results(&results))
}

/// Append `q=<query>` and the configured extra params to the endpoint URL.
fn build_search_url(backend: &WebSearchBackend, query: &str) -> Result<String, String> {
    let mut url = url::Url::parse(&backend.url).map_err(|err| err.to_string())?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("q", query);
        for (key, value) in &backend.params {
            pairs.append_pair(key, value);
        }
    }
    Ok(url.to_string())
}

/// Pull result entries out of a SearXNG-style JSON body.
///
/// Accepts the common `web` / `results` array names and the usual snippet
/// field spellings so plain SearXNG, most proxies, and simple custom gateways
/// all work without extra configuration.
fn parse_search_results(body: &Value, max_results: usize) -> Vec<SearchResult> {
    let entries = body
        .get("web")
        .or_else(|| body.get("results"))
        .and_then(Value::as_array);
    let Some(entries) = entries else {
        return Vec::new();
    };

    entries
        .iter()
        .filter_map(|entry| {
            let title = entry.get("title").and_then(Value::as_str).unwrap_or("");
            let url = entry
                .get("url")
                .or_else(|| entry.get("link"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if title.is_empty() && url.is_empty() {
                return None;
            }
            let content = ["content", "description", "snippet", "text"]
                .iter()
                .find_map(|key| entry.get(*key).and_then(Value::as_str))
                .unwrap_or("")
                .trim()
                .to_string();
            Some(SearchResult {
                title: title.trim().to_string(),
                url: url.to_string(),
                content,
            })
        })
        .take(max_results)
        .collect()
}

fn format_results(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "No results.".to_string();
    }
    let mut out = String::new();
    for (index, result) in results.iter().enumerate() {
        out.push_str(&format!("{}.\n", index + 1));
        out.push_str(&format!("Title: {}\n", result.title));
        out.push_str(&format!("URL: {}\n", result.url));
        if !result.content.is_empty() {
            out.push_str(&format!("Snippet: {}\n", result.content));
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::time::Duration;

    fn backend(url: &str) -> WebSearchBackend {
        WebSearchBackend {
            url: url.to_string(),
            api_key: None,
            params: BTreeMap::from([("format".to_string(), "json".to_string())]),
            max_results: 3,
            timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn build_search_url_appends_query_and_params() {
        let url = build_search_url(&backend("http://127.0.0.1:8888/search"), "owls habitat")
            .expect("build url");
        assert_eq!(
            url,
            "http://127.0.0.1:8888/search?q=owls+habitat&format=json"
        );
    }

    #[test]
    fn build_search_url_keeps_existing_query_params() {
        let url = build_search_url(
            &backend("http://127.0.0.1:8888/search?categories=news"),
            "owls",
        )
        .expect("build url");
        assert_eq!(
            url,
            "http://127.0.0.1:8888/search?categories=news&q=owls&format=json"
        );
    }

    #[test]
    fn build_search_url_rejects_invalid_endpoint() {
        assert!(build_search_url(&backend("not a url"), "owls").is_err());
    }

    #[test]
    fn parse_results_accepts_searxng_web_shape() {
        let body = json!({
            "web": [
                {"title": "Owls", "url": "https://example.com/owl", "content": "Nocturnal birds."},
                {"title": "Habitats", "link": "https://example.com/habitat", "description": "Forest and barn."},
                {"title": "Third", "url": "https://example.com/third", "snippet": "Short."},
                {"title": "Fourth", "url": "https://example.com/fourth", "content": "Dropped."},
            ]
        });
        let results = parse_search_results(&body, 3);
        assert_eq!(results.len(), 3, "max_results should truncate");
        assert_eq!(results[0].title, "Owls");
        assert_eq!(results[1].url, "https://example.com/habitat");
        assert_eq!(results[2].content, "Short.");
    }

    #[test]
    fn parse_results_accepts_generic_results_array() {
        let body = json!({
            "results": [
                {"title": "One", "url": "https://example.com/one", "text": "First hit."}
            ]
        });
        let results = parse_search_results(&body, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "First hit.");
    }

    #[test]
    fn parse_results_skips_entries_without_title_and_url() {
        let body = json!({
            "results": [
                {"title": "", "url": "", "content": "junk"},
                {"title": "Kept", "url": "https://example.com/kept"}
            ]
        });
        let results = parse_search_results(&body, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Kept");
        assert_eq!(results[0].content, "");
    }

    #[test]
    fn parse_results_handles_missing_or_empty_payloads() {
        assert!(parse_search_results(&json!({}), 5).is_empty());
        assert!(parse_search_results(&json!({"web": []}), 5).is_empty());
    }

    #[test]
    fn format_results_numbers_entries_and_notes_empty() {
        let results = vec![
            SearchResult {
                title: "Owls".to_string(),
                url: "https://example.com/owl".to_string(),
                content: "Nocturnal birds.".to_string(),
            },
            SearchResult {
                title: "No snippet".to_string(),
                url: "https://example.com/none".to_string(),
                content: String::new(),
            },
        ];
        assert_eq!(
            format_results(&results),
            "1.\nTitle: Owls\nURL: https://example.com/owl\nSnippet: Nocturnal birds.\n\n2.\nTitle: No snippet\nURL: https://example.com/none"
        );
        assert_eq!(format_results(&[]), "No results.");
    }
}
