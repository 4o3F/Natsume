use std::env;

fn main() {
    match env::var("CARGO_FEATURE_SERVER") {
        Ok(_) => {
            use std::process::Command;
            let status = Command::new("pnpm")
                .arg("install")
                .current_dir("panel")
                .status()
                .expect("failed to install frontend dependency");

            if !status.success() {
                panic!("Vue frontend prepare failed");
            }

            let status = Command::new("pnpm")
                .arg("build")
                .current_dir("panel")
                .status()
                .expect("failed to build frontend");

            if !status.success() {
                panic!("Vue frontend build failed");
            }
        }
        Err(env::VarError::NotPresent) => {}
        Err(other) => {
            panic!("Failed to parse feature flag, err {}", other)
        }
    }
    println!("cargo:rerun-if-changed=migrations");
}
