//! The `zib` command, which sends each command to the running instance and starts it first if
//! none runs. With no command, it opens the instance's window.

use std::env;
use std::process::{Command, ExitCode, Stdio};

use zib_core::Id;
use zib_core::channel::Dirs;
use zib_core::instance::{Client, Reply, Request};
use zib_core::search::{MATCH_END, MATCH_START};
use zib_linux::CHANNEL;

const USAGE: &str = "\
Usage:
  zib                     Open the window, starting Zib if it isn't running
  zib zib:note/<id>       Open a note
  zib spaces              List the spaces on this machine
  zib create-space NAME   Create a space
  zib search WORDS...     Search every space
  zib print [--linked] -o FILE NOTE...
                          Print notes to a PDF, with --linked adding what they link to
  zib stop                Quit Zib";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = CHANNEL.command();
    if matches!(args.first().map(String::as_str), Some("-h" | "--help" | "help")) {
        println!("{}", USAGE.replace("zib ", &format!("{command} ")));
        return ExitCode::SUCCESS;
    }
    let runtime =
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    match runtime.block_on(run(args)) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{command}: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Vec<String>) -> Result<ExitCode, String> {
    let dirs = Dirs::for_channel(CHANNEL).map_err(|e| e.to_string())?;
    let mut client = Client::connect(&dirs, start).await.map_err(|e| e.to_string())?;
    let id = client.next_id();
    let request = match args.first().map(String::as_str) {
        None => Request::Activate { id, uri: None },
        Some(uri) if uri.to_ascii_lowercase().starts_with("zib:") && args.len() == 1 => {
            Request::Activate { id, uri: Some(uri.to_owned()) }
        }
        Some("spaces") => Request::Spaces { id },
        Some("create-space") if args.len() == 2 => {
            Request::CreateSpace { id, name: args[1].clone() }
        }
        Some("search") if args.len() > 1 => {
            Request::Search { id, query: args[1..].join(" "), limit: None }
        }
        Some("print") => print_request(id, &args[1..])?,
        Some("stop") => Request::Stop { id },
        Some(other) => return Err(format!("unknown command {other:?}\n{USAGE}")),
    };
    match client.request(request).await.map_err(|e| e.to_string())? {
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
            note => {
                let text = note.strip_prefix("zib:note/").unwrap_or(note);
                let text = text.split('#').next().unwrap_or(text);
                let note = Id::parse(text).map_err(|_| format!("{note:?} doesn't name a note"))?;
                notes.push(note);
            }
        }
    }
    let path = path.ok_or_else(|| format!("print needs -o FILE\n{USAGE}"))?;
    if notes.is_empty() {
        return Err(format!("print needs at least one note\n{USAGE}"));
    }
    let path = env::current_dir().map_err(|e| e.to_string())?.join(path);
    Ok(Request::Print { id, notes, linked, path: path.to_string_lossy().into_owned() })
}
