use super::super::{db::tests as fixtures, tests::PasswordVerificationTestGuard};
use super::*;
use crate::db::{Database, DatabaseConfig, PersistenceError, TransactionError};
use diesel::{RunQueryDsl, connection::SimpleConnection};
use std::{future::Future, pin::Pin, sync::Arc, task::Context, time::Duration};
use tempfile::TempDir;
use tokio::sync::Barrier;

const PASSWORD: &str = "operator-password1!";
const NEW_PASSWORD: &str = "replacement-password2@";
type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Fixture {
    component: OperatorComponent,
    admin: OperatorIdentity,
    directory: TempDir,
}

impl Fixture {
    async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let database = Database::connect_and_migrate(&DatabaseConfig::new(
            directory.path().join("operators.sqlite"),
            true,
        ))
        .await?;
        let phc = hash_password(&OperatorPassword::new(PASSWORD.to_owned()))?;
        let id =
            fixtures::test_insert_account(&database, "admin", OperatorRole::Admin, &phc).await?;
        Ok(Self {
            component: OperatorComponent::new(database),
            admin: OperatorIdentity {
                operator_id: id,
                role: OperatorRole::Admin,
            },
            directory,
        })
    }

    async fn account(
        &self,
        name: &str,
        role: OperatorRole,
    ) -> Result<OperatorIdentity, OperatorError> {
        let phc = hash_password(&OperatorPassword::new(PASSWORD.to_owned()))?;
        let id = fixtures::test_insert_account(&self.component.database, name, role, &phc).await?;
        Ok(OperatorIdentity {
            operator_id: id,
            role,
        })
    }

    async fn sql(&self, statement: &str) -> Result<(), OperatorError> {
        let statement = statement.to_owned();
        self.component
            .database
            .write(move |tx| {
                tx.connection()
                    .batch_execute(&statement)
                    .map_err(|_| PersistenceError::OperationFailed)
            })
            .await
            .map_err(TransactionError::into_error)
            .map_err(OperatorError::from)
    }
}

async fn register(
    component: &OperatorComponent,
    token: &LinkToken,
    name: &str,
) -> Result<OperatorIdentity, OperatorError> {
    component
        .register(
            token.expose().to_owned(),
            name.to_owned(),
            PASSWORD.to_owned(),
            PASSWORD.to_owned(),
        )
        .await
}

async fn reset(component: &OperatorComponent, token: &LinkToken) -> Result<(), OperatorError> {
    component
        .consume_password_reset(
            token.expose().to_owned(),
            NEW_PASSWORD.to_owned(),
            NEW_PASSWORD.to_owned(),
        )
        .await
}

#[tokio::test]
async fn invitation_registration_preserves_conflicts_and_secret_boundaries() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let before = fixtures::test_now(&f.component.database).await?;
    let issued = f
        .component
        .issue_invitation(f.admin, OperatorRole::Viewer)
        .await?;
    let after = fixtures::test_now(&f.component.database).await?;
    assert!((before..=after).contains(&issued.invitation.created_at_unix_ms));
    assert_eq!(
        issued.invitation.expires_at_unix_ms - issued.invitation.created_at_unix_ms,
        INVITATION_TTL_MS
    );
    assert!(!format!("{issued:?}").contains(issued.token.expose()));
    assert_eq!(f.component.list_accounts(f.admin).await?.len(), 1);
    assert_eq!(
        f.component
            .inspect_invitation(issued.token.expose().to_owned())
            .await?,
        issued.invitation
    );
    assert_eq!(
        register(&f.component, &issued.token, " admin").await,
        Err(OperatorError::InvalidNewLoginName)
    );
    assert_eq!(
        f.component
            .register(
                issued.token.expose().to_owned(),
                "new".to_owned(),
                PASSWORD.to_owned(),
                NEW_PASSWORD.to_owned()
            )
            .await,
        Err(OperatorError::PasswordMismatch)
    );
    assert_eq!(
        register(&f.component, &issued.token, "admin").await,
        Err(OperatorError::LoginNameConflict)
    );
    assert_eq!(
        f.component.list_invitations(f.admin).await?,
        vec![issued.invitation.clone()]
    );
    let viewer = register(&f.component, &issued.token, "Admin").await?;
    assert_eq!(viewer.role, OperatorRole::Viewer);
    assert_eq!(f.component.list_accounts(f.admin).await?.len(), 2);
    assert!(f.component.list_invitations(f.admin).await?.is_empty());
    assert_eq!(
        register(&f.component, &issued.token, "another").await,
        Err(OperatorError::LinkUnavailable)
    );
    let session = f.component.sign_in("Admin", PASSWORD.to_owned()).await?;
    assert_eq!(session.identity(), viewer);
    Ok(())
}

