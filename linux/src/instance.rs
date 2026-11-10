//! The instance's connections to the `zib` command and the tray, as the instance specification
//! defines them, and running without a display.

use std::sync::Arc;

use zib_core::instance::{self, Handler, HitInfo, Lock, NoteRead, Reply, Request, SpaceInfo};
use zib_core::{Channel, Error, Id, Result};

use crate::state::{State, UiEvent};
use crate::tray;

/// Answers the `zib` command and shows the tray icon. `lock` proves this process is the instance.
pub async fn serve(state: Arc<State>, lock: &Lock) -> Result<()> {
    let listener = instance::listen(&state.dirs, lock)?;
    tokio::spawn(instance::serve(listener, handler(state.clone())));
    match tray::show(state.channel).await {
        Ok(mut events) => {
            tokio::spawn(async move {
                while let Some(event) = events.recv().await {
                    match event {
                        tray::Event::Activate => activate(&state, None),
                        tray::Event::Quit => quit(&state),
                    }
                }
            });
        }
        Err(error) => eprintln!("{}: no tray icon: {error}", state.channel.instance()),
    }
    Ok(())
}

/// Runs without a window until `stop` or a signal, for a machine without a display.
pub async fn run_headless(state: Arc<State>) {
    tokio::select! {
        () = state.quit.notified() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    state.finish().await;
}

fn activate(state: &State, uri: Option<String>) {
    state.send_ui(UiEvent::Activate(uri));
}

fn quit(state: &State) {
    state.send_ui(UiEvent::Quit);
    state.quit.notify_one();
}

fn handler(state: Arc<State>) -> Handler {
    Arc::new(move |request| {
        let state = state.clone();
        Box::pin(async move {
            let id = request.id();
            match answer(&state, request).await {
                Ok(reply) => reply,
                Err(error) => {
                    let code = match error {
                        Error::NotFound(_) => "not-found",
                        Error::NoteChanged(_) => "conflict",
                        _ => "failed",
                    };
                    Reply::Error { id, reason: error.to_string(), code: code.into() }
                }
            }
        })
    })
}

async fn answer(state: &State, request: Request) -> Result<Reply> {
    Ok(match request {
        Request::Hello { .. } => Reply::Hello { features: Vec::new() },
        Request::Activate { id, uri } => {
            activate(state, uri);
            Reply::done(id)
        }
        Request::Stop { id } => {
            quit(state);
            Reply::done(id)
        }
        Request::Spaces { id } => {
            let spaces =
                state.spaces().await.into_iter().map(|(id, name)| SpaceInfo { id, name }).collect();
            Reply::Result { id, clip: None, spaces, hits: Vec::new(), notes: Vec::new() }
        }
        Request::CreateSpace { id, name } => {
            let space = state.create_space(&name).await?;
            Reply::Result {
                id,
                clip: None,
                spaces: vec![SpaceInfo { id: space, name }],
                hits: Vec::new(),
                notes: Vec::new(),
            }
        }
        Request::Search { id, query, limit } => search_reply(state, id, &query, limit, None),
        Request::SearchSpace { id, query, limit, space } => {
            let space = resolve_space(state, &space).await?;
            search_reply(state, id, &query, limit, Some(space))
        }
        Request::GetNotes { id, notes } => {
            let mut entries = Vec::with_capacity(notes.len());
            for note in notes {
                entries.push(state.read_note(note).await.unwrap_or_else(|error| {
                    NoteRead::Unavailable { error: error.to_string(), note }
                }));
            }
            Reply::Result { id, clip: None, spaces: Vec::new(), hits: Vec::new(), notes: entries }
        }
        Request::WriteNote { id, note, body, expected } => {
            state.replace_note_body(note, &expected, &body, false).await?;
            Reply::done(id)
        }
        Request::ImportWebClip { id, clip, images } => Reply::Result {
            id,
            clip: Some(state.import_web_clip(clip, images).await?),
            spaces: Vec::new(),
            hits: Vec::new(),
            notes: Vec::new(),
        },
        Request::ImportWebPdf { id, pdf } => Reply::Result {
            id,
            clip: Some(state.import_web_pdf(pdf).await?),
            spaces: Vec::new(),
            hits: Vec::new(),
            notes: Vec::new(),
        },
        Request::Print { id, notes, linked, path } => {
            let pdf = state.print(&notes, linked).await?;
            std::fs::write(&path, pdf).map_err(|e| Error::io(path, e))?;
            Reply::done(id)
        }
    })
}

fn search_reply(
    state: &State,
    id: u64,
    query: &str,
    limit: Option<u32>,
    space: Option<Id>,
) -> Reply {
    let hits = state
        .search_in(query, limit.unwrap_or(20) as usize, space)
        .into_iter()
        .map(|(space, hit)| HitInfo { space, note: hit.id, title: hit.title, snippet: hit.snippet })
        .collect();
    Reply::Result { id, clip: None, spaces: Vec::new(), hits, notes: Vec::new() }
}

async fn resolve_space(state: &State, name: &str) -> Result<Id> {
    let spaces = state.spaces().await;
    if let Ok(id) = Id::parse(name)
        && spaces.iter().any(|(space, _)| *space == id)
    {
        return Ok(id);
    }
    let mut named = spaces.into_iter().filter(|(_, candidate)| candidate == name);
    let (id, _) = named.next().ok_or_else(|| Error::NotFound(format!("space {name:?}")))?;
    if named.next().is_some() {
        return Err(Error::Protocol(format!("more than one space is named {name:?}. Use its ID")));
    }
    Ok(id)
}

/// Whether a display is available for the window.
pub fn has_display() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}

