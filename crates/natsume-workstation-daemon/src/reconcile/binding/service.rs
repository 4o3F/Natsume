use std::sync::Arc;

use natsume_workstation_ipc::{
    BindingSubmission, DEVICE1_PATH, DEVICE1_SERVICE, GraphicalSession, GraphicalSessionState,
    Privileged1Proxy, SessionAgentLease, SessionPresentation, SessionUiSnapshot,
};
use uuid::Uuid;
use zbus::message::Header;

use super::{
    BindingInputProvider, Registration, SnapshotError, caller, service_error, unix_time_ms,
};

pub(in crate::reconcile) struct DeviceService {
    provider: Arc<BindingInputProvider>,
    privileged_connection: zbus::Connection,
}

impl DeviceService {
    pub(in crate::reconcile) async fn start(
        connection: &zbus::Connection,
        provider: Arc<BindingInputProvider>,
    ) -> Result<(), SnapshotError> {
        connection
            .object_server()
            .at(
                DEVICE1_PATH,
                Self {
                    provider,
                    privileged_connection: connection.clone(),
                },
            )
            .await
            .map_err(|_| SnapshotError::LocalControl)?;
        connection
            .request_name(DEVICE1_SERVICE)
            .await
            .map_err(|_| SnapshotError::LocalControl)?;
        Ok(())
    }

    async fn exact_current_session(&self, session: &GraphicalSession) -> bool {
        let Ok(proxy) = Privileged1Proxy::new(&self.privileged_connection).await else {
            return false;
        };
        matches!(
            proxy.query_managed_sessions().await,
            Ok(observation)
                if matches!(
                    observation.waiting.state,
                    GraphicalSessionState::Running
                ) && observation.waiting.session.as_ref() == Some(session)
        )
    }

    async fn authenticated(
        &self,
        header: &Header<'_>,
        session: &GraphicalSession,
    ) -> zbus::fdo::Result<caller::Caller> {
        let caller = caller::authenticate(&self.privileged_connection, header, session)
            .await
            .ok_or_else(|| service_error("Session Agent caller was rejected"))?;
        if !self.exact_current_session(session).await {
            return Err(service_error("Session Agent session is stale"));
        }
        Ok(caller)
    }
}