#[tokio::test]
async fn viewers_cannot_manage_accounts_or_links() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let viewer = f.account("viewer", OperatorRole::Viewer).await?;
    let issued = f
        .component
        .issue_invitation(f.admin, OperatorRole::Admin)
        .await?;
    assert_eq!(
        f.component.list_accounts(viewer).await.err(),
        Some(OperatorError::AuthorizationDenied)
    );
    assert_eq!(
        f.component.list_invitations(viewer).await.err(),
        Some(OperatorError::AuthorizationDenied)
    );
    assert_eq!(
        f.component
            .change_role(viewer, viewer.operator_id(), OperatorRole::Admin)
            .await,
        Err(OperatorError::AuthorizationDenied)
    );
    assert_eq!(
        f.component
            .delete_account(viewer, f.admin.operator_id())
            .await,
        Err(OperatorError::AuthorizationDenied)
    );
    assert_eq!(
        f.component
            .issue_invitation(viewer, OperatorRole::Viewer)
            .await
            .err(),
        Some(OperatorError::AuthorizationDenied)
    );
    assert_eq!(
        f.component
            .regenerate_invitation(viewer, issued.invitation.invitation_id)
            .await
            .err(),
        Some(OperatorError::AuthorizationDenied)
    );
    assert_eq!(
        f.component
            .revoke_invitation(viewer, issued.invitation.invitation_id)
            .await,
        Err(OperatorError::AuthorizationDenied)
    );
    assert_eq!(
        f.component
            .issue_password_reset(viewer, viewer.operator_id())
            .await
            .err(),
        Some(OperatorError::AuthorizationDenied)
    );
    Ok(())
}

#[tokio::test]
async fn last_admin_is_protected_including_concurrent_self_removals() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    f.component
        .issue_invitation(f.admin, OperatorRole::Admin)
        .await?;
    assert_eq!(
        f.component
            .delete_account(f.admin, f.admin.operator_id())
            .await,
        Err(OperatorError::LastAdmin)
    );
    assert_eq!(
        f.component
            .change_role(f.admin, f.admin.operator_id(), OperatorRole::Viewer)
            .await,
        Err(OperatorError::LastAdmin)
    );
    // An idempotent admin assignment is allowed even for the sole admin.
    f.component
        .change_role(f.admin, f.admin.operator_id(), OperatorRole::Admin)
        .await?;
    for delete in [false, true] {
        let f = Fixture::new().await?;
        let second = f.account("second", OperatorRole::Admin).await?;
        let barrier = Arc::new(Barrier::new(2));
        let attempt = |actor, barrier: Arc<Barrier>| {
            let component = &f.component;
            async move {
                barrier.wait().await;
                if delete {
                    component.delete_account(actor, actor.operator_id()).await
                } else {
                    component
                        .change_role(actor, actor.operator_id(), OperatorRole::Viewer)
                        .await
                }
            }
        };
        let (one, two) = tokio::join!(attempt(f.admin, barrier.clone()), attempt(second, barrier));
        assert!(matches!(
            (one, two),
            (Ok(()), Err(OperatorError::LastAdmin)) | (Err(OperatorError::LastAdmin), Ok(()))
        ));
        let admins = f
            .component
            .list_accounts(f.admin)
            .await?
            .into_iter()
            .filter(|row| row.role == OperatorRole::Admin)
            .count();
        assert_eq!(admins, 1);
    }
    Ok(())
}

#[tokio::test]
async fn account_demotion_and_deletion_affect_subsequent_authentication() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let second = f.account("second", OperatorRole::Admin).await?;
    let session = f.component.sign_in("second", PASSWORD.to_owned()).await?;
    let already_authenticated = f
        .component
        .authenticate_session(session.wire_credential())
        .await?;
    f.component
        .change_role(second, second.operator_id(), OperatorRole::Viewer)
        .await?;
    assert_eq!(already_authenticated.require_admin(), Ok(()));
    assert_eq!(
        f.component
            .authenticate_session(session.wire_credential())
            .await?
            .require_admin(),
        Err(OperatorError::AuthorizationDenied)
    );
    f.component
        .delete_account(f.admin, second.operator_id())
        .await?;
    assert_eq!(
        f.component
            .authenticate_session(session.wire_credential())
            .await,
        Err(OperatorError::SessionAuthenticationFailed)
    );
    assert_eq!(
        f.component
            .change_role(f.admin, second.operator_id(), OperatorRole::Admin)
            .await,
        Err(OperatorError::OperatorNotFound)
    );
    Ok(())
}

#[tokio::test]
async fn expired_invitations_remain_visible_and_regeneration_revokes_old_tokens() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let issued = f
        .component
        .issue_invitation(f.admin, OperatorRole::Viewer)
        .await?;
    f.sql("UPDATE operator_invitations SET expires_at_unix_ms = 0")
        .await?;
    assert_eq!(f.component.list_invitations(f.admin).await?.len(), 1);
    assert_eq!(
        f.component
            .inspect_invitation(issued.token.expose().to_owned())
            .await
            .err(),
        Some(OperatorError::LinkUnavailable)
    );
    let new = f
        .component
        .regenerate_invitation(f.admin, issued.invitation.invitation_id)
        .await?;
    assert_eq!(new.invitation.role, issued.invitation.role);
    assert_ne!(
        new.invitation.invitation_id,
        issued.invitation.invitation_id
    );
    assert_eq!(
        new.invitation.expires_at_unix_ms - new.invitation.created_at_unix_ms,
        INVITATION_TTL_MS
    );
    assert_eq!(
        register(&f.component, &issued.token, "old").await,
        Err(OperatorError::LinkUnavailable)
    );
    f.component
        .revoke_invitation(f.admin, new.invitation.invitation_id)
        .await?;
    f.component
        .revoke_invitation(f.admin, new.invitation.invitation_id)
        .await?;
    assert_eq!(
        register(&f.component, &new.token, "new").await,
        Err(OperatorError::LinkUnavailable)
    );
    Ok(())
}

