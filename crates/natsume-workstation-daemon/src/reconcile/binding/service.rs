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
        matches!(proxy.validate_waiting_session(session).await, Ok(true))
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

#[cfg(test)]
mod tests {
    use std::{sync::Mutex, time::Duration};

    use natsume_workstation_ipc::{
        ManagedSessionsObservation, PRIVILEGED1_PATH, ResourceControlError,
    };

    use super::*;

    #[derive(Clone, Copy)]
    enum IdentityReply {
        Accept,
        Reject,
        Error,
        Timeout,
    }

    struct HelperCalls {
        identity: usize,
        full: usize,
        reply: IdentityReply,
    }

    struct IdentityHelper(Arc<Mutex<HelperCalls>>);
    struct LegacyHelper(Arc<Mutex<HelperCalls>>);

    #[zbus::interface(name = "org.natsume.Privileged1")]
    impl IdentityHelper {
        async fn validate_waiting_session(
            &self,
            session: GraphicalSession,
        ) -> Result<bool, ResourceControlError> {
            assert_eq!(session.logind_session_id, "w1");
            assert_eq!(session.boot_id, "550e8400-e29b-41d4-a716-446655440000");
            let reply = {
                let mut calls = self
                    .0
                    .lock()
                    .unwrap_or_else(|error| panic!("helper calls: {error}"));
                calls.identity += 1;
                calls.reply
            };
            match reply {
                IdentityReply::Accept => Ok(true),
                IdentityReply::Reject => Ok(false),
                IdentityReply::Error => Err(ResourceControlError::Unavailable(
                    "fixture identity failure".to_owned(),
                )),
                IdentityReply::Timeout => std::future::pending().await,
            }
        }

        fn query_managed_sessions(
            &self,
        ) -> Result<ManagedSessionsObservation, ResourceControlError> {
            self.0
                .lock()
                .unwrap_or_else(|error| panic!("helper calls: {error}"))
                .full += 1;
            Err(ResourceControlError::Unavailable(
                "unexpected full query".to_owned(),
            ))
        }
    }

    #[zbus::interface(name = "org.natsume.Privileged1")]
    impl LegacyHelper {
        fn query_managed_sessions(
            &self,
        ) -> Result<ManagedSessionsObservation, ResourceControlError> {
            self.0
                .lock()
                .unwrap_or_else(|error| panic!("helper calls: {error}"))
                .full += 1;
            Err(ResourceControlError::Unavailable(
                "unexpected legacy query".to_owned(),
            ))
        }
    }

    #[tokio::test]
    async fn exact_identity_uses_only_the_narrow_method_and_rejects_missing_or_failed_checks() {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| panic!("service fixture: {error}"));
        let calls = Arc::new(Mutex::new(HelperCalls {
            identity: 0,
            full: 0,
            reply: IdentityReply::Accept,
        }));
        let (server_stream, client_stream) =
            tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("service peer: {error}"));
        let server_builder = zbus::connection::Builder::unix_stream(
            server_stream
                .into_std()
                .unwrap_or_else(|error| panic!("server stream: {error}")),
        )
        .server(zbus::Guid::generate())
        .p2p()
        .serve_at(PRIVILEGED1_PATH, IdentityHelper(Arc::clone(&calls)));
        let client_builder = zbus::connection::Builder::unix_stream(
            client_stream
                .into_std()
                .unwrap_or_else(|error| panic!("client stream: {error}")),
        )
        .p2p()
        .method_timeout(Duration::from_millis(100));
        let (server, client) = tokio::join!(server_builder.build(), client_builder.build());
        let server = server.unwrap_or_else(|error| panic!("helper server: {error}"));
        let service = DeviceService {
            provider: Arc::new(super::super::tests::input_provider(&directory)),
            privileged_connection: client.unwrap_or_else(|error| panic!("helper client: {error}")),
        };
        let session = GraphicalSession {
            logind_session_id: "w1".to_owned(),
            boot_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
        };
        for reply in [
            IdentityReply::Accept,
            IdentityReply::Reject,
            IdentityReply::Error,
            IdentityReply::Timeout,
        ] {
            calls
                .lock()
                .unwrap_or_else(|error| panic!("helper calls: {error}"))
                .reply = reply;
            assert_eq!(
                service.exact_current_session(&session).await,
                matches!(reply, IdentityReply::Accept)
            );
        }
        let objects = server.object_server();
        objects
            .remove::<IdentityHelper, _>(PRIVILEGED1_PATH)
            .await
            .unwrap_or_else(|error| panic!("remove current helper: {error}"));
        objects
            .at(PRIVILEGED1_PATH, LegacyHelper(Arc::clone(&calls)))
            .await
            .unwrap_or_else(|error| panic!("legacy helper: {error}"));
        assert!(!service.exact_current_session(&session).await);
        let calls = calls
            .lock()
            .unwrap_or_else(|error| panic!("helper calls: {error}"));
        assert_eq!(calls.identity, 4);
        assert_eq!(calls.full, 0);
    }
}