#[zbus::interface(name = "org.natsume.Device1")]
impl DeviceService {
    #[zbus(name = "RegisterSessionAgent")]
    async fn register_session_agent(
        &self,
        protocol_version: u32,
        session: GraphicalSession,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<(SessionAgentLease, SessionUiSnapshot)> {
        if protocol_version != natsume_workstation_ipc::SESSION_AGENT_PROTOCOL_VERSION {
            return Err(zbus::fdo::Error::InvalidArgs(
                "incompatible Session Agent protocol: expected version 3".into(),
            ));
        }
        let caller = self.authenticated(&header, &session).await?;
        let mut snapshot = self
            .provider
            .ui_snapshot(session.clone())
            .map_err(|_| service_error("Session UI state is unavailable"))?;
        let lease = SessionAgentLease {
            lease_id: Uuid::now_v7().hyphenated().to_string(),
            session,
            expires_at_unix_ms: unix_time_ms().saturating_add(15_000),
        };
        let mut registered = self
            .provider
            .registered
            .lock()
            .map_err(|_| service_error("Session Agent state is unavailable"))?;
        snapshot.agent_lease_id.clone_from(&lease.lease_id);
        *registered = Some(Registration {
            lease: lease.clone(),
            caller,
            presentation: None,
        });
        self.provider.changed.notify_one();
        Ok((lease, snapshot))
    }

    #[zbus(name = "RenewSessionAgentLease")]
    async fn renew_session_agent_lease(
        &self,
        lease_id: &str,
        session: GraphicalSession,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<SessionAgentLease> {
        let caller = self.authenticated(&header, &session).await?;
        let mut registered = self
            .provider
            .registered
            .lock()
            .map_err(|_| service_error("Session Agent state is unavailable"))?;
        let registration = registered
            .as_mut()
            .filter(|registration| registration.matches(&caller, lease_id, &session))
            .ok_or_else(|| service_error("Session Agent lease is stale"))?;
        registration.lease.expires_at_unix_ms = unix_time_ms().saturating_add(15_000);
        Ok(registration.lease.clone())
    }

    #[zbus(name = "GetSessionUiSnapshot")]
    async fn get_session_ui_snapshot(
        &self,
        lease_id: &str,
        session: GraphicalSession,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<SessionUiSnapshot> {
        let caller = self.authenticated(&header, &session).await?;
        let registered = self
            .provider
            .registered
            .lock()
            .map_err(|_| service_error("Session Agent state is unavailable"))?;
        if !registered
            .as_ref()
            .is_some_and(|r| r.matches(&caller, lease_id, &session))
        {
            return Err(service_error("Session Agent lease is stale"));
        }
        self.provider
            .ui_snapshot(session)
            .map(|mut snapshot| {
                lease_id.clone_into(&mut snapshot.agent_lease_id);
                snapshot
            })
            .map_err(|_| service_error("Session UI state is unavailable"))
    }

    #[zbus(name = "ConfirmSessionPresentation")]
    async fn confirm_session_presentation(
        &self,
        lease_id: &str,
        presentation: SessionPresentation,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        if presentation.agent_lease_id != lease_id {
            return Err(service_error(
                "Session frame belongs to a different Agent lease",
            ));
        }
        let caller = self.authenticated(&header, &presentation.session).await?;
        let mut registered = self
            .provider
            .registered
            .lock()
            .map_err(|_| service_error("Session Agent state is unavailable"))?;
        let registration = registered
            .as_mut()
            .filter(|r| r.matches(&caller, lease_id, &presentation.session))
            .ok_or_else(|| service_error("Session Agent lease is stale"))?;
        let snapshot = self
            .provider
            .ui_snapshot(presentation.session.clone())
            .map_err(|_| service_error("Session UI state is unavailable"))?;
        let result = registration.confirm(presentation, snapshot.ui_revision);
        self.provider.changed.notify_one();
        result
    }

    #[zbus(name = "SubmitBinding")]
    async fn submit_binding(
        &self,
        lease_id: &str,
        submission: BindingSubmission,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let caller = self.authenticated(&header, &submission.session).await?;
        let proxy = Privileged1Proxy::new(&self.privileged_connection)
            .await
            .map_err(|_| service_error("waiting foreground is unavailable"))?;
        let observed = proxy
            .query_managed_sessions()
            .await
            .map_err(|_| service_error("waiting foreground is unavailable"))?;
        if !proxy.is_home_ready().await.unwrap_or(false)
            || observed.contest.state != GraphicalSessionState::Running
            || !observed.contest.desktop_ready
            || observed.contest.locked_hint
            || observed.foreground != natsume_workstation_ipc::SessionForeground::Waiting
            || observed.waiting.session.as_ref() != Some(&submission.session)
            || observed.waiting.locked_hint
            || !observed.waiting.desktop_ready
        {
            return Err(service_error(
                "Binding requires the ready waiting foreground",
            ));
        }
        let registered = self
            .provider
            .registered
            .lock()
            .map_err(|_| service_error("Session Agent state is unavailable"))?;
        let snapshot = self
            .provider
            .ui_snapshot(submission.session.clone())
            .map_err(|_| service_error("Session UI state is unavailable"))?;
        if !registered.as_ref().is_some_and(|r| {
            r.matches(&caller, lease_id, &submission.session)
                && r.presentation
                    .as_ref()
                    .is_some_and(|p| p.ui_revision == snapshot.ui_revision)
        }) {
            return Err(service_error("Binding presentation is stale"));
        }
        let result = self.provider.submit(
            &submission.negotiation_id,
            submission.submission_epoch,
            &submission.seat_code,
        );
        drop(registered);
        result.map_err(|_| service_error("Binding submission was rejected"))
    }
}