#[tokio::test]
async fn issuer_loss_revokes_both_link_kinds_and_promotion_does_not_resurrect_them() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    for delete in [false, true] {
        let f = Fixture::new().await?;
        let issuer = f.account("issuer", OperatorRole::Admin).await?;
        let target = f.account("target", OperatorRole::Viewer).await?;
        let registered_invite = f
            .component
            .issue_invitation(issuer, OperatorRole::Viewer)
            .await?;
        let registered = register(&f.component, &registered_invite.token, "registered").await?;
        let invite = f
            .component
            .issue_invitation(issuer, OperatorRole::Admin)
            .await?;
        let reset_link = f
            .component
            .issue_password_reset(issuer, target.operator_id())
            .await?;
        if delete {
            f.component
                .delete_account(issuer, issuer.operator_id())
                .await?;
        } else {
            f.component
                .change_role(issuer, issuer.operator_id(), OperatorRole::Viewer)
                .await?;
            f.component
                .change_role(f.admin, issuer.operator_id(), OperatorRole::Admin)
                .await?;
        }
        assert_eq!(
            f.component
                .inspect_invitation(invite.token.expose().to_owned())
                .await
                .err(),
            Some(OperatorError::LinkUnavailable)
        );
        assert_eq!(
            f.component
                .inspect_password_reset(reset_link.token.expose().to_owned())
                .await
                .err(),
            Some(OperatorError::LinkUnavailable)
        );
        assert!(
            f.component
                .list_accounts(f.admin)
                .await?
                .iter()
                .any(|account| account.operator_id == registered.operator_id())
        );
        if delete {
            assert_eq!(
                f.component
                    .issue_invitation(issuer, OperatorRole::Viewer)
                    .await
                    .err(),
                Some(OperatorError::AuthorizationDenied)
            );
        }
        assert!(f.component.list_invitations(f.admin).await?.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn concurrent_registration_consumes_once_and_keeps_the_conflicting_invitation() -> TestResult
{
    let _guard = PasswordVerificationTestGuard::acquire().await;
    for same_token in [true, false] {
        let f = Fixture::new().await?;
        let one = f
            .component
            .issue_invitation(f.admin, OperatorRole::Viewer)
            .await?;
        let two = f
            .component
            .issue_invitation(f.admin, OperatorRole::Viewer)
            .await?;
        let barrier = Arc::new(Barrier::new(2));
        let attempt = |token: String, name: String, barrier: Arc<Barrier>| {
            let component = &f.component;
            async move {
                barrier.wait().await;
                component
                    .register(token, name, PASSWORD.to_owned(), PASSWORD.to_owned())
                    .await
            }
        };
        let (a, b) = tokio::join!(
            attempt(
                one.token.expose().to_owned(),
                "new".to_owned(),
                barrier.clone()
            ),
            attempt(
                if same_token {
                    one.token.expose().to_owned()
                } else {
                    two.token.expose().to_owned()
                },
                if same_token { "different" } else { "new" }.to_owned(),
                barrier
            )
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        let failure = a.err().or(b.err());
        assert_eq!(
            failure,
            Some(if same_token {
                OperatorError::LinkUnavailable
            } else {
                OperatorError::LoginNameConflict
            })
        );
        let remaining = f.component.list_invitations(f.admin).await?;
        assert_eq!(remaining.len(), 1);
        let remaining_token = if remaining[0].invitation_id == one.invitation.invitation_id {
            &one.token
        } else {
            &two.token
        };
        register(&f.component, remaining_token, "retry").await?;
    }
    Ok(())
}

#[tokio::test]
async fn reset_generation_replacement_expiry_and_uuid_binding() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let target = f.account("target", OperatorRole::Viewer).await?;
    let session = f.component.sign_in("target", PASSWORD.to_owned()).await?;
    let before = fixtures::test_account_credentials(&f.component.database, "target").await?;
    let before_issue = fixtures::test_now(&f.component.database).await?;
    let one = f
        .component
        .issue_password_reset(f.admin, target.operator_id())
        .await?;
    let now = fixtures::test_now(&f.component.database).await?;
    assert!((before_issue..=now).contains(&(one.reset.expires_at_unix_ms - RESET_TTL_MS)));
    assert_eq!(one.reset.username, "target");
    assert!(!format!("{one:?}").contains(one.token.expose()));
    assert_eq!(
        f.component
            .inspect_password_reset(one.token.expose().to_owned())
            .await?,
        one.reset
    );
    assert_eq!(
        fixtures::test_account_credentials(&f.component.database, "target").await?,
        before
    );
    assert_eq!(
        f.component
            .authenticate_session(session.wire_credential())
            .await?,
        target
    );
    let two = f
        .component
        .issue_password_reset(f.admin, target.operator_id())
        .await?;
    assert_eq!(
        reset(&f.component, &one.token).await,
        Err(OperatorError::LinkUnavailable)
    );
    assert_eq!(
        f.component
            .inspect_invitation(two.token.expose().to_owned())
            .await
            .err(),
        Some(OperatorError::LinkUnavailable)
    );
    f.sql("UPDATE operator_password_resets SET expires_at_unix_ms = 0")
        .await?;
    assert_eq!(
        reset(&f.component, &two.token).await,
        Err(OperatorError::LinkUnavailable)
    );
    let old = f
        .component
        .issue_password_reset(f.admin, target.operator_id())
        .await?;
    f.component
        .delete_account(f.admin, target.operator_id())
        .await?;
    let invite = f
        .component
        .issue_invitation(f.admin, OperatorRole::Viewer)
        .await?;
    let new_target = register(&f.component, &invite.token, "target").await?;
    assert_ne!(new_target.operator_id(), target.operator_id());
    assert_eq!(
        reset(&f.component, &old.token).await,
        Err(OperatorError::LinkUnavailable)
    );
    assert_eq!(
        f.component
            .authenticate_session(session.wire_credential())
            .await,
        Err(OperatorError::SessionAuthenticationFailed)
    );
    Ok(())
}

#[tokio::test]
async fn concurrent_reset_consumes_once_and_revokes_all_sessions() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let a = f.component.sign_in("admin", PASSWORD.to_owned()).await?;
    let b = f.component.sign_in("admin", PASSWORD.to_owned()).await?;
    let issued = f
        .component
        .issue_password_reset(f.admin, f.admin.operator_id())
        .await?;
    let barrier = Arc::new(Barrier::new(2));
    let attempt = |barrier: Arc<Barrier>| {
        let component = &f.component;
        let token = &issued.token;
        async move {
            barrier.wait().await;
            reset(component, token).await
        }
    };
    let (one, two) = tokio::join!(attempt(barrier.clone()), attempt(barrier));
    assert!(matches!(
        (one, two),
        (Ok(()), Err(OperatorError::LinkUnavailable))
            | (Err(OperatorError::LinkUnavailable), Ok(()))
    ));
    assert_eq!(
        fixtures::test_account_credentials(&f.component.database, "admin")
            .await?
            .1,
        2
    );
    for session in [a, b] {
        assert_eq!(
            f.component
                .authenticate_session(session.wire_credential())
                .await,
            Err(OperatorError::SessionAuthenticationFailed)
        );
    }
    assert_eq!(
        f.component
            .sign_in("admin", PASSWORD.to_owned())
            .await
            .err(),
        Some(OperatorError::AuthenticationFailed)
    );
    f.component
        .sign_in("admin", NEW_PASSWORD.to_owned())
        .await?;
    Ok(())
}

#[tokio::test]
async fn own_password_change_accepts_legacy_password_and_revokes_recovery_grants() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let legacy = " 旧密码 ";
    let phc = hash_password(&OperatorPassword::new(legacy.to_owned()))?;
    let id =
        fixtures::test_insert_account(&f.component.database, "legacy", OperatorRole::Viewer, &phc)
            .await?;
    let session = f.component.sign_in("legacy", legacy.to_owned()).await?;
    let issued = f.component.issue_password_reset(f.admin, id).await?;
    let actor = session.identity();
    assert_eq!(
        f.component
            .change_password(
                actor,
                "wrong".to_owned(),
                NEW_PASSWORD.to_owned(),
                NEW_PASSWORD.to_owned()
            )
            .await,
        Err(OperatorError::AuthenticationFailed)
    );
    assert_eq!(
        fixtures::test_account_credentials(&f.component.database, "legacy").await?,
        (phc, 1)
    );
    f.component
        .inspect_password_reset(issued.token.expose().to_owned())
        .await?;
    f.component
        .change_password(
            actor,
            legacy.to_owned(),
            NEW_PASSWORD.to_owned(),
            NEW_PASSWORD.to_owned(),
        )
        .await?;
    assert_eq!(
        f.component
            .authenticate_session(session.wire_credential())
            .await,
        Err(OperatorError::SessionAuthenticationFailed)
    );
    assert_eq!(
        reset(&f.component, &issued.token).await,
        Err(OperatorError::LinkUnavailable)
    );
    f.component
        .sign_in("legacy", NEW_PASSWORD.to_owned())
        .await?;
    // Replacing a password with itself still advances the credential fence.
    let issued = f.component.issue_password_reset(f.admin, id).await?;
    reset(&f.component, &issued.token).await?;
    assert_eq!(
        fixtures::test_account_credentials(&f.component.database, "legacy")
            .await?
            .1,
        3
    );
    Ok(())
}

