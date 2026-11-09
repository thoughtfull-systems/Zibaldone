//! Agent specification: session recovery, model provenance, tools, and durable archiving.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use pulldown_cmark::{Event, Parser, Tag};

use super::*;

struct TestHost {
    notes: Mutex<BTreeMap<Id, Value>>,
    prompt: Mutex<String>,
    prompt_reads: AtomicUsize,
    creates: AtomicUsize,
    destination: Id,
    wait_for_destination: bool,
}

impl TestHost {
    fn new() -> Self {
        Self {
            notes: Mutex::default(),
            prompt: Mutex::new("Original instructions".into()),
            prompt_reads: AtomicUsize::new(0),
            creates: AtomicUsize::new(0),
            destination: Id::generate(),
            wait_for_destination: false,
        }
    }
}

impl Host for TestHost {
    fn prompt(&self, _: Id) -> impl Future<Output = Result<String>> {
        self.prompt_reads.fetch_add(1, Ordering::SeqCst);
        std::future::ready(Ok(self.prompt.lock().unwrap().clone()))
    }

    fn get_note(&self, id: Id) -> impl Future<Output = Result<Value>> {
        std::future::ready(
            self.notes
                .lock()
                .unwrap()
                .get(&id)
                .cloned()
                .ok_or_else(|| Error::NotFound(id.to_string())),
        )
    }

    fn search_notes(&self, _: String) -> impl Future<Output = Result<Value>> {
        std::future::ready(Ok(json!([])))
    }

    fn write_note(
        &self,
        note: Id,
        expected: String,
        body: String,
    ) -> impl Future<Output = Result<Value>> {
        let mut notes = self.notes.lock().unwrap();
        let result = if let Some(current) = notes.get_mut(&note) {
            if current["body"] != expected && current["body"] != body {
                Err(Error::Agent("The note changed after reading".into()))
            } else {
                current["body"] = Value::String(body);
                Ok(current.clone())
            }
        } else {
            Err(Error::NotFound(note.to_string()))
        };
        std::future::ready(result)
    }

    async fn choose_space(&self, _: String) -> Option<Id> {
        if self.wait_for_destination {
            std::future::pending().await
        } else {
            Some(self.destination)
        }
    }

    fn create_note(
        &self,
        creation: Creation,
        type_name: String,
        text: String,
    ) -> impl Future<Output = Result<Value>> {
        let mut notes = self.notes.lock().unwrap();
        std::future::ready(Ok(notes
            .entry(creation.id)
            .or_insert_with(|| {
                self.creates.fetch_add(1, Ordering::SeqCst);
                json!({"body": text, "id": creation.id, "space": creation.space, "type": type_name})
            })
            .clone()))
    }
}

fn message(text: &str) -> Value {
    json!({"type": "message", "role": "assistant", "content": [
        {"type": "output_text", "text": text, "annotations": []}
    ]})
}

fn function_call(name: &str, call_id: &str, arguments: Value) -> Value {
    json!({
        "type": "function_call", "name": name, "call_id": call_id,
        "arguments": arguments.to_string()
    })
}

fn response(model: &str, output: Vec<Value>) -> Value {
    let mut response = json!({"model": model, "status": "completed"});
    response["output"] = Value::Array(output);
    response
}

fn stream(response: Value) -> String {
    let mut event = json!({"type": "response.completed"});
    event["response"] = response;
    format!("data: {event}\n\n")
}

/// A real local HTTP exchange checks the request body and SSE decoding without an API key.
fn server(responses: Vec<String>) -> (client::Client, std::thread::JoinHandle<Vec<Value>>) {
    server_with_status(responses.into_iter().map(|body| (200, body)).collect())
}

fn server_with_status(
    responses: Vec<(u16, String)>,
) -> (client::Client, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = client::Client::new("test-key").unwrap();
    client.endpoint = format!("http://{}/responses", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut bytes = Vec::new();
            let end = loop {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            while bytes.len() < end + length {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&buffer[..count]);
            }
            requests.push(serde_json::from_slice(&bytes[end..end + length]).unwrap());
            write!(
                socket,
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\n\
                Content-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
        requests
    });
    (client, thread)
}

