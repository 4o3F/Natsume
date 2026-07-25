use std::path::Path;

#[test]
fn repository_smoke_finds_authoritative_phase0_inputs() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let Some(repository_root) = manifest_dir.parent() else {
        panic!("integration-tests must be located below the repository root");
    };

    for relative_path in [
        "Cargo.lock",
        "pnpm-lock.yaml",
        "docs/README.md",
        "docs/requirements/phase-0.md",
        "docs/gates/g0-checklist.md",
    ] {
        assert!(
            repository_root.join(relative_path).is_file(),
            "required Phase 0 input is missing: {relative_path}"
        );
    }
}