#[tokio::test]
async fn tty_reset_uses_the_same_grant_and_session_revocation_transaction() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let session = f.component.sign_in("admin", PASSWORD.to_owned()).await?;
    let issued = f
        .component
        .issue_password_reset(f.admin, f.admin.operator_id())
        .await?;
    let phc = hash_password(&OperatorPassword::new(NEW_PASSWORD.to_owned()))?;
    f.component.reset_password("admin", &phc).await?;
    assert_eq!(
        f.component
            .inspect_password_reset(issued.token.expose().to_owned())
            .await
            .err(),
        Some(OperatorError::LinkUnavailable)
    );
    assert_eq!(
        f.component
            .authenticate_session(session.wire_credential())
            .await,
        Err(OperatorError::SessionAuthenticationFailed)
    );
    assert_eq!(
        fixtures::test_account_credentials(&f.component.database, "admin").await?,
        (phc, 2)
    );
    Ok(())
}

#[tokio::test]
async fn registration_and_regeneration_roll_back_if_invitation_deletion_fails() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let invite = f
        .component
        .issue_invitation(f.admin, OperatorRole::Viewer)
        .await?;
    f.sql("CREATE TRIGGER reject_invitation_delete BEFORE DELETE ON operator_invitations BEGIN SELECT RAISE(ABORT, 'test'); END;").await?;
    assert_eq!(
        register(&f.component, &invite.token, "new").await,
        Err(OperatorError::PersistenceFailed)
    );
    assert_eq!(f.component.list_accounts(f.admin).await?.len(), 1);
    assert_eq!(
        f.component
            .regenerate_invitation(f.admin, invite.invitation.invitation_id)
            .await
            .err(),
        Some(OperatorError::PersistenceFailed)
    );
    f.component
        .inspect_invitation(invite.token.expose().to_owned())
        .await?;
    f.sql("DROP TRIGGER reject_invitation_delete").await?;
    register(&f.component, &invite.token, "new").await?;
    Ok(())
}

