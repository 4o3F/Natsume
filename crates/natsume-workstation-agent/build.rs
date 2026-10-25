fn main() {
    println!("cargo:rerun-if-changed=ui/session_agent.slint");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS")
        .unwrap_or_else(|error| panic!("Cargo did not provide a valid target OS: {error}"));
    if target_os != "linux" {
        println!(
            "cargo:warning=Skipping Linux Session Agent UI generation for target OS {target_os}; only the unsupported-platform entry point is compiled"
        );
        return;
    }

    if let Err(error) = slint_build::compile("ui/session_agent.slint") {
        panic!("failed to compile Session Agent Slint UI: {error}");
    }
}