#[tokio::test]
async fn api_errors_report_nested_and_flat_details_and_keep_partial_replies() {
    for (status, payload, code, message) in [
        (
            200,
            json!({"type": "error", "error": {
                "code": "credit_balance_exhausted", "type": "insufficient_quota",
                "message": "Add credits to continue."
            }}),
            "credit_balance_exhausted",
            "Add credits to continue.",
        ),
        (
            200,
            json!({"type": "error", "code": "rate_limit_exceeded",
            "message": "Too many requests."}),
            "rate_limit_exceeded",
            "Too many requests.",
        ),
        (
            200,
            json!({"type": "response.failed", "response": {"error": {
                "code": "server_error", "message": "Please retry later."
            }}}),
            "server_error",
            "Please retry later.",
        ),
        (
            200,
            json!({"type": "response.incomplete", "response": {
                "incomplete_details": {"reason": "max_output_tokens"}
            }}),
            "max_output_tokens",
            "token limit",
        ),
        (
            429,
            json!({"error": {
                "code": "project_spend_limit_exceeded", "type": "insufficient_quota",
                "message": "Project spending limit reached."
            }}),
            "project_spend_limit_exceeded",
            "Project spending limit reached.",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let agent = Agent::open(root.path()).unwrap();
        let body = if status == 200 {
            format!(
                "data: {}\n\ndata: {payload}\n\n",
                json!({
                    "type": "response.output_text.delta", "delta": "Partial reply"
                })
            )
        } else {
            payload.to_string()
        };
        let (client, server) = server_with_status(vec![(status, body)]);
        let error = agent
            .run_client(
                &TestHost::new(),
                Settings {
                    api_key: "test-key".into(),
                    model: "model-a".into(),
                    ..Settings::default()
                },
                Some(("Question".into(), None)),
                &client,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(code), "{error}");
        assert!(error.contains(message), "{error}");
        if status == 429 {
            assert!(error.contains("HTTP 429"), "{error}");
        }
        let session = agent.snapshot();
        assert_eq!(session.turns[0].status, Status::Failed);
        assert_eq!(session.turns[0].error.as_deref(), Some(error.as_str()));
        if status == 200 {
            assert_eq!(session.turns[0].reply, "Partial reply");
        }
        // A quota failure never automatically resubmits the request.
        assert_eq!(server.join().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn turns_pin_models_snapshot_prompt_and_replay_reasoning_with_tool_results() {
    let root = tempfile::tempdir().unwrap();
    let agent = Agent::open(root.path()).unwrap();
    let host = TestHost::new();
    let reasoning = json!({"type": "reasoning", "encrypted_content": "opaque", "summary": []});
    let call = json!({"type": "function_call", "name": "create_note", "call_id": "call_1",
        "arguments": json!({"body": "# Created note\n\nSome text"}).to_string()});
    let (client, server) = server(vec![
        stream(response("model-a-snapshot", vec![reasoning, call])),
        stream(response("model-a-snapshot", vec![message("Created the note.")])),
        response("model-a-snapshot", vec![message("Note planning")]).to_string(),
        stream(response("model-b-snapshot", vec![message("Second reply")])),
    ]);
    let settings = Settings {
        api_key: "test-key".into(),
        model: "model-a".into(),
        system_prompt_note: Some(Id::generate()),
    };
    agent
        .run_client(
            &host,
            settings.clone(),
            Some((
                "Make a note".into(),
                Some(Context { id: Id::generate(), title: "Open note".into() }),
            )),
            &client,
        )
        .await
        .unwrap();
    let session = agent.snapshot();
    assert_eq!(session.title, "Note planning");
    assert_eq!(session.turns[0].reported_models, ["model-a-snapshot"]);
    assert_eq!(session.turns[0].model, "model-a");
    assert_eq!(host.creates.load(Ordering::SeqCst), 1);
    *host.prompt.lock().unwrap() = "Changed instructions".into();
    agent.rename("My manual title").unwrap();
    agent
        .run_client(
            &host,
            Settings { model: "model-b".into(), ..settings },
            Some(("Another question".into(), None)),
            &client,
        )
        .await
        .unwrap();
    let requests = server.join().unwrap();
    assert_eq!(requests[0]["model"], "model-a");
    assert_eq!(requests[1]["model"], "model-a");
    assert_eq!(requests[3]["model"], "model-b");
    assert_eq!(requests[0]["instructions"], requests[3]["instructions"]);
    assert_eq!(host.prompt_reads.load(Ordering::SeqCst), 1);
    let replay = requests[1]["input"].as_array().unwrap();
    assert!(replay.iter().any(|item| item["encrypted_content"] == "opaque"));
    assert!(replay.iter().any(|item| item["type"] == "function_call_output"));
    assert_eq!(Agent::open(root.path()).unwrap().snapshot().title, "My manual title");
    let transcript = agent.snapshot().transcript();
    assert!(transcript.starts_with("# My manual title\n"));
    assert!(transcript.contains("model-a-snapshot"));
    assert!(transcript.contains("model-b-snapshot"));
    assert!(transcript.contains("Created [["));
}

#[tokio::test]
async fn web_search_is_always_available_and_sends_only_the_public_query() {
    let root = tempfile::tempdir().unwrap();
    let agent = Agent::open(root.path()).unwrap();
    agent.rename("Research").unwrap();
    let host = TestHost::new();
    let id = Id::generate();
    host.notes.lock().unwrap().insert(id, json!({"id": id, "body": "Private note text"}));
    let query = "Public release date";
    let url = "https://example.com/release?channel=stable&year=2026";
    let mut cited = message("The release was in September.");
    cited["content"][0]["annotations"] = json!([
        {"type": "url_citation", "title": "Release [notes]", "url": url},
        {"type": "url_citation", "title": "Duplicate source", "url": url},
        {"type": "url_citation", "title": "Local file", "url": "file:///private.txt"}
    ]);
    let (client, server) = server(vec![
        stream(response("chat-model", vec![function_call("get_note", "read", json!({"id": id}))])),
        stream(response("chat-model", vec![message("Note summary")])),
        stream(response(
            "chat-model",
            vec![function_call("search_web", "web", json!({"query": query}))],
        )),
        response(
            "research-model",
            vec![json!({"type": "web_search_call", "status": "completed"}), cited],
        )
        .to_string(),
        stream(response("chat-model", vec![message(&format!("[Release notes]({url})"))])),
    ]);
    let settings = Settings {
        api_key: "test-key".into(),
        model: "chosen-model".into(),
        system_prompt_note: Some(Id::generate()),
    };
    let context = Context { id, title: "Open note".into() };
    agent
        .run_client(
            &host,
            settings.clone(),
            Some(("Private chat question".into(), Some(context))),
            &client,
        )
        .await
        .unwrap();
    agent.run_client(&host, settings, Some((query.into(), None)), &client).await.unwrap();
    let requests = server.join().unwrap();
    for request in [&requests[0], &requests[1], &requests[2], &requests[4]] {
        assert!(
            request["tools"].as_array().unwrap().iter().any(|tool| tool["name"] == "search_web")
        );
    }
    assert_eq!(requests[2]["instructions"], format!("{INSTRUCTIONS}\n\nOriginal instructions"));
    assert!(requests[2].to_string().contains("Private note text"));
    let research = &requests[3];
    assert_eq!(research["input"], query);
    assert_eq!(research["model"], "chosen-model");
    assert_eq!(research["tool_choice"], json!({"type": "web_search"}));
    assert_eq!(research["tools"], json!([{"type": "web_search"}]));
    assert_eq!(research["store"], false);
    for private in ["Private note text", "Private chat question", "Original instructions"] {
        assert!(!research.to_string().contains(private));
    }
    assert!(requests[4].to_string().contains("The release was in September."));
    let session = Agent::open(root.path()).unwrap().snapshot();
    let turn = &session.turns[1];
    assert_eq!(turn.reported_models, ["chat-model", "research-model"]);
    assert_eq!(
        turn.tools[0].result.as_ref().unwrap()["sources"],
        json!([
            {"title": "Release [notes]", "url": url}
        ])
    );
    let links = turn.tools[0].web_source_links();
    assert!(Parser::new(&links).any(|event| {
        matches!(event, Event::Start(Tag::Link { dest_url, .. }) if &*dest_url == url)
    }));
    assert!(session.transcript().contains(&links));
}

#[tokio::test]
async fn existing_note_writes_require_known_content_and_run_through_the_tool_loop() {
    let root = tempfile::tempdir().unwrap();
    let agent = Agent::open(root.path()).unwrap();
    agent.rename("Editing notes").unwrap();
    let host = TestHost::new();
    let id = Id::generate();
    host.create_note(
        Creation { id, space: Some(host.destination) },
        "note".into(),
        "Original text".into(),
    )
    .await
    .unwrap();
    let call = |name: &str, call_id: &str, arguments: Value| {
        json!({
            "type": "function_call", "name": name, "call_id": call_id,
            "arguments": arguments.to_string()
        })
    };
    let write = || call("write_note", "write", json!({"body": "Updated text", "id": id}));
    let (client, server) = server(vec![
        stream(response("model-a", vec![write()])),
        stream(response("model-a", vec![message("I need to read it first.")])),
        stream(response("model-a", vec![call("get_note", "read", json!({"id": id}))])),
        stream(response("model-a", vec![write()])),
        stream(response("model-a", vec![message("Updated the note.")])),
    ]);
    let settings =
        Settings { api_key: "test-key".into(), model: "model-a".into(), ..Settings::default() };
    agent
        .run_client(&host, settings.clone(), Some(("Update it".into(), None)), &client)
        .await
        .unwrap();
    assert_eq!(host.get_note(id).await.unwrap()["body"], "Original text");
    assert!(
        agent.snapshot().turns[0].tools[0].result.as_ref().unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("get_note")
    );
    agent
        .run_client(&host, settings, Some(("Read and update it".into(), None)), &client)
        .await
        .unwrap();
    assert_eq!(host.get_note(id).await.unwrap()["body"], "Updated text");
    assert!(agent.snapshot().transcript().contains(&format!("Updated [[{id}]]")));
    assert_eq!(server.join().unwrap().len(), 5);
}

#[tokio::test]
async fn batch_note_reads_keep_order_and_supply_bodies_for_writes() {
    let root = tempfile::tempdir().unwrap();
    let agent = Agent::open(root.path()).unwrap();
    agent.rename("Editing notes").unwrap();
    let host = TestHost::new();
    let first = Id::generate();
    let missing = Id::generate();
    let second = Id::generate();
    let first_note = json!({"body": "First body", "id": first, "title": "First"});
    let second_note = json!({"body": "Second body", "id": second, "title": "Second"});
    host.notes.lock().unwrap().extend([(first, first_note.clone()), (second, second_note.clone())]);
    let (client, server) = server(vec![
        stream(response(
            "model",
            vec![function_call(
                "get_notes",
                "batch",
                json!({"ids": [first, missing, second, "invalid", first]}),
            )],
        )),
        stream(response(
            "model",
            vec![function_call("write_note", "write", json!({"body": "Updated", "id": second}))],
        )),
        stream(response("model", vec![message("Updated the second note.")])),
    ]);
    let settings =
        Settings { api_key: "test-key".into(), model: "model".into(), ..Settings::default() };
    agent
        .run_client(
            &host,
            settings,
            Some(("Read these notes and update the second".into(), None)),
            &client,
        )
        .await
        .unwrap();
    let requests = server.join().unwrap();
    let batch = requests[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output" && item["call_id"] == "batch")
        .unwrap();
    let entries: Vec<Value> = serde_json::from_str(batch["output"].as_str().unwrap()).unwrap();
    assert_eq!(entries.len(), 5);
    assert_eq!(entries[0], first_note);
    assert_eq!(entries[1]["id"], missing.to_string());
    assert!(entries[1]["error"].is_string());
    assert_eq!(entries[2], second_note);
    assert_eq!(entries[3]["id"], "invalid");
    assert!(entries[3]["error"].is_string());
    assert_eq!(entries[4], first_note);
    assert_eq!(host.get_note(second).await.unwrap()["body"], "Updated");
}

#[tokio::test]
async fn restart_and_retry_finish_a_reserved_creation_without_duplicates() {
    let root = tempfile::tempdir().unwrap();
    let agent = Agent::open(root.path()).unwrap();
    let host = TestHost::new();
    let creation = Creation { id: Id::generate(), space: Some(host.destination) };
    host.create_note(creation.clone(), "note".into(), "Original note".into()).await.unwrap();
    agent
        .update(|s| {
            s.manual_title = true;
            s.turns.push(Turn {
                context: None,
                error: None,
                message: "Make a note".into(),
                model: "original-model".into(),
                pending_text: String::new(),
                reply: String::new(),
                reported_models: Vec::new(),
                status: Status::Running,
                items: vec![json!({"type": "function_call", "name": "create_note",
                "call_id": "pending", "arguments": "{\"body\":\"Original note\"}"})],
                tools: vec![Tool {
                    arguments: json!({"body": "Original note"}),
                    call_id: "pending".into(),
                    creation: Some(creation),
                    name: "create_note".into(),
                    result: None,
                }],
            });
        })
        .unwrap();
    let agent = Agent::open(root.path()).unwrap();
    assert!(agent.snapshot().can_retry());
    let (client, server) = server(vec![stream(response("original-model", vec![message("Done")]))]);
    agent
        .run_client(
            &host,
            Settings {
                api_key: "test-key".into(),
                model: "different-selection".into(),
                ..Settings::default()
            },
            None,
            &client,
        )
        .await
        .unwrap();
    assert_eq!(server.join().unwrap()[0]["model"], "original-model");
    assert_eq!(host.creates.load(Ordering::SeqCst), 1);
    assert_eq!(agent.snapshot().turns[0].status, Status::Complete);
}

#[tokio::test]
async fn pending_archive_reuses_its_destination_and_note_then_starts_a_new_session() {
    let root = tempfile::tempdir().unwrap();
    let agent = Agent::open(root.path()).unwrap();
    let host = TestHost::new();
    agent.set_draft("A draft worth keeping".into()).unwrap();
    agent.rename("Saved discussion").unwrap();
    let original = agent.snapshot().id;
    let creation = Creation { id: Id::generate(), space: Some(host.destination) };
    agent.update(|s| s.archive = Some(creation.clone())).unwrap();
    host.create_note(creation.clone(), "agent-transcript".into(), agent.snapshot().transcript())
        .await
        .unwrap();
    let agent = Agent::open(root.path()).unwrap();
    agent.archive(&host, Id::generate()).await.unwrap();
    assert_eq!(host.creates.load(Ordering::SeqCst), 1);
    assert_ne!(agent.snapshot().id, original);
    assert!(agent.snapshot().is_empty());
    let notes = host.notes.lock().unwrap();
    assert_eq!(notes[&creation.id]["space"], host.destination.to_string());
    assert!(notes[&creation.id]["body"].as_str().unwrap().contains("A draft worth keeping"));
}

#[tokio::test]
async fn stopped_stream_keeps_partial_reply_in_replay_and_never_runs_partial_tools() {
    let root = tempfile::tempdir().unwrap();
    let agent = Agent::open(root.path()).unwrap();
    let host = TestHost::new();
    agent.rename("Interrupted discussion").unwrap();
    let delta = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Partial café\"}\n\n";
    let (client, server) = server(vec![delta.into()]);
    assert!(
        agent
            .run_client(
                &host,
                Settings {
                    api_key: "test-key".into(),
                    model: "model-a".into(),
                    ..Settings::default()
                },
                Some(("Question".into(), None)),
                &client
            )
            .await
            .is_err()
    );
    server.join().unwrap();
    let reopened = Agent::open(root.path()).unwrap();
    assert_eq!(reopened.snapshot().turns[0].reply, "Partial café");
    assert_eq!(reopened.snapshot().input().last().unwrap()["content"], "Partial café");
    assert_eq!(host.creates.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stop_cancels_a_destination_request_before_creating_a_note() {
    let root = tempfile::tempdir().unwrap();
    let agent = Agent::open(root.path()).unwrap();
    let host = TestHost { wait_for_destination: true, ..TestHost::new() };
    let call = json!({"type": "function_call", "name": "create_note", "call_id": "waiting",
        "arguments": "{\"body\":\"Never created\"}"});
    let (client, server) = server(vec![stream(response("model-a", vec![call]))]);
    let run = agent.run_client(
        &host,
        Settings { api_key: "test-key".into(), model: "model-a".into(), ..Settings::default() },
        Some(("Make a note".into(), None)),
        &client,
    );
    let stop = async {
        tokio::time::timeout(Duration::from_secs(10), async {
            while agent.snapshot().turns.last().is_none_or(|t| t.tools.is_empty()) {
                tokio::task::yield_now().await;
            }
            agent.stop();
        })
        .await
        .unwrap();
    };
    let (result, ()) = tokio::join!(run, stop);
    assert!(result.is_err());
    server.join().unwrap();
    assert_eq!(host.creates.load(Ordering::SeqCst), 0);
    assert_eq!(agent.snapshot().turns[0].status, Status::Stopped);
    assert!(!agent.is_running());
}
