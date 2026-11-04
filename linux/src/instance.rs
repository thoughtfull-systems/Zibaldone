//! The instance's connections to the `zib` command and the tray, as the instance specification
//! defines them, and running without a display.

use std::sync::Arc;

use zib_core::instance::{self, Handler, HitInfo, Lock, Reply, Request, SpaceInfo};
use zib_core::{Channel, Error, Result};

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
                    let code =
                        if matches!(error, Error::NotFound(_)) { "not-found" } else { "failed" };
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
            Reply::Result { id, spaces, hits: Vec::new() }
        }
        Request::CreateSpace { id, name } => {
            let space = state.create_space(&name).await?;
            Reply::Result { id, spaces: vec![SpaceInfo { id: space, name }], hits: Vec::new() }
        }
        Request::Search { id, query, limit } => {
            let hits = state
                .search(&query, limit.unwrap_or(20) as usize)
                .into_iter()
                .map(|(space, hit)| HitInfo {
                    space,
                    note: hit.id,
                    title: hit.title,
                    snippet: hit.snippet,
                })
                .collect();
            Reply::Result { id, spaces: Vec::new(), hits }
        }
        Request::Print { id, notes, linked, path } => {
            let pdf = state.print(&notes, linked).await?;
            std::fs::write(&path, pdf).map_err(|e| Error::io(path, e))?;
            Reply::done(id)
        }
    })
}

/// Whether a display is available for the window.
pub fn has_display() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}

/// The name the instance's messages start with.
pub fn name(channel: Channel) -> &'static str {
    channel.instance()
}
