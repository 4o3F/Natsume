use std::{env, process::Command};

fn run_pnpm(args: &[&str], dir: &str) {
    let pnpm = if cfg!(windows) { "pnpm.cmd" } else { "pnpm" };

    let status = Command::new(pnpm)
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|e| panic!("failed to run {} {:?}: {}", pnpm, args, e));

    if !status.success() {
        panic!("pnpm {:?} failed with {}", args, status);
    }
}

fn main() {
    if env::var_os("CARGO_FEATURE_SERVER").is_some() {
        run_pnpm(&["install"], "panel");
        run_pnpm(&["build"], "panel");
    }

    println!("cargo:rerun-if-changed=migrations");
}
