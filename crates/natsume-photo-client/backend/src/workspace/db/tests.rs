use diesel::connection::SimpleConnection as _;

use super::*;
use crate::workspace::Workspace;

fn roster() -> Result<Roster> {
    Roster::parse(include_bytes!("../../../tests/fixtures/roster.json")).context(RosterSnafu)
}

fn unready() -> Result<(tempfile::TempDir, Database)> {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let path = directory.path().join("workspace.sqlite3");
    std::fs::File::create_new(&path).context(IoSnafu)?;
    let mut database = Database::connect(&path)?;
    database.migrate()?;
    Ok((directory, database))
}

#[test]
fn incomplete_workspace_is_rejected() -> Result<()> {
    let (directory, _database) = unready()?;
    assert!(matches!(
        Workspace::open(directory.path()),
        Err(Error::Incomplete)
    ));
    Ok(())
}

#[test]
fn unsupported_version_is_rejected_without_changing_database() -> Result<()> {
    let (directory, mut database) = unready()?;
    database.write(|transaction| {
        diesel::update(photo_workspace::table.find(1))
            .set(photo_workspace::format_version.eq(999))
            .execute(transaction.connection())
            .context(DatabaseSnafu)?;
        Ok(())
    })?;
    drop(database);
    let path = directory.path().join("workspace.sqlite3");
    let before = std::fs::read(&path).context(IoSnafu)?;
    assert!(matches!(
        Workspace::open(directory.path()),
        Err(Error::Version { version: 999 })
    ));
    assert_eq!(std::fs::read(path).context(IoSnafu)?, before);
    Ok(())
}

#[test]
fn failed_roster_write_rolls_back_all_rows_and_ready_flag() -> Result<()> {
    let (directory, mut database) = unready()?;
    // Fault injection only: abort the second insert after the first has succeeded.
    database
        .0
        .batch_execute(
            "CREATE TRIGGER fail_second_insert BEFORE INSERT ON photo_roster
         WHEN NEW.account = 'T-002'
         BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
        )
        .context(DatabaseSnafu)?;
    assert!(matches!(
        database.initialize(&roster()?, &Cache::default()),
        Err(Error::Database { .. })
    ));
    let count: i64 = database.read(|transaction| {
        photo_roster::table
            .count()
            .get_result(transaction.connection())
            .context(DatabaseSnafu)
    })?;
    assert_eq!(count, 0);
    assert!(matches!(
        Workspace::open(directory.path()),
        Err(Error::Incomplete)
    ));
    Ok(())
}

#[test]
fn ready_marker_does_not_hide_an_empty_or_invalid_persisted_roster() -> Result<()> {
    let (directory, mut database) = unready()?;
    database.initialize(&roster()?, &Cache::default())?;
    database.write(|transaction| {
        diesel::update(photo_roster::table.filter(photo_roster::account.eq("T-001")))
            .set(photo_roster::account_id.eq(" "))
            .execute(transaction.connection())
            .context(DatabaseSnafu)?;
        Ok(())
    })?;
    assert!(matches!(
        Workspace::open(directory.path()),
        Err(Error::Roster {
            source: crate::roster::Error::MissingIdentity { .. }
        })
    ));
    database.write(|transaction| {
        diesel::delete(photo_roster::table)
            .execute(transaction.connection())
            .context(DatabaseSnafu)?;
        Ok(())
    })?;
    assert!(matches!(
        Workspace::open(directory.path()),
        Err(Error::Roster {
            source: crate::roster::Error::Empty
        })
    ));
    Ok(())
}

#[test]
fn read_write_connection_mode_never_creates_missing_database() -> Result<()> {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let path = directory.path().join("missing.sqlite3");
    assert!(matches!(
        Database::connect(&path),
        Err(Error::Connection { .. })
    ));
    assert!(!path.exists());
    Ok(())
}

#[test]
fn previous_workspace_schema_migrates_without_losing_roster() -> Result<()> {
    let directory = tempfile::tempdir().context(IoSnafu)?;
    let path = directory.path().join("workspace.sqlite3");
    std::fs::File::create_new(&path).context(IoSnafu)?;
    let mut database = Database::connect(&path)?;
    database
        .0
        .run_next_migration(MIGRATIONS)
        .map_err(|_| MigrationSnafu.build())?;
    database.write(|transaction| {
        for team in roster()?.teams() {
            diesel::insert_into(photo_roster::table)
                .values((
                    photo_roster::account_id.eq(&team.account_id),
                    photo_roster::account.eq(&team.account),
                    photo_roster::organization_id.eq(team.organization_id),
                    photo_roster::school_name_zh.eq(&team.school_name_zh),
                    photo_roster::school_name_en.eq(&team.school_name_en),
                    photo_roster::team_name_zh.eq(&team.team_name_zh),
                    photo_roster::team_name_en.eq(&team.team_name_en),
                ))
                .execute(transaction.connection())
                .context(DatabaseSnafu)?;
        }
        diesel::update(photo_workspace::table.find(1))
            .set(photo_workspace::ready.eq(true))
            .execute(transaction.connection())
            .context(DatabaseSnafu)?;
        Ok(())
    })?;
    drop(database);
    let mut workspace = Workspace::open(directory.path())?;
    assert_eq!(workspace.roster()?.teams().len(), 2);
    assert!(workspace.default_image_path()?.is_none());
    assert!(
        workspace
            .logo_path(&roster()?.teams()[0].account_id)?
            .is_none()
    );
    Ok(())
}