/// The name the instance's messages start with.
pub fn name(channel: Channel) -> &'static str {
    channel.instance()
}

#[cfg(test)]
mod tests {
    use zib_core::channel::Dirs;
    use zib_core::instance::Client;

    use super::*;

    #[tokio::test]
    async fn note_commands_filter_search_read_batches_and_check_edits() {
        let root = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(root.path());
        let state = State::for_test(dirs.clone()).await.unwrap();
        let first_space = state.spaces().await[0].0;
        let second_space = state.create_space("Research notes").await.unwrap();
        let first = state.create_note(first_space, "# First\n\nSharedterm").await.unwrap();
        let original = "# Second\n\nSharedterm";
        let second = state.create_note(second_space, original).await.unwrap();
        let missing = Id::generate();
        let lock = Lock::take(&dirs).unwrap();
        let listener = instance::listen(&dirs, &lock).unwrap();
        let server = tokio::spawn(instance::serve(listener, handler(state.clone())));
        let mut client =
            Client::connect(&dirs, || panic!("instance already started")).await.unwrap();

        let id = client.next_id();
        let request = Request::Search { id, query: "Sharedterm".into(), limit: None };
        let Reply::Result { hits, .. } = client.request(request).await.unwrap() else {
            panic!("expected search hits")
        };
        assert_eq!(hits.len(), 2);
        for space in ["Research notes".into(), second_space.to_string()] {
            let id = client.next_id();
            let request =
                Request::SearchSpace { id, query: "Sharedterm".into(), limit: None, space };
            let Reply::Result { hits, .. } = client.request(request).await.unwrap() else {
                panic!("expected scoped search hits")
            };
            assert_eq!(hits.iter().map(|hit| hit.note).collect::<Vec<_>>(), [second]);
        }

        let id = client.next_id();
        let request = Request::GetNotes { id, notes: vec![second, missing, first] };
        let Reply::Result { notes, .. } = client.request(request).await.unwrap() else {
            panic!("expected note entries")
        };
        assert!(matches!(&notes[0], NoteRead::Found { body, note, .. }
            if *note == second && body == original));
        assert!(matches!(&notes[1], NoteRead::Unavailable { note, .. } if *note == missing));
        assert!(matches!(&notes[2], NoteRead::Found { note, .. } if *note == first));

        for body in ["# Updated\n\nNewterm", "# Updated\n\nNewterm", "Stale replacement"] {
            let id = client.next_id();
            let request = Request::WriteNote {
                id,
                note: second,
                body: body.into(),
                expected: original.into(),
            };
            let reply = client.request(request).await.unwrap();
            if body == "Stale replacement" {
                assert!(matches!(reply, Reply::Error { code, .. } if code == "conflict"));
            } else {
                assert_eq!(reply, Reply::done(id));
            }
        }
        let updated = state.note(second_space, second).await.unwrap();
        assert_eq!(updated.body, "# Updated\n\nNewterm");
        assert_eq!(updated.type_name, "note");
        assert_eq!(state.search("Newterm", 20)[0].1.id, second);
        state.finish().await;
        server.abort();
    }
}
