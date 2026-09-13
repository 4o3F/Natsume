use serde_json::Value;
use std::{collections::BTreeMap, fs, io::Read as _, os::unix::fs::PermissionsExt as _, sync::Arc};
use tempfile::TempDir;
use zip::ZipArchive;

use super::*;
use crate::{
    component::{
        contest::{OrganizationDetails, roster::ExportTeam},
        import::{ImportComponent, tests::workbook},
    },
    db::{Database, DatabaseConfig},
    vault,
};

fn checked<T, E: std::fmt::Debug>(value: Result<T, E>) -> T {
    value.unwrap_or_else(|error| panic!("fixture operation failed: {error:?}"))
}
fn vault_fixture() -> (TempDir, Arc<VaultSession>) {
    let root = checked(TempDir::new());
    checked(fs::set_permissions(
        root.path(),
        fs::Permissions::from_mode(0o700),
    ));
    checked(vault::ensure_master_key(&root.path().join("key")));
    let vault = Arc::new(checked(vault::load(&root.path().join("key"))));
    (root, vault)
}
fn entries(bytes: Vec<u8>) -> BTreeMap<String, Vec<u8>> {
    let mut zip = checked(ZipArchive::new(Cursor::new(bytes)));
    (0..zip.len())
        .map(|i| {
            let mut entry = checked(zip.by_index(i));
            let mut bytes = Vec::new();
            checked(entry.read_to_end(&mut bytes));
            (entry.name().to_owned(), bytes)
        })
        .collect()
}
fn school(id: i64, name: &str) -> OrganizationDetails {
    OrganizationDetails {
        organization_id: id,
        name_zh: name.to_owned(),
        name_en: String::new(),
        country: "CHN".to_owned(),
    }
}
fn team(vault: &VaultSession, account: &str, organization_id: i64, password: &str) -> ExportTeam {
    let (nonce, ciphertext) = checked(vault.seal(password.as_bytes()));
    ExportTeam {
        account: account.to_owned(),
        seat: format!("A-{account}"),
        organization_id,
        name_zh: "示例队".to_owned(),
        name_en: "Example team".to_owned(),
        category: "participant".to_owned(),
        nonce: nonce.to_vec(),
        ciphertext,
    }
}
fn json(files: &BTreeMap<String, Vec<u8>>, name: &str) -> Value {
    checked(serde_json::from_slice(&files[name]))
}

