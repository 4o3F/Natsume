use tokio::sync::SemaphorePermit;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{
    OperatorComponent, OperatorError, OperatorIdentity, OperatorRole,
    account::{AccountFacts, replace_password},
    credentials::{validate_input, validate_new_login_name, validate_new_password},
    db::{self, links, management as store},
    link::{LinkKind, LinkToken},
    password::{OperatorPassword, hash_password, verify_password_once},
    session::SIGN_IN_GATE,
};
use crate::db::{Transaction, TransactionError};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "TODO(P3): expose the account list response")
)]
pub(crate) struct OperatorSummary {
    pub(crate) operator_id: Uuid,
    pub(crate) username: String,
    pub(crate) role: OperatorRole,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvitationSummary {
    pub(crate) invitation_id: Uuid,
    pub(crate) role: OperatorRole,
    pub(crate) issuer_operator_id: Uuid,
    pub(crate) created_at_unix_ms: i64,
    pub(crate) expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PasswordResetInspection {
    pub(crate) operator_id: Uuid,
    pub(crate) reset_id: Uuid,
    pub(crate) username: String,
    pub(crate) expires_at_unix_ms: i64,
}

#[derive(Debug)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "TODO(P3): return the invitation secret once")
)]
pub(crate) struct IssuedInvitation {
    pub(crate) invitation: InvitationSummary,
    pub(crate) token: LinkToken,
}

#[derive(Debug)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "TODO(P3): return the password-reset secret once")
)]
pub(crate) struct IssuedPasswordReset {
    pub(crate) reset: PasswordResetInspection,
    pub(crate) token: LinkToken,
}

const INVITATION_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const RESET_TTL_MS: i64 = 60 * 60 * 1000;

// The HTTP adapter is the next phase. Only these external domain entrypoints
// await consumers; all persistence and transactional helpers are private.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "TODO(P3): connect the operator management HTTP handlers"
    )
)]
impl OperatorComponent {
    pub(crate) async fn list_accounts(
        &self,
        actor: OperatorIdentity,
    ) -> Result<Vec<OperatorSummary>, OperatorError> {
        actor.require_admin()?;
        self.database
            .read(store::list_accounts)
            .await
            .map_err(TransactionError::into_error)
            .map_err(OperatorError::from)
    }

