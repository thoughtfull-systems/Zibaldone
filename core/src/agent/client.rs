//! The small Responses transport. Session and tool behavior stay in the parent module.

use std::time::Duration;

use serde_json::{Value, json};

use super::agent_error;
use crate::{Error, Result};

const RESPONSES: &str = "https://api.openai.com/v1/responses";

pub(super) struct Client {
    pub(super) endpoint: String,
    http: reqwest::Client,
    key: String,
}

impl Client {
    pub(super) fn new(key: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(agent_error)?;
        Ok(Self { endpoint: RESPONSES.into(), http, key: key.trim().into() })
    }

    async fn post(&self, request: Value) -> Result<reqwest::Response> {
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.key)
            .json(&request)
            .send()
            .await
            .map_err(agent_error)?;
        checked(response, &self.key).await
    }

    pub(super) async fn json(&self, request: Value) -> Result<Value> {
        self.post(request).await?.json().await.map_err(agent_error)
    }

    pub(super) async fn stream(
        &self,
        request: Value,
        mut delta: impl FnMut(&str) -> Result<()> + Send,
        mut model: impl FnMut(&str) -> Result<()> + Send,
    ) -> Result<Value> {
        let mut response = self.post(request).await?;
        let mut decoder = Events::default();
        while let Some(chunk) = response.chunk().await.map_err(agent_error)? {
            for event in decoder.feed(&chunk)? {
                match event["type"].as_str().unwrap_or_default() {
                    "response.created" => {
                        if let Some(id) = event["response"]["model"].as_str() {
                            model(id)?;
                        }
                    }
                    "response.output_text.delta" | "response.refusal.delta" => {
                        delta(event["delta"].as_str().unwrap_or_default())?;
                    }
                    "response.completed" => return Ok(event["response"].clone()),
                    "error" | "response.failed" | "response.incomplete" => {
                        return Err(api_error(&event, None, &self.key));
                    }
                    _ => {}
                }
            }
        }
        Err(Error::Agent(
            concat!(
                "OpenAI's stream closed without a completion or failure event; ",
                "retry to continue"
            )
            .into(),
        ))
    }
}

async fn checked(response: reqwest::Response, key: &str) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let error: Value = response.json().await.unwrap_or(Value::Null);
    Err(api_error(&error, Some(status), key))
}

/// HTTP errors, nested stream errors, and failed responses carry the same useful details in
/// different envelopes. Keep their codes instead of replacing them with a generic stream error.
fn api_error(payload: &Value, status: Option<reqwest::StatusCode>, key: &str) -> Error {
    let response = payload.get("response").unwrap_or(payload);
    let detail = response.get("error").filter(|value| !value.is_null()).unwrap_or(response);
    let code = detail["code"]
        .as_str()
        .filter(|code| !code.is_empty())
        .or_else(|| response["incomplete_details"]["reason"].as_str())
        .or_else(|| {
            detail["type"].as_str().filter(|kind| {
                !matches!(*kind, "error" | "response.failed" | "response.incomplete")
            })
        });
    let mut labels = Vec::new();
    if let Some(status) = status {
        labels.push(format!("HTTP {status}"));
    }
    if let Some(code) = code {
        labels.push(code.into());
    } else if let Some(event) = payload["type"].as_str() {
        labels.push(event.into());
    }
    let fallback = match code {
        Some("credit_balance_exhausted") => {
            "The account has no prepaid credits remaining. Add OpenAI credits before retrying."
        }
        Some("insufficient_quota") => {
            "OpenAI quota is unavailable. Check credits, billing, and spending limits."
        }
        Some("organization_spend_limit_exceeded" | "project_spend_limit_exceeded") => {
            "An OpenAI spending limit was reached. Check the organization or project limit."
        }
        Some("organization_usage_limit_exceeded") => {
            "The organization reached its OpenAI usage limit. Request a higher limit."
        }
        Some("rate_limit_exceeded" | "slow_down") => {
            "OpenAI's request rate limit was reached. Wait before retrying."
        }
        Some("max_output_tokens") => {
            "OpenAI reached an output or context token limit. Try a shorter request or a new chat."
        }
        Some("content_filter") => "OpenAI's content filter stopped the response.",
        _ if status == Some(reqwest::StatusCode::UNAUTHORIZED) => {
            "OpenAI rejected authentication. Check the API key and project settings."
        }
        _ => "The request failed without further error details.",
    };
    let message = detail["message"]
        .as_str()
        .filter(|message| !message.is_empty())
        .or_else(|| detail.as_str())
        .or_else(|| payload["message"].as_str())
        .unwrap_or(fallback);
    let label = if labels.is_empty() { String::new() } else { format!(" ({})", labels.join("; ")) };
    Error::Agent(format!("OpenAI{label}: {}", redact(message, key)))
}

fn redact(message: &str, key: &str) -> String {
    let message = if key.is_empty() { message.into() } else { message.replace(key, "[redacted]") };
    let mut safe = String::new();
    // Authentication messages may quote a shortened key, which won't match the full credential.
    for word in message.split_inclusive(char::is_whitespace) {
        let start = word
            .match_indices("sk-")
            .find(|(at, _)| {
                word[..*at].chars().next_back().is_none_or(|c| !c.is_ascii_alphanumeric())
            })
            .map(|(at, _)| at);
        if let Some(start) = start {
            safe.push_str(&word[..start]);
            safe.push_str("[redacted]");
            safe.push_str(&word[word.trim_end_matches(char::is_whitespace).len()..]);
        } else {
            safe.push_str(word);
        }
    }
    safe
}