#[tokio::test]
async fn failed_reset_and_demotion_preserve_credentials_sessions_and_grants() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let second = f.account("second", OperatorRole::Admin).await?;
    let session = f.component.sign_in("second", PASSWORD.to_owned()).await?;
    let invite = f
        .component
        .issue_invitation(second, OperatorRole::Viewer)
        .await?;
    let issued = f
        .component
        .issue_password_reset(second, second.operator_id())
        .await?;
    let before = fixtures::test_account_credentials(&f.component.database, "second").await?;
    f.sql("CREATE TRIGGER reject_reset_delete BEFORE DELETE ON operator_password_resets BEGIN SELECT RAISE(ABORT, 'test'); END;").await?;
    assert_eq!(
        reset(&f.component, &issued.token).await,
        Err(OperatorError::PersistenceFailed)
    );
    assert_eq!(
        fixtures::test_account_credentials(&f.component.database, "second").await?,
        before
    );
    assert_eq!(
        f.component
            .authenticate_session(session.wire_credential())
            .await?,
        second
    );
    assert_eq!(
        f.component
            .change_role(second, second.operator_id(), OperatorRole::Viewer)
            .await,
        Err(OperatorError::PersistenceFailed)
    );
    assert_eq!(
        f.component
            .delete_account(second, second.operator_id())
            .await,
        Err(OperatorError::PersistenceFailed)
    );
    f.component
        .inspect_invitation(invite.token.expose().to_owned())
        .await?;
    f.component
        .inspect_password_reset(issued.token.expose().to_owned())
        .await?;
    assert_eq!(
        f.component
            .authenticate_session(session.wire_credential())
            .await?,
        second
    );
    f.sql("DROP TRIGGER reject_reset_delete").await?;
    reset(&f.component, &issued.token).await?;
    Ok(())
}

struct ReadReady(tokio::sync::Notify);
impl futures_util::task::ArcWake for ReadReady {
    fn wake_by_ref(this: &Arc<Self>) {
        this.0.notify_one();
    }
}