    pub(crate) async fn change_role(
        &self,
        actor: OperatorIdentity,
        target: Uuid,
        role: OperatorRole,
    ) -> Result<(), OperatorError> {
        actor.require_admin()?;
        self.database
            .write(move |tx| {
                store::find_account_by_id(tx, target)?.ok_or(OperatorError::OperatorNotFound)?;
                if role == OperatorRole::Viewer {
                    store::protect_last_admin(tx, target)?;
                    links::revoke_issued_links(tx, target)?;
                }
                store::update_role(tx, target, role)?;
                Ok(())
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn delete_account(
        &self,
        actor: OperatorIdentity,
        target: Uuid,
    ) -> Result<(), OperatorError> {
        actor.require_admin()?;
        self.database
            .write(move |tx| {
                store::protect_last_admin(tx, target)?;
                // Foreign keys revoke sessions and both kinds of link. Receipts and
                // competition facts intentionally have no cascading account FK.
                store::delete_account(tx, target)?;
                Ok(())
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn list_invitations(
        &self,
        actor: OperatorIdentity,
    ) -> Result<Vec<InvitationSummary>, OperatorError> {
        actor.require_admin()?;
        self.database
            .read(links::list_invitations)
            .await
            .map_err(TransactionError::into_error)
            .map_err(OperatorError::from)
    }

    pub(crate) async fn issue_invitation(
        &self,
        actor: OperatorIdentity,
        role: OperatorRole,
    ) -> Result<IssuedInvitation, OperatorError> {
        actor.require_admin()?;
        let token = mint(LinkKind::Invitation)?;
        let hash = Zeroizing::new(token.sha256());
        let invitation = self
            .database
            .write(move |tx| {
                current_admin(tx, actor.operator_id())?;
                create_invitation(tx, actor.operator_id(), role, &hash)
            })
            .await
            .map_err(TransactionError::into_error)?;
        Ok(IssuedInvitation { invitation, token })
    }

    pub(crate) async fn revoke_invitation(
        &self,
        actor: OperatorIdentity,
        id: Uuid,
    ) -> Result<(), OperatorError> {
        actor.require_admin()?;
        self.database
            .write(move |tx| {
                // Repeated revocation is an idempotent no-op.
                links::delete_invitation(tx, id).map_err(OperatorError::from)
            })
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn regenerate_invitation(
        &self,
        actor: OperatorIdentity,
        id: Uuid,
    ) -> Result<IssuedInvitation, OperatorError> {
        actor.require_admin()?;
        let token = mint(LinkKind::Invitation)?;
        let hash = Zeroizing::new(token.sha256());
        let invitation = self
            .database
            .write(move |tx| {
                current_admin(tx, actor.operator_id())?;
                let previous = links::invitation_by_id(tx, id)?;
                links::delete_invitation(tx, id)?;
                // A fresh authorization is signed by the administrator performing
                // the regeneration, with the original immutable role.
                create_invitation(tx, actor.operator_id(), previous.role, &hash)
            })
            .await
            .map_err(TransactionError::into_error)?;
        Ok(IssuedInvitation { invitation, token })
    }

    pub(crate) async fn inspect_invitation(
        &self,
        wire_token: String,
    ) -> Result<InvitationSummary, OperatorError> {
        let token = parse(LinkKind::Invitation, wire_token)?;
        let hash = Zeroizing::new(token.sha256());
        self.database
            .read(move |tx| valid_invitation(tx, &hash))
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn register(
        &self,
        wire_token: String,
        username: String,
        password: String,
        confirmation: String,
    ) -> Result<OperatorIdentity, OperatorError> {
        let token = parse(LinkKind::Invitation, wire_token);
        let password = confirmed_password(password, confirmation);
        let token = token?;
        let password = password?;
        validate_new_login_name(&username)?;
        let permit = password_permit()?;
        let hash = Zeroizing::new(token.sha256());
        let permit = self
            .database
            .read(move |tx| valid_invitation(tx, &hash).map(|_| permit))
            .await
            .map_err(TransactionError::into_error)?;
        let (password_hash, permit) = hash_new_password(password, permit).await?;
        let hash = Zeroizing::new(token.sha256());
        self.database
            .write(move |tx| {
                let identity = register_hashed(tx, &hash, &username, &password_hash)?;
                Ok((identity, permit))
            })
            .await
            .map(|(identity, _)| identity)
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn issue_password_reset(
        &self,
        actor: OperatorIdentity,
        target: Uuid,
    ) -> Result<IssuedPasswordReset, OperatorError> {
        actor.require_admin()?;
        let token = mint(LinkKind::PasswordReset)?;
        let hash = Zeroizing::new(token.sha256());
        let reset = self
            .database
            .write(move |tx| {
                current_admin(tx, actor.operator_id())?;
                let created = links::now(tx)?;
                let reset = PasswordResetInspection {
                    operator_id: target,
                    reset_id: Uuid::now_v7(),
                    username: store::account_name(tx, target)?,
                    expires_at_unix_ms: created
                        .checked_add(RESET_TTL_MS)
                        .ok_or(OperatorError::PersistenceFailed)?,
                };
                links::insert_reset(tx, &reset, actor.operator_id(), &hash, created)?;
                Ok::<_, OperatorError>(reset)
            })
            .await
            .map_err(TransactionError::into_error)?;
        Ok(IssuedPasswordReset { reset, token })
    }

    pub(crate) async fn inspect_password_reset(
        &self,
        wire_token: String,
    ) -> Result<PasswordResetInspection, OperatorError> {
        let token = parse(LinkKind::PasswordReset, wire_token)?;
        let hash = Zeroizing::new(token.sha256());
        self.database
            .read(move |tx| valid_reset(tx, &hash))
            .await
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn consume_password_reset(
        &self,
        wire_token: String,
        password: String,
        confirmation: String,
    ) -> Result<(), OperatorError> {
        let token = parse(LinkKind::PasswordReset, wire_token);
        let password = confirmed_password(password, confirmation);
        let token = token?;
        let password = password?;
        let permit = password_permit()?;
        let hash = Zeroizing::new(token.sha256());
        let permit = self
            .database
            .read(move |tx| valid_reset(tx, &hash).map(|_| permit))
            .await
            .map_err(TransactionError::into_error)?;
        let (password_hash, permit) = hash_new_password(password, permit).await?;
        let hash = Zeroizing::new(token.sha256());
        self.database
            .write(move |tx| {
                reset_hashed(tx, &hash, &password_hash)?;
                Ok(permit)
            })
            .await
            .map(|_| ())
            .map_err(TransactionError::into_error)
    }

    pub(crate) async fn change_password(
        &self,
        actor: OperatorIdentity,
        current_password: String,
        new_password: String,
        confirmation: String,
    ) -> Result<(), OperatorError> {
        let current_password = OperatorPassword::new(current_password);
        let new_password = confirmed_password(new_password, confirmation)?;
        // Existing current passwords retain the old permissive byte policy.
        validate_input("current", current_password.expose())?;
        let permit = password_permit()?;
        let (account, permit) = self
            .database
            .read(move |tx| {
                let account = store::find_account_by_id(tx, actor.operator_id())?
                    .ok_or(OperatorError::SessionAuthenticationFailed)?;
                Ok::<_, OperatorError>((account, permit))
            })
            .await
            .map_err(TransactionError::into_error)?;
        let revision = account.credential_revision;
        let (password_hash, permit) = tokio::task::spawn_blocking(move || {
            if !verify_password_once(&current_password, &account.password_hash)? {
                return Err(OperatorError::AuthenticationFailed);
            }
            hash_password(&new_password).map(|hash| (hash, permit))
        })
        .await
        .map_err(|_| OperatorError::PasswordTaskFailed)??;
        self.database
            .write(move |tx| {
                change_password_hashed(tx, actor.operator_id(), revision, &password_hash)?;
                Ok(permit)
            })
            .await
            .map(|_| ())
            .map_err(TransactionError::into_error)
    }
}

fn confirmed_password(
    password: String,
    confirmation: String,
) -> Result<OperatorPassword, OperatorError> {
    let password = OperatorPassword::new(password);
    let confirmation = OperatorPassword::new(confirmation);
    validate_new_password(password.expose())?;
    if password.expose() != confirmation.expose() {
        return Err(OperatorError::PasswordMismatch);
    }
    Ok(password)
}

fn mint(kind: LinkKind) -> Result<LinkToken, OperatorError> {
    LinkToken::generate(kind).map_err(|_| OperatorError::EntropyUnavailable)
}

fn parse(kind: LinkKind, token: String) -> Result<LinkToken, OperatorError> {
    LinkToken::from_wire(kind, token).ok_or(OperatorError::LinkUnavailable)
}

fn password_permit() -> Result<SemaphorePermit<'static>, OperatorError> {
    SIGN_IN_GATE
        .try_acquire()
        .map_err(|_| OperatorError::SignInBusy)
}

async fn hash_new_password(
    password: OperatorPassword,
    permit: SemaphorePermit<'static>,
) -> Result<(String, SemaphorePermit<'static>), OperatorError> {
    tokio::task::spawn_blocking(move || hash_password(&password).map(|hash| (hash, permit)))
        .await
        .map_err(|_| OperatorError::PasswordTaskFailed)?
}

fn current_admin(tx: &mut Transaction<'_>, id: Uuid) -> Result<(), OperatorError> {
    store::find_account_by_id(tx, id)?
        .ok_or(OperatorError::AuthorizationDenied)?
        .identity
        .require_admin()
}

fn create_invitation(
    tx: &mut Transaction<'_>,
    issuer: Uuid,
    role: OperatorRole,
    hash: &[u8; 32],
) -> Result<InvitationSummary, OperatorError> {
    let created = links::now(tx)?;
    let invitation = InvitationSummary {
        invitation_id: Uuid::now_v7(),
        role,
        issuer_operator_id: issuer,
        created_at_unix_ms: created,
        expires_at_unix_ms: created
            .checked_add(INVITATION_TTL_MS)
            .ok_or(OperatorError::PersistenceFailed)?,
    };
    links::insert_invitation(tx, &invitation, hash)?;
    Ok(invitation)
}

fn valid_invitation(
    tx: &mut Transaction<'_>,
    hash: &[u8; 32],
) -> Result<InvitationSummary, OperatorError> {
    let invitation = links::find_invitation(tx, hash)?.ok_or(OperatorError::LinkUnavailable)?;
    if invitation.expires_at_unix_ms <= links::now(tx)? {
        return Err(OperatorError::LinkUnavailable);
    }
    current_admin(tx, invitation.issuer_operator_id).map_err(|error| match error {
        OperatorError::AuthorizationDenied => OperatorError::LinkUnavailable,
        other => other,
    })?;
    Ok(invitation)
}

fn valid_reset(
    tx: &mut Transaction<'_>,
    hash: &[u8; 32],
) -> Result<PasswordResetInspection, OperatorError> {
    let (reset, issuer) = links::find_reset(tx, hash)?.ok_or(OperatorError::LinkUnavailable)?;
    if reset.expires_at_unix_ms <= links::now(tx)? {
        return Err(OperatorError::LinkUnavailable);
    }
    current_admin(tx, issuer).map_err(|error| match error {
        OperatorError::AuthorizationDenied => OperatorError::LinkUnavailable,
        other => other,
    })?;
    Ok(reset)
}

fn register_hashed(
    tx: &mut Transaction<'_>,
    hash: &[u8; 32],
    username: &str,
    password_hash: &str,
) -> Result<OperatorIdentity, OperatorError> {
    let invitation = valid_invitation(tx, hash)?;
    // BEGIN IMMEDIATE serializes the conflict check and insert, without
    // misclassifying unrelated persistence failures as username conflicts.
    if db::find_account(tx, username)?.is_some() {
        return Err(OperatorError::LoginNameConflict);
    }
    let identity = OperatorIdentity {
        operator_id: Uuid::now_v7(),
        role: invitation.role,
    };
    db::insert_account(
        tx,
        identity.operator_id(),
        username,
        identity.role,
        password_hash,
    )?;
    links::delete_invitation(tx, invitation.invitation_id)?;
    Ok(identity)
}

fn reset_hashed(
    tx: &mut Transaction<'_>,
    hash: &[u8; 32],
    password_hash: &str,
) -> Result<(), OperatorError> {
    let reset = valid_reset(tx, hash)?;
    let account =
        store::find_account_by_id(tx, reset.operator_id)?.ok_or(OperatorError::LinkUnavailable)?;
    replace_password(tx, &account, password_hash)
}

fn change_password_hashed(
    tx: &mut Transaction<'_>,
    target: Uuid,
    expected_revision: i64,
    password_hash: &str,
) -> Result<(), OperatorError> {
    let account: AccountFacts =
        store::find_account_by_id(tx, target)?.ok_or(OperatorError::CredentialChanged)?;
    if account.credential_revision != expected_revision {
        return Err(OperatorError::CredentialChanged);
    }
    replace_password(tx, &account, password_hash)
}

#[cfg(test)]
mod tests;