/// Model identifiers available to the configured account, with room for manual entry in the UI.
pub async fn models(key: &str) -> Result<Vec<String>> {
    if key.trim().is_empty() {
        return Err(Error::Agent("Enter an API key first".into()));
    }
    let client = Client::new(key)?;
    let response = client
        .http
        .get("https://api.openai.com/v1/models")
        .bearer_auth(key.trim())
        .send()
        .await
        .map_err(agent_error)?;
    let data: Value = checked(response, key).await?.json().await.map_err(agent_error)?;
    let mut models: Vec<String> = data["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["id"].as_str().map(str::to_owned))
        .collect();
    models.sort();
    models.dedup();
    Ok(models)
}

pub(super) fn text(output: &Value) -> String {
    output
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|item| item["content"].as_array().into_iter().flatten())
        .filter_map(|part| part["text"].as_str().or_else(|| part["refusal"].as_str()))
        .collect()
}

pub(super) fn tools() -> Value {
    let tool = |name: &str, description: &str, arguments: &[&str]| {
        let properties: serde_json::Map<String, Value> =
            arguments.iter().map(|name| ((*name).into(), json!({"type": "string"}))).collect();
        json!({
            "description": description,
            "name": name,
            "parameters": {
                "additionalProperties": false,
                "properties": properties,
                "required": arguments,
                "type": "object"
            },
            "strict": true,
            "type": "function"
        })
    };
    json!([
        tool(
            "create_note",
            "Create a markdown note. The owner chooses its destination.",
            &["body"]
        ),
        tool("get_note", "Read a note by identifier from a currently shown space.", &["id"]),
        tool("search_notes", "Search shown spaces for note IDs, titles, and snippets.", &["query"]),
        tool(
            "write_note",
            concat!(
                "Replace a shown note's complete markdown body. Read it with get_note first, ",
                "preserve content the owner didn't ask to change, and reread if it changed."
            ),
            &["body", "id"]
        )
    ])
}

/// Server-sent events can split UTF-8, lines, and JSON at any network boundary.
#[derive(Default)]
struct Events {
    buffer: Vec<u8>,
    data: Vec<String>,
}

impl Events {
    fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Value>> {
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() > 16 << 20 {
            return Err(Error::Agent("A stream event is too large".into()));
        }
        let mut events = Vec::new();
        while let Some(end) = self.buffer.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            let line = std::str::from_utf8(&line).map_err(agent_error)?;
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                let data = self.data.join("\n");
                self.data.clear();
                if !data.is_empty() && data != "[DONE]" {
                    events.push(serde_json::from_str(&data).map_err(agent_error)?);
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data.push(data.strip_prefix(' ').unwrap_or(data).into());
            }
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_preserves_split_unicode_and_multiline_events() {
        let source = concat!(
            ": heartbeat\r\n",
            "event: response.output_text.delta\r\n",
            "data: {\"type\":\"response.output_text.delta\",\r\n",
            "data: \"delta\":\"café 📝\"}\r\n\r\n",
            "data: [DONE]\n\n"
        );
        // Every possible split, including within a multibyte character and between CR and LF.
        for split in 0..=source.len() {
            let mut decoder = Events::default();
            let mut events = decoder.feed(&source.as_bytes()[..split]).unwrap();
            events.extend(decoder.feed(&source.as_bytes()[split..]).unwrap());
            assert_eq!(
                events,
                vec![json!({
                    "type": "response.output_text.delta", "delta": "café 📝"
                })]
            );
        }
    }

    #[test]
    fn tool_argument_names_are_dynamic_json_keys() {
        let tools = tools();
        assert_eq!(tools[0]["parameters"]["properties"]["body"]["type"], "string");
        assert_eq!(tools[1]["parameters"]["properties"]["id"]["type"], "string");
        assert_eq!(tools[3]["parameters"]["required"], json!(["body", "id"]));
    }

    #[test]
    fn missing_messages_still_report_the_code_or_incomplete_reason() {
        for (payload, code, explanation) in [
            (
                json!({"type": "error", "error": {"type": "insufficient_quota"}}),
                "insufficient_quota",
                "credits, billing",
            ),
            (
                json!({"type": "response.incomplete", "response": {
                    "incomplete_details": {"reason": "max_output_tokens"}
                }}),
                "max_output_tokens",
                "token limit",
            ),
            (
                json!({"type": "response.incomplete", "response": {
                    "incomplete_details": {"reason": "content_filter"}
                }}),
                "content_filter",
                "content filter",
            ),
        ] {
            let error = api_error(&payload, None, "test-key").to_string();
            assert!(error.contains(code), "{error}");
            assert!(error.contains(explanation), "{error}");
        }
        let error = api_error(&json!({"type": "response.failed"}), None, "").to_string();
        assert!(error.contains("response.failed"), "{error}");
    }

    #[test]
    fn error_messages_redact_full_and_shortened_credentials() {
        let payload = json!({"error": {"code": "invalid_api_key",
            "message": "Invalid test-key or 'sk-short***suffix'. Check the API key."}});
        let error =
            api_error(&payload, Some(reqwest::StatusCode::UNAUTHORIZED), "test-key").to_string();
        assert!(error.contains("HTTP 401"));
        assert!(error.contains("invalid_api_key"));
        assert!(!error.contains("test-key"));
        assert!(!error.contains("sk-short"));
        assert_eq!(redact("A risk-level message", ""), "A risk-level message");
    }
}
