use std::os::unix::fs::symlink;

use super::*;

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("fixture: {e}"));
    write(
        dir.path(),
        HOSTS,
        "127.0.0.1 localhost\n::1 localhost ip6-localhost\n192.0.2.4 upstream GATEWAY.TEST. other # keep comment\n192.0.2.5 unrelated\n",
    );
    write(dir.path(), POLICY, &json!({"policies": {
        "Homepage": {"URL": "https://domjudge/"},
        "Bookmarks": [
            {"Title": "Contest Site", "Placement": "toolbar", "URL": "https://domjudge/"},
            {"Title": "Documentation", "Placement": "toolbar", "URL": "https://devdocs.io/"}
        ],
        "Certificates": {"Install": ["/etc/natsume/trust/local-origin-ca.crt"]},
        "DNSOverHTTPS": {"Enabled": false},
        "Permissions": {"Notifications": {"Allow": ["https://domjudge/", "https://other.test/"]}}
    }}).to_string());
    dir
}

fn write(root: &Path, relative: &str, content: &str) {
    parents(root, relative, true).unwrap_or_else(|e| panic!("parent: {e}"));
    fs::write(root.join(relative), content).unwrap_or_else(|e| panic!("write: {e}"));
    fs::set_permissions(root.join(relative), fs::Permissions::from_mode(0o644))
        .unwrap_or_else(|e| panic!("mode: {e}"));
}

fn text(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative)).unwrap_or_else(|e| panic!("read: {e}"))
}

fn policy(root: &Path) -> Value {
    serde_json::from_str(&text(root, POLICY)).unwrap_or_else(|e| panic!("JSON: {e}"))
}

fn apply(root: &Path, hostname: &str) {
    configure(root, hostname).unwrap_or_else(|e| panic!("configure: {e}"));
}

#[test]
fn daemon_hostname_applies_without_config_file_and_preserves_metadata() {
    let dir = fixture();
    let root = dir.path();
    let before = policy(root);
    let meta = fs::metadata(root.join(HOSTS)).unwrap_or_else(|e| panic!("metadata: {e}"));
    apply(root, "gateway.test");
    let after = policy(root);
    assert_eq!(
        after["policies"]["Homepage"],
        json!({"URL":"https://gateway.test/", "Locked":true, "StartPage":"homepage-locked"})
    );
    assert_eq!(
        after["policies"]["Bookmarks"][0]["URL"],
        "https://gateway.test/"
    );
    assert_eq!(
        after["policies"]["Bookmarks"][1],
        before["policies"]["Bookmarks"][1]
    );
    assert_eq!(
        after["policies"]["Certificates"],
        before["policies"]["Certificates"]
    );
    assert_eq!(
        after["policies"]["DNSOverHTTPS"],
        before["policies"]["DNSOverHTTPS"]
    );
    assert_eq!(
        after["policies"]["Permissions"]["Notifications"]["Allow"],
        json!(["https://gateway.test/", "https://other.test/"])
    );
    assert_eq!(
        text(root, HOSTS),
        format!(
            "127.0.0.1 localhost\n::1 localhost ip6-localhost\n192.0.2.4\tupstream other # keep comment\n192.0.2.5 unrelated\n{BEGIN}\n127.0.0.1 gateway.test\n::1 gateway.test\n{END}\n"
        )
    );
    let new_meta = fs::metadata(root.join(HOSTS)).unwrap_or_else(|e| panic!("metadata: {e}"));
    assert_eq!(
        (meta.uid(), meta.gid(), meta.mode()),
        (new_meta.uid(), new_meta.gid(), new_meta.mode())
    );
}

#[test]
fn repeated_startup_does_not_rewrite_either_file() {
    let dir = fixture();
    apply(dir.path(), "gateway.test");
    let before: Vec<_> = [HOSTS, POLICY]
        .map(|path| fs::metadata(dir.path().join(path)).unwrap_or_else(|e| panic!("metadata: {e}")))
        .into();
    apply(dir.path(), "gateway.test");
    for (path, old) in [HOSTS, POLICY].into_iter().zip(before) {
        let new = fs::metadata(dir.path().join(path)).unwrap_or_else(|e| panic!("metadata: {e}"));
        assert_eq!(
            (old.ino(), old.mtime(), old.mtime_nsec()),
            (new.ino(), new.mtime(), new.mtime_nsec())
        );
    }
}

