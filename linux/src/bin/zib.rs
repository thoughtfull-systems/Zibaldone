//! The `zib` command, which sends each command to the running instance and starts it first if
//! none runs. With no command, it opens the instance's window.

use std::env;
use std::io::Read;
use std::process::{Command, ExitCode, Stdio};

use zib_core::Id;
use zib_core::channel::Dirs;
use zib_core::instance::{Client, NoteRead, Reply, Request};
use zib_core::search::{MATCH_END, MATCH_START};
use zib_linux::CHANNEL;

#[path = "../firefox_host.rs"]
mod firefox_host;

const USAGE: &str = "\
Usage:
  zib                     Open the window, starting Zib if it isn't running
  zib zib:note/<id>       Open a note
  zib spaces              List the spaces on this machine
  zib create-space NAME   Create a space
  zib search [--json] [--space SPACE] WORDS...
                          Search all spaces or the given space name or ID
  zib get NOTE...         Read note bodies as a JSON array
  zib get --raw NOTE      Print one note body as markdown
  zib write NOTE --expect ORIGINAL < REVISED
                          Replace a note body if it still matches ORIGINAL
  zib print [--linked] -o FILE NOTE...
                          Print notes to a PDF, with --linked adding what they link to
  zib stop                Quit Zib";

enum Output {
    Default,
    HitsJson,
    NotesJson,
    RawNote,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = CHANNEL.command();
    if matches!(args.first().map(String::as_str), Some("-h" | "--help" | "help")) {
        println!("{}", USAGE.replace("zib ", &format!("{command} ")));
        return ExitCode::SUCCESS;
    }
    let runtime =
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    let result = if args.first().is_some_and(|arg| arg == "firefox-host") {
        runtime.block_on(firefox_host::run(start)).map(|()| ExitCode::SUCCESS)
    } else {
        runtime.block_on(run(args))
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{command}: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Vec<String>) -> Result<ExitCode, String> {
    let id = 1; // The command sends a single request on its connection.
    let mut output = Output::Default;
    let request = match args.first().map(String::as_str) {
        None => Request::Activate { id, uri: None },
        Some(uri) if uri.to_ascii_lowercase().starts_with("zib:") && args.len() == 1 => {
            Request::Activate { id, uri: Some(uri.to_owned()) }
        }
        Some("spaces") => Request::Spaces { id },
        Some("create-space") if args.len() == 2 => {
            Request::CreateSpace { id, name: args[1].clone() }
        }
        Some("search") => {
            let (request, mode) = search_request(id, &args[1..])?;
            output = mode;
            request
        }
        Some("get") => {
            let mut args = &args[1..];
            output = Output::NotesJson;
            if args.first().is_some_and(|flag| flag == "--raw") {
                args = &args[1..];
                if args.len() != 1 {
                    return Err("get --raw needs exactly one note".into());
                }
                output = Output::RawNote;
            }
            if args.is_empty() {
                return Err("get needs at least one note".into());
            }
            Request::GetNotes {
                id,
                notes: args.iter().map(|arg| note_id(arg)).collect::<Result<_, _>>()?,
            }
        }
        Some("print") => print_request(id, &args[1..])?,
        Some("stop") => Request::Stop { id },
        Some("write") => write_request(id, &args[1..])?,
        Some(other) => return Err(format!("unknown command {other:?}\n{USAGE}")),
    };
    let dirs = Dirs::for_channel(CHANNEL).map_err(|e| e.to_string())?;
    let mut client = Client::connect(&dirs, start).await.map_err(|e| e.to_string())?;
    match client.request(request).await.map_err(|e| e.to_string())? {
        Reply::Result { mut hits, .. } if matches!(output, Output::HitsJson) => {
            for hit in &mut hits {
                hit.snippet = hit.snippet.replace([MATCH_START, MATCH_END], "");
            }
            println!("{}", serde_json::to_string_pretty(&hits).map_err(|e| e.to_string())?);
            Ok(ExitCode::SUCCESS)
        }
        Reply::Result { notes, .. } if matches!(output, Output::NotesJson) => {
            println!("{}", serde_json::to_string_pretty(&notes).map_err(|e| e.to_string())?);
            Ok(ExitCode::SUCCESS)
        }
        Reply::Result { notes, .. } if matches!(output, Output::RawNote) => {
            match notes.as_slice() {
                [NoteRead::Found { body, .. }] => {
                    print!("{body}");
                    Ok(ExitCode::SUCCESS)
                }
                [NoteRead::Unavailable { error, .. }] => Err(error.clone()),
                _ => Err("expected a single note in the reply".into()),
            }
        }
        Reply::Result { spaces, hits, .. } => {
            for space in spaces {
                println!("{}\t{}", space.id, space.name);
            }
            for hit in hits {
                let snippet =
                    hit.snippet.replace(MATCH_START, "\u{1b}[1m").replace(MATCH_END, "\u{1b}[0m");
                println!(
                    "zib:note/{}\t{}\n    {}",
                    hit.note,
                    hit.title,
                    snippet.replace('\n', " ")
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Reply::Error { reason, .. } => Err(reason),
        Reply::Hello { .. } => Err("unexpected reply".into()),
    }
}

fn search_request(id: u64, mut args: &[String]) -> Result<(Request, Output), String> {
    let mut space = None;
    let mut output = Output::Default;
    while let [flag, rest @ ..] = args {
        match flag.as_str() {
            "--" => {
                args = rest;
                break;
            }
            "--json" => {
                output = Output::HitsJson;
                args = rest;
            }
            "--space" => {
                let Some((name, rest)) = rest.split_first() else {
                    return Err("search --space needs a space name or ID".into());
                };
                space = Some(name.clone());
                args = rest;
            }
            _ => break,
        }
    }
    let query = args.join(" ");
    if query.trim().is_empty() {
        return Err("search needs at least one search term".into());
    }
    let request = if let Some(space) = space {
        Request::SearchSpace { id, query, limit: None, space }
    } else {
        Request::Search { id, query, limit: None }
    };
    Ok((request, output))
}

fn note_id(note: &str) -> Result<Id, String> {
    let text = note.strip_prefix("zib:note/").unwrap_or(note);
    let text = text.split('#').next().unwrap_or(text);
    Id::parse(text).map_err(|_| format!("{note:?} doesn't name a note"))
}

fn write_request(id: u64, args: &[String]) -> Result<Request, String> {
    let [note, flag, original] = args else {
        return Err("write needs NOTE --expect ORIGINAL and the revised body on stdin".into());
    };
    if flag != "--expect" {
        return Err("write needs --expect ORIGINAL".into());
    }
    let note = note_id(note)?;
    let expected = std::fs::read_to_string(original).map_err(|e| format!("{original}: {e}"))?;
    let mut body = String::new();
    std::io::stdin().read_to_string(&mut body).map_err(|e| e.to_string())?;
    Ok(Request::WriteNote { id, note, body, expected })
}

/// Starts the instance from the executable beside this one, detached from the terminal. Cargo names
/// it `zibaldone` in every channel, and the Nix packages name it after the channel.
fn start() -> std::io::Result<()> {
    let exe = env::current_exe()?;
    let named = exe.with_file_name(CHANNEL.instance());
    let instance = if named.exists() { named } else { exe.with_file_name("zibaldone") };
    Command::new(instance)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

/// Reads `print`'s arguments: notes as identifiers or `zib:note/` URIs, `--linked`, and `-o` with
/// the file to write, which the instance needs as an absolute path.
fn print_request(id: u64, args: &[String]) -> Result<Request, String> {
    let mut notes = Vec::new();
    let mut linked = false;
    let mut path = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--linked" => linked = true,
            "-o" | "--output" => path = args.next().cloned(),
            note => notes.push(note_id(note)?),
        }
    }
    let path = path.ok_or_else(|| format!("print needs -o FILE\n{USAGE}"))?;
    if notes.is_empty() {
        return Err(format!("print needs at least one note\n{USAGE}"));
    }
    let path = env::current_dir().map_err(|e| e.to_string())?.join(path);
    Ok(Request::Print { id, notes, linked, path: path.to_string_lossy().into_owned() })
}