#[test]
fn complete_zip_contains_stable_data_deduplicated_pngs_and_missing_ambiguous_mapping() {
    let (root, vault) = vault_fixture();
    let logos = root.path().join("logos");
    checked(fs::create_dir(&logos));
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="4"><rect width="4" height="4" fill="#ff0000"/></svg>"##;
    for name in ["甲.webp", "丙.png", "丙.svg", "unrelated.svg"] {
        checked(fs::write(logos.join(name), svg));
    }
    let mut roster = ExportRoster {
        organizations: vec![school(1, "甲"), school(2, "乙"), school(3, "丙")],
        teams: vec![
            team(&vault, "001", 1, "00123"),
            team(&vault, "team2", 1, "特殊: # \"密码\"\\\n"),
            team(&vault, "team3", 2, "yes"),
            team(&vault, "team4", 3, "no"),
        ],
    };
    let first = entries(checked(build(&roster, &vault, &logos)));
    assert_eq!(
        first.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "README.md",
            "accounts.yaml",
            "groups.json",
            "logos/INST-001.png",
            "organizations.json",
            "teams.json"
        ]
    );
    assert_eq!(
        json(&first, "groups.json"),
        serde_json::json!([{"id":"participant","name":"participant"}])
    );
    let teams = json(&first, "teams.json");
    assert_eq!(teams[0]["id"], "001");
    assert_eq!(teams[0]["name"], "示例队(Example team)");
    assert_eq!(teams[0]["organization_id"], "INST-001");
    assert_eq!(teams[0]["label"], teams[0]["location"]["description"]);
    assert!(teams[0].get("icpc_id").is_none());
    let accounts: Vec<Value> = checked(serde_saphyr::from_slice(&first["accounts.yaml"]));
    assert_eq!(accounts[0]["id"], "001");
    assert_eq!(accounts[0]["password"], "00123");
    assert_eq!(accounts[1]["password"], "特殊: # \"密码\"\\\n");
    for account in &accounts {
        assert_eq!(account["id"], account["username"]);
        assert_eq!(account["id"], account["team_id"]);
        assert_eq!(account["type"], "team");
    }
    let png = checked(image::load_from_memory(&first["logos/INST-001.png"])).to_rgba8();
    assert_eq!(png.dimensions(), (8, 4));
    assert_eq!(png.get_pixel(6, 1).0, [0, 0, 0, 0]);
    let readme = checked(std::str::from_utf8(&first["README.md"]));
    assert!(readme.contains("INST-002 | 乙 |  | Missing"));
    assert!(readme.contains("INST-003 | 丙 |  | Ambiguous"));
    assert_eq!(entries(checked(build(&roster, &vault, &logos))), first);
    roster.teams[0] = team(&vault, "001", 1, "next-password");
    let second = entries(checked(build(&roster, &vault, &logos)));
    assert_ne!(second["accounts.yaml"], first["accounts.yaml"]);
    for (name, bytes) in &first {
        if name != "accounts.yaml" {
            assert_eq!(&second[name], bytes);
        }
    }
    assert_eq!(checked(fs::read(logos.join("甲.webp"))), svg);
    checked(fs::write(logos.join("甲.webp"), "broken image"));
    let error = build(&roster, &vault, &logos)
        .err()
        .unwrap_or_else(|| panic!("corrupt logo accepted"));
    assert!(matches!(error, ExportError::Logo { .. }));
    assert!(error.to_string().contains("INST-001 甲: 甲.webp"));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn export_reads_committed_full_roster_and_current_vault_independent_of_candidates() {
    let (root, vault) = vault_fixture();
    let database = checked(
        Database::connect_and_migrate(&DatabaseConfig::new(root.path().join("db"), true)).await,
    );
    let import = ImportComponent::new(database.clone(), vault.clone());
    let contest = ContestComponent::new(database, vault, root.path().join("absent-logos"));
    assert!(matches!(
        contest.export_domjudge().await,
        Err(ExportError::IncompleteRoster)
    ));
    let rows: Vec<_> = (0..400)
        .map(|i| {
            [
                format!("学校{:03}", i % 240),
                String::new(),
                "CHN".to_owned(),
                format!("Team{:03}", i + 1),
                "old-password".to_owned(),
                format!("A-{i}"),
                "队名".to_owned(),
                "Team".to_owned(),
                "participant".to_owned(),
            ]
        })
        .collect();
    let xlsx = workbook(
        &rows
            .iter()
            .map(|row| row.each_ref().map(String::as_str))
            .collect::<Vec<_>>(),
    );
    let preview = checked(import.create_candidate(&xlsx).await);
    assert!(matches!(
        contest.export_domjudge().await,
        Err(ExportError::IncompleteRoster)
    ));
    checked(
        import
            .commit(preview.candidate_id(), preview.preview_token_bytes(), &xlsx)
            .await,
    );
    let first = entries(checked(contest.export_domjudge().await));
    assert_eq!(
        json(&first, "teams.json").as_array().map(Vec::len),
        Some(400)
    );
    assert_eq!(
        json(&first, "organizations.json").as_array().map(Vec::len),
        Some(240)
    );
    let mut next_rows = rows;
    next_rows[0][4] = "rotated-password".to_owned();
    let next_xlsx = workbook(
        &next_rows
            .iter()
            .map(|row| row.each_ref().map(String::as_str))
            .collect::<Vec<_>>(),
    );
    let preview = checked(import.create_candidate(&next_xlsx).await);
    assert_eq!(entries(checked(contest.export_domjudge().await)), first);
    checked(
        import
            .commit(
                preview.candidate_id(),
                preview.preview_token_bytes(),
                &next_xlsx,
            )
            .await,
    );
    let next = entries(checked(contest.export_domjudge().await));
    let accounts: Vec<Value> = checked(serde_saphyr::from_slice(&next["accounts.yaml"]));
    assert_eq!(accounts.len(), 400);
    assert_eq!(accounts[0]["password"], "rotated-password");
    assert_eq!(
        accounts
            .iter()
            .filter(|a| a["password"] == "old-password")
            .count(),
        399
    );
    for (name, bytes) in first {
        if name != "accounts.yaml" {
            assert_eq!(next[&name], bytes);
        }
    }
    checked(
        contest
            .database
            .write(|transaction| {
                use diesel::connection::SimpleConnection as _;
                transaction
                    .connection()
                    .batch_execute(
                        "INSERT INTO accounts VALUES ('legacy-account', 'legacy-account', 1)",
                    )
                    .map_err(|_| PersistenceError::OperationFailed)
            })
            .await,
    );
    assert!(matches!(
        contest.export_domjudge().await,
        Err(ExportError::IncompleteRoster)
    ));
    let _permit = checked(contest.export_work.try_acquire());
    assert!(matches!(
        contest.export_domjudge().await,
        Err(ExportError::Busy)
    ));
}

#[test]
fn memory_archive_rejects_an_oversized_write_without_allocating_it() {
    let mut writer = LimitedArchive(Cursor::new(Vec::new()));
    checked(writer.seek(SeekFrom::Start(MAX_EXPORT_BYTES)));
    assert!(writer.write_all(b"x").is_err());
    assert!(writer.0.get_ref().is_empty());
}
