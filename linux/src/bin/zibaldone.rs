//! The instance's executable, which the `zib` command starts when no instance runs. It starts with
//! only its tray icon, and opens its window when asked.

use std::process::ExitCode;

use zib_core::Error;
use zib_core::channel::Dirs;
use zib_core::instance::Lock;
use zib_linux::state::State;
use zib_linux::{CHANNEL, app, instance};

fn main() -> ExitCode {
    let name = instance::name(CHANNEL);
    let dirs = match Dirs::for_channel(CHANNEL) {
        Ok(dirs) => dirs,
        Err(error) => {
            eprintln!("{name}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let lock = match Lock::take(&dirs) {
        Ok(lock) => lock,
        // An instance already runs, so this process has nothing to do.
        Err(Error::Locked(_)) => return ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{name}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let runtime =
        tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a runtime");
    let started = runtime.block_on(async {
        let state = State::start(CHANNEL, dirs).await?;
        instance::serve(state.clone(), &lock).await?;
        Ok::<_, Error>(state)
    });
    let state = match started {
        Ok(state) => state,
        Err(error) => {
            eprintln!("{name}: {error}");
            return ExitCode::FAILURE;
        }
    };
    if instance::has_display() {
        app::run(state.clone(), runtime.handle().clone());
        runtime.block_on(state.finish());
    } else {
        runtime.block_on(instance::run_headless(state));
    }
    drop(lock);
    ExitCode::SUCCESS
}