// Park the real public operation after its database snapshot has completed but
// before its async continuation starts hashing. No scheduling sleeps are used.
async fn pause_after_read<F: Future>(database: &Database, mut pending: Pin<&mut F>) -> TestResult {
    let connections = database.test_exhaust_pool();
    let ready = Arc::new(ReadReady(tokio::sync::Notify::new()));
    let waker = futures_util::task::waker(ready.clone());
    assert!(
        pending
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    drop(connections);
    tokio::time::timeout(Duration::from_secs(10), ready.0.notified()).await?;
    Ok(())
}

#[tokio::test]
async fn final_registration_rechecks_expiry_issuer_and_replacement_after_hash_snapshot()
-> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    for event in ["expiry", "demotion", "replacement", "deletion"] {
        let f = Fixture::new().await?;
        let issuer = f.account("issuer", OperatorRole::Admin).await?;
        let invite = f
            .component
            .issue_invitation(issuer, OperatorRole::Viewer)
            .await?;
        let mut pending = Box::pin(register(&f.component, &invite.token, "new"));
        pause_after_read(&f.component.database, pending.as_mut()).await?;
        match event {
            "expiry" => f.sql("UPDATE operator_invitations SET expires_at_unix_ms = CAST(unixepoch('subsec') * 1000 AS INTEGER)").await?,
            "demotion" => f.component.change_role(f.admin, issuer.operator_id(), OperatorRole::Viewer).await?,
            "replacement" => { f.component.regenerate_invitation(f.admin, invite.invitation.invitation_id).await?; },
            _ => f.component.delete_account(f.admin, issuer.operator_id()).await?,
        }
        assert_eq!(pending.await, Err(OperatorError::LinkUnavailable));
        assert!(
            !f.component
                .list_accounts(f.admin)
                .await?
                .iter()
                .any(|account| account.username == "new")
        );
    }
    Ok(())
}