#[test]
fn hostname_change_removes_old_owned_mapping_and_browser_references() {
    let dir = fixture();
    apply(dir.path(), "gateway.test");
    apply(dir.path(), "next.test");
    assert!(!text(dir.path(), HOSTS).contains("gateway.test"));
    assert!(!text(dir.path(), POLICY).contains("gateway.test"));
    assert_eq!(text(dir.path(), HOSTS).matches("next.test").count(), 2);
    assert_eq!(
        policy(dir.path())["policies"]["Permissions"]["Notifications"]["Allow"],
        json!(["https://next.test/", "https://other.test/"])
    );
    apply(dir.path(), "next.test");
    assert_eq!(
        policy(dir.path())["policies"]["Bookmarks"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
}

#[test]
fn missing_firefox_policy_is_created_without_home_or_certificate_inputs() {
    let dir = fixture();
    fs::remove_dir_all(dir.path().join("etc/firefox")).unwrap_or_else(|e| panic!("remove: {e}"));
    apply(dir.path(), "gateway.test");
    assert_eq!(
        policy(dir.path())["policies"]["Homepage"]["URL"],
        "https://gateway.test/"
    );
    assert!(policy(dir.path())["policies"].get("Certificates").is_none());
    let meta = fs::metadata(dir.path().join(POLICY)).unwrap_or_else(|e| panic!("metadata: {e}"));
    assert_eq!(meta.mode() & 0o777, 0o644);
}

#[test]
fn invalid_files_leave_both_target_files_untouched() {
    for (path, content) in [
        (HOSTS, "# BEGIN NATSUME GATEWAY\n127.0.0.1 old.test"),
        (HOSTS, "# END NATSUME GATEWAY\n"),
        (POLICY, "not JSON"),
        (POLICY, "{\"policies\":{\"Homepage\":null}}"),
        (POLICY, "{\"policies\":{\"Bookmarks\":false}}"),
    ] {
        let dir = fixture();
        write(dir.path(), path, content);
        let before = [text(dir.path(), HOSTS), text(dir.path(), POLICY)];
        assert!(
            matches!(
                configure(dir.path(), "gateway.test"),
                Err(ResourceControlError::Rejected(_))
            ),
            "{path}"
        );
        assert_eq!(before, [text(dir.path(), HOSTS), text(dir.path(), POLICY)]);
    }
}

#[test]
fn invalid_daemon_hostname_is_rejected_before_writing_files() {
    for hostname in [
        "",
        "bad name",
        "192.0.2.1",
        "https://gateway.test/",
        "Gateway.test",
        "gateway.test.",
        "gateway.test\n192.0.2.1 injected.test",
    ] {
        let dir = fixture();
        let before = [text(dir.path(), HOSTS), text(dir.path(), POLICY)];
        assert!(
            matches!(
                configure(dir.path(), hostname),
                Err(ResourceControlError::Rejected(_))
            ),
            "{hostname:?}"
        );
        assert_eq!(before, [text(dir.path(), HOSTS), text(dir.path(), POLICY)]);
    }
}

#[test]
fn untrusted_files_and_parent_symlinks_are_rejected_before_changes() {
    for path in [HOSTS, POLICY, "etc/firefox"] {
        let dir = fixture();
        let before = text(dir.path(), HOSTS);
        let target = dir.path().join(path);
        let moved = dir.path().join("untrusted");
        fs::rename(&target, &moved).unwrap_or_else(|e| panic!("rename: {e}"));
        symlink(&moved, &target).unwrap_or_else(|e| panic!("symlink: {e}"));
        assert!(matches!(
            configure(dir.path(), "gateway.test"),
            Err(ResourceControlError::Rejected(_))
        ));
        assert_eq!(before, text(dir.path(), HOSTS));
    }
    let dir = fixture();
    fs::set_permissions(dir.path().join(POLICY), fs::Permissions::from_mode(0o666))
        .unwrap_or_else(|e| panic!("permissions: {e}"));
    assert!(matches!(
        configure(dir.path(), "gateway.test"),
        Err(ResourceControlError::Rejected(_))
    ));
}

#[test]
fn interrupted_pair_replays_without_losing_old_notification_origin() {
    let dir = fixture();
    apply(dir.path(), "gateway.test");
    let old_hosts = text(dir.path(), HOSTS);
    apply(dir.path(), "next.test");
    // Simulate policy replacement completing before hosts replacement failed.
    write(dir.path(), HOSTS, &old_hosts);
    apply(dir.path(), "next.test");
    assert!(!text(dir.path(), HOSTS).contains("gateway.test"));
    assert!(!text(dir.path(), POLICY).contains("gateway.test"));
}

#[test]
fn root_group_writable_parent_is_trusted_but_other_users_are_not() {
    let dir = fixture();
    fs::set_permissions(dir.path().join("etc"), fs::Permissions::from_mode(0o775))
        .unwrap_or_else(|e| panic!("mode: {e}"));
    apply(dir.path(), "gateway.test");
    fs::set_permissions(dir.path().join("etc"), fs::Permissions::from_mode(0o777))
        .unwrap_or_else(|e| panic!("mode: {e}"));
    match configure(dir.path(), "gateway.test") {
        Err(ResourceControlError::Rejected(message)) => assert!(message.starts_with("/etc:")),
        result => panic!("expected parent rejection: {result:?}"),
    }
}
