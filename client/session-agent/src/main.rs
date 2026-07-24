#![forbid(unsafe_code)]

use std::{env, ffi::OsStr, process::ExitCode, thread};

fn run() -> Result<(), &'static str> {
    let mut args = env::args_os().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some(mode), None) if mode == OsStr::new("--autostart") => {
            // Phase 0 resident process contract: desktop XDG Autostart owns
            // launch; the process begins hidden and does not create a window.
            // P0.7/Probe E1 still owes logind validation, Daemon lease renewal
            // and the minimal lazy Slint slice; Phase 6 owns the full GUI state machines.
            loop {
                thread::park();
            }
        }
        _ => Err("usage: natsume-session-agent --autostart"),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}