#[tokio::test]
async fn final_reset_rechecks_expiry_issuer_and_replacement_after_hash_snapshot() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    for event in [
        "expiry",
        "demotion",
        "replacement",
        "target_deletion",
        "issuer_deletion",
    ] {
        let f = Fixture::new().await?;
        let issuer = f.account("issuer", OperatorRole::Admin).await?;
        let target = f.account("target", OperatorRole::Viewer).await?;
        let recovery = f
            .component
            .issue_password_reset(issuer, target.operator_id())
            .await?;
        let before = fixtures::test_account_credentials(&f.component.database, "target").await?;
        let mut pending = Box::pin(reset(&f.component, &recovery.token));
        pause_after_read(&f.component.database, pending.as_mut()).await?;
        match event {
            "expiry" => f.sql("UPDATE operator_password_resets SET expires_at_unix_ms = CAST(unixepoch('subsec') * 1000 AS INTEGER)").await?,
            "demotion" => f.component.change_role(f.admin, issuer.operator_id(), OperatorRole::Viewer).await?,
            "replacement" => { f.component.issue_password_reset(f.admin, target.operator_id()).await?; },
            "target_deletion" => f.component.delete_account(f.admin, target.operator_id()).await?,
            _ => f.component.delete_account(f.admin, issuer.operator_id()).await?,
        }
        assert_eq!(pending.await, Err(OperatorError::LinkUnavailable));
        if event != "target_deletion" {
            assert_eq!(
                fixtures::test_account_credentials(&f.component.database, "target").await?,
                before
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn stale_own_password_change_cannot_overwrite_a_newer_reset() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let mut pending = Box::pin(f.component.change_password(
        f.admin,
        PASSWORD.to_owned(),
        NEW_PASSWORD.to_owned(),
        NEW_PASSWORD.to_owned(),
    ));
    pause_after_read(&f.component.database, pending.as_mut()).await?;
    let issued = f
        .component
        .issue_password_reset(f.admin, f.admin.operator_id())
        .await?;
    // Reset to the same old password: checking PHC/password alone would fail to
    // detect this race, whereas the monotonic revision must reject it.
    f.component
        .consume_password_reset(
            issued.token.expose().to_owned(),
            PASSWORD.to_owned(),
            PASSWORD.to_owned(),
        )
        .await?;
    let after_reset = fixtures::test_account_credentials(&f.component.database, "admin").await?;
    assert_eq!(pending.await, Err(OperatorError::CredentialChanged));
    assert_eq!(
        fixtures::test_account_credentials(&f.component.database, "admin").await?,
        after_reset
    );
    Ok(())
}

#[tokio::test]
async fn in_flight_old_sign_ins_are_fenced_by_every_password_replacement_and_deletion() -> TestResult
{
    let _guard = PasswordVerificationTestGuard::acquire().await;
    for action in ["link", "own", "tty", "delete"] {
        let f = Fixture::new().await?;
        let target = f.account("target", OperatorRole::Viewer).await?;
        let mut pending = Box::pin(f.component.sign_in("target", PASSWORD.to_owned()));
        pause_after_read(&f.component.database, pending.as_mut()).await?;
        match action {
            "link" => {
                let link = f
                    .component
                    .issue_password_reset(f.admin, target.operator_id())
                    .await?;
                reset(&f.component, &link.token).await?;
            }
            "own" => {
                f.component
                    .change_password(
                        target,
                        PASSWORD.to_owned(),
                        NEW_PASSWORD.to_owned(),
                        NEW_PASSWORD.to_owned(),
                    )
                    .await?;
            }
            "tty" => {
                let phc = hash_password(&OperatorPassword::new(NEW_PASSWORD.to_owned()))?;
                f.component.reset_password("target", &phc).await?;
            }
            _ => {
                f.component
                    .delete_account(f.admin, target.operator_id())
                    .await?;
            }
        }
        assert_eq!(
            pending.await.err(),
            Some(OperatorError::AuthenticationFailed)
        );
        assert_eq!(
            fixtures::test_session_count(&f.component.database).await?,
            0
        );
    }
    Ok(())
}

#[tokio::test]
async fn all_new_password_operations_share_the_gate_and_cancelled_reads_retain_permits()
-> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let invite = f
        .component
        .issue_invitation(f.admin, OperatorRole::Viewer)
        .await?;
    let link = f
        .component
        .issue_password_reset(f.admin, f.admin.operator_id())
        .await?;
    let held = SIGN_IN_GATE.acquire_many(4).await?;
    assert_eq!(
        register(&f.component, &invite.token, "new").await.err(),
        Some(OperatorError::SignInBusy)
    );
    assert_eq!(
        reset(&f.component, &link.token).await,
        Err(OperatorError::SignInBusy)
    );
    assert_eq!(
        f.component
            .change_password(
                f.admin,
                PASSWORD.to_owned(),
                NEW_PASSWORD.to_owned(),
                NEW_PASSWORD.to_owned()
            )
            .await,
        Err(OperatorError::SignInBusy)
    );
    drop(held);
    let connections = f.component.database.test_exhaust_pool();
    let mut registration = Box::pin(register(&f.component, &invite.token, "new"));
    let mut recovery = Box::pin(reset(&f.component, &link.token));
    let mut change = Box::pin(f.component.change_password(
        f.admin,
        PASSWORD.to_owned(),
        NEW_PASSWORD.to_owned(),
        NEW_PASSWORD.to_owned(),
    ));
    assert!(futures_util::poll!(registration.as_mut()).is_pending());
    assert!(futures_util::poll!(recovery.as_mut()).is_pending());
    assert!(futures_util::poll!(change.as_mut()).is_pending());
    drop((registration, recovery, change));
    assert_eq!(SIGN_IN_GATE.available_permits(), 1);
    drop(connections);
    let restored =
        tokio::time::timeout(Duration::from_secs(10), SIGN_IN_GATE.acquire_many(4)).await??;
    drop(restored);
    assert_eq!(SIGN_IN_GATE.available_permits(), 4);
    Ok(())
}

#[tokio::test]
async fn persisted_links_survive_reconnect_without_renewing_ttl_and_store_only_hashes() -> TestResult
{
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let invite = f
        .component
        .issue_invitation(f.admin, OperatorRole::Viewer)
        .await?;
    let link = f
        .component
        .issue_password_reset(f.admin, f.admin.operator_id())
        .await?;
    let hash = f
        .component
        .database
        .read(|tx| {
            use diesel::QueryDsl;
            crate::diesel_schema::operator_invitations::table
                .select(crate::diesel_schema::operator_invitations::token_hash)
                .first::<Vec<u8>>(tx.connection())
                .map_err(|_| PersistenceError::OperationFailed)
        })
        .await
        .map_err(TransactionError::into_error)?;
    assert_eq!(hash, invite.token.sha256());
    assert_ne!(hash, invite.token.expose().as_bytes());
    let reconnected = OperatorComponent::new(
        Database::connect_and_migrate(&DatabaseConfig::new(
            f.directory.path().join("operators.sqlite"),
            false,
        ))
        .await?,
    );
    assert_eq!(
        reconnected
            .inspect_invitation(invite.token.expose().to_owned())
            .await?,
        invite.invitation
    );
    assert_eq!(
        reconnected
            .inspect_password_reset(link.token.expose().to_owned())
            .await?,
        link.reset
    );
    register(&reconnected, &invite.token, "new").await?;
    assert_eq!(
        f.component
            .inspect_invitation(invite.token.expose().to_owned())
            .await
            .err(),
        Some(OperatorError::LinkUnavailable)
    );
    Ok(())
}

#[tokio::test]
async fn replacement_insert_failures_restore_the_previous_authorizations() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let invite = f
        .component
        .issue_invitation(f.admin, OperatorRole::Viewer)
        .await?;
    let recovery = f
        .component
        .issue_password_reset(f.admin, f.admin.operator_id())
        .await?;
    f.sql("CREATE TRIGGER reject_invitation_insert BEFORE INSERT ON operator_invitations BEGIN SELECT RAISE(ABORT, 'test'); END; CREATE TRIGGER reject_reset_insert BEFORE INSERT ON operator_password_resets BEGIN SELECT RAISE(ABORT, 'test'); END;").await?;
    assert_eq!(
        f.component
            .regenerate_invitation(f.admin, invite.invitation.invitation_id)
            .await
            .err(),
        Some(OperatorError::PersistenceFailed)
    );
    assert_eq!(
        f.component
            .issue_password_reset(f.admin, f.admin.operator_id())
            .await
            .err(),
        Some(OperatorError::PersistenceFailed)
    );
    assert_eq!(
        f.component
            .inspect_invitation(invite.token.expose().to_owned())
            .await?,
        invite.invitation
    );
    assert_eq!(
        f.component
            .inspect_password_reset(recovery.token.expose().to_owned())
            .await?,
        recovery.reset
    );
    assert_eq!(f.component.list_invitations(f.admin).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn deleted_identity_leaves_receipts_bound_to_the_original_uuid() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let target = f.account("target", OperatorRole::Viewer).await?;
    let original_id = target.operator_id().to_string();
    let stored_id = original_id.clone();
    f.component.database.write(move |tx| {
        diesel::sql_query("INSERT INTO target_submission_receipts VALUES ('operation', ?, 'request', 'results')")
            .bind::<diesel::sql_types::Text, _>(stored_id).execute(tx.connection())
            .map(|_| ()).map_err(|_| PersistenceError::OperationFailed)
    }).await.map_err(TransactionError::into_error)?;
    f.component
        .delete_account(f.admin, target.operator_id())
        .await?;
    let invite = f
        .component
        .issue_invitation(f.admin, OperatorRole::Viewer)
        .await?;
    let replacement = register(&f.component, &invite.token, "target").await?;
    assert_ne!(replacement.operator_id(), target.operator_id());
    let receipt = f
        .component
        .database
        .read(|tx| {
            use crate::diesel_schema::target_submission_receipts as receipts;
            use diesel::QueryDsl;
            receipts::table
                .select((
                    receipts::operator_id,
                    receipts::request_json,
                    receipts::results_json,
                ))
                .first::<(String, String, String)>(tx.connection())
                .map_err(|_| PersistenceError::OperationFailed)
        })
        .await
        .map_err(TransactionError::into_error)?;
    assert_eq!(
        receipt,
        (original_id, "request".to_owned(), "results".to_owned())
    );
    Ok(())
}

struct HeldBlocking(Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);
impl Drop for HeldBlocking {
    fn drop(&mut self) {
        *self
            .0
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.0.1.notify_all();
    }
}

#[test]
fn cancelled_queued_hashing_keeps_capacity_until_the_blocking_task_finishes() -> TestResult {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()?;
    runtime.block_on(async {
        let _guard = PasswordVerificationTestGuard::acquire().await;
        let held = HeldBlocking(Arc::new((
            std::sync::Mutex::new(false),
            std::sync::Condvar::new(),
        )));
        let gate = held.0.clone();
        let (entered, started) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = entered.send(());
            let released = gate
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            drop(
                gate.1
                    .wait_while(released, |released| !*released)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
        });
        tokio::time::timeout(Duration::from_secs(10), started).await??;
        let permit = password_permit()?;
        let mut hashing = Box::pin(hash_new_password(
            OperatorPassword::new(PASSWORD.to_owned()),
            permit,
        ));
        assert!(futures_util::poll!(hashing.as_mut()).is_pending());
        drop(hashing);
        assert_eq!(SIGN_IN_GATE.available_permits(), 3);
        drop(held);
        blocker.await?;
        let restored =
            tokio::time::timeout(Duration::from_secs(10), SIGN_IN_GATE.acquire_many(4)).await??;
        drop(restored);
        Ok(())
    })
}

