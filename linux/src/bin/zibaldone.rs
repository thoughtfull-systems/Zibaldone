//! The instance's executable, which the `zib` command starts when no instance runs.

use std::process::ExitCode;

use zib_core::Error;
use zib_core::channel::Dirs;
use zib_core::instance::Lock;
use zib_linux::{CHANNEL, instance};

fn main() -> ExitCode {
    let name = CHANNEL.instance();
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
    match runtime.block_on(instance::run(CHANNEL, dirs, lock)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{name}: {error}");
            ExitCode::FAILURE
        }
    }
}