#[tokio::test]
async fn grant_issuance_racing_issuer_demotion_cannot_leave_usable_links() -> TestResult {
    let _guard = PasswordVerificationTestGuard::acquire().await;
    let f = Fixture::new().await?;
    let issuer = f.account("issuer", OperatorRole::Admin).await?;
    let barrier = Arc::new(Barrier::new(3));
    let issue_invite = async {
        barrier.wait().await;
        f.component
            .issue_invitation(issuer, OperatorRole::Viewer)
            .await
    };
    let issue_reset = async {
        barrier.wait().await;
        f.component
            .issue_password_reset(issuer, f.admin.operator_id())
            .await
    };
    let demote = async {
        barrier.wait().await;
        f.component
            .change_role(f.admin, issuer.operator_id(), OperatorRole::Viewer)
            .await
    };
    let (invite, recovery, demotion) = tokio::join!(issue_invite, issue_reset, demote);
    demotion?;
    match invite {
        Ok(invite) => assert_eq!(
            f.component
                .inspect_invitation(invite.token.expose().to_owned())
                .await
                .err(),
            Some(OperatorError::LinkUnavailable)
        ),
        Err(error) => assert_eq!(error, OperatorError::AuthorizationDenied),
    }
    match recovery {
        Ok(recovery) => assert_eq!(
            f.component
                .inspect_password_reset(recovery.token.expose().to_owned())
                .await
                .err(),
            Some(OperatorError::LinkUnavailable)
        ),
        Err(error) => assert_eq!(error, OperatorError::AuthorizationDenied),
    }
    assert!(f.component.list_invitations(f.admin).await?.is_empty());
    // Reusing the pre-demotion authenticated identity cannot sign new grants.
    assert_eq!(
        f.component
            .issue_invitation(issuer, OperatorRole::Viewer)
            .await
            .err(),
        Some(OperatorError::AuthorizationDenied)
    );
    assert_eq!(
        f.component
            .issue_password_reset(issuer, f.admin.operator_id())
            .await
            .err(),
        Some(OperatorError::AuthorizationDenied)
    );
    Ok(())
}
