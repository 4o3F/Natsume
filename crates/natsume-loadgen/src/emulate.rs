use std::time::{Duration, SystemTime, UNIX_EPOCH};

use natsume_workstation_protocol::generated::{
    ActualState, BindingAccessActualState, BindingArtifactState, BindingInput, ClientInputState,
    ClientStateSnapshot, ForegroundTarget, GatewayActualState, GatewayCredentialInput,
    GatewayState, HomeActualState, HomeState, PowerControlActualState, PowerState,
    RuntimeConfigActualState, RuntimeConfigState, ServerStateSnapshot, SessionControlActualState,
    SessionForeground, SessionState,
};
use prost::Message as _;
use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use sha2::{Digest as _, Sha256};
use snafu::{OptionExt as _, ResultExt as _};
use tokio::time::Instant;
use x509_parser::prelude::{FromDer as _, X509Certificate};
use zeroize::Zeroizing;

use crate::{
    cli::Run,
    error::{self, Error, Result},
    identity::{Gateway, Identity, Samples},
};

#[derive(Clone, Copy)]
pub(crate) enum Resource {
    Gateway,
    Binding,
    Runtime,
    Session,
    Home,
    Power,
}
const RESOURCES: [Resource; 6] = [
    Resource::Gateway,
    Resource::Binding,
    Resource::Runtime,
    Resource::Session,
    Resource::Home,
    Resource::Power,
];

impl Resource {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Gateway => "GATEWAY",
            Self::Binding => "BINDING",
            Self::Runtime => "RUNTIME",
            Self::Session => "SESSION",
            Self::Home => "HOME",
            Self::Power => "POWER",
        }
    }

    fn delay(self, run: &Run) -> Duration {
        match self {
            Self::Gateway => run.gateway_delay,
            Self::Binding => run.binding_delay,
            Self::Runtime => run.runtime_delay,
            Self::Session => run.session_delay,
            Self::Home => run.home_delay,
            Self::Power => run.power_delay,
        }
        .unwrap_or(run.resource_delay)
    }
}

struct Pending {
    due: Instant,
}

pub(crate) struct Emulator {
    pub(crate) identity: Identity,
    pub(crate) snapshot: ClientStateSnapshot,
    target: Option<ServerStateSnapshot>,
    keys: [Option<[u8; 32]>; 6],
    pending: [Option<Pending>; 6],
    samples: Samples,
    received: [Option<Instant>; 6],
}

impl Emulator {
    pub(crate) fn new(identity: Identity, seed: u64, index: usize) -> Result<Self> {
        let snapshot = if identity.state.local.is_empty() {
            initial()
        } else {
            ClientStateSnapshot::decode(identity.state.local.as_slice())
                .context(error::DecodeSnafu)?
        };
        let mut emulator = Self {
            identity,
            snapshot,
            target: None,
            keys: [None; 6],
            pending: std::array::from_fn(|_| None),
            samples: Samples::new(seed, index, "resources"),
            received: [None; 6],
        };
        emulator.deactivate()?;
        Ok(emulator)
    }

    pub(crate) fn deactivate(&mut self) -> Result<()> {
        // Caddy credentials are volatile; disconnect blocks access while durable input survives.
        if let Some(actual) = self.snapshot.actual.as_mut() {
            if let Some(gateway) = actual.gateway.as_mut()
                && gateway.gateway_leaf_sha256.is_some()
            {
                gateway.state = GatewayState::Blocked.into();
            }
            actual.power = Some(PowerControlActualState {
                state: if self.identity.state.power_accepted_epoch.is_some() {
                    PowerState::Accepted
                } else {
                    PowerState::Idle
                }
                .into(),
                attempted_shutdown_epoch: self.identity.state.power_accepted_epoch,
            });
            if let Some(binding) = actual.binding_access.as_mut() {
                binding.credential_state = BindingArtifactState::Absent.into();
                binding.context = None;
            }
        }
        self.target = None;
        self.keys = [None; 6];
        self.pending = std::array::from_fn(|_| None);
        self.persist()
    }

    pub(crate) fn accept(&mut self, target: ServerStateSnapshot, run: &Run) -> Result<()> {
        crate::target::validate(&target)?;
        let concrete = target.target.as_ref().context(error::PeerSnafu {
            code: "TARGET_MISSING",
        })?;
        let intent = target.intent.as_ref().context(error::PeerSnafu {
            code: "INTENT_MISSING",
        })?;
        let fingerprints = [
            concrete.gateway.as_ref().map(prost::Message::encode_to_vec),
            concrete.binding_access.as_ref().map(|binding| {
                let mut bytes = binding.encode_to_vec();
                if let Some(intent) = &intent.binding {
                    bytes.extend(intent.encode_to_vec());
                }
                bytes
            }),
            concrete
                .runtime_config
                .as_ref()
                .map(prost::Message::encode_to_vec),
            concrete
                .session_control
                .as_ref()
                .map(prost::Message::encode_to_vec),
            concrete.home.as_ref().map(prost::Message::encode_to_vec),
            concrete.power.as_ref().map(prost::Message::encode_to_vec),
        ];
        for (index, bytes) in fingerprints.into_iter().enumerate() {
            let bytes = Zeroizing::new(bytes.context(error::PeerSnafu {
                code: "PARTIAL_TARGET",
            })?);
            let key: [u8; 32] = Sha256::digest(&bytes).into();
            if self.keys[index] != Some(key) {
                self.keys[index] = Some(key);
                let received = Instant::now();
                self.received[index] = Some(received);
                let jitter = self.samples.uniform(millis(run.resource_jitter));
                self.pending[index] = Some(Pending {
                    due: received + RESOURCES[index].delay(run) + Duration::from_millis(jitter),
                });
            }
        }
        self.target = Some(target);
        Ok(())
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.pending
            .iter()
            .flatten()
            .map(|pending| pending.due)
            .min()
    }

    pub(crate) fn organization(&self) -> Option<&str> {
        self.target
            .as_ref()?
            .target
            .as_ref()?
            .binding_access
            .as_ref()?
            .bound
            .as_ref()?
            .presentation
            .as_ref()
            .map(|value| value.organization_id.as_str())
    }

    /// Apply one due local resource at a time; the control pump stays available for heartbeat traffic.
    pub(crate) async fn apply(&mut self) -> Result<()> {
        for (index, resource) in RESOURCES.into_iter().enumerate() {
            if self.pending[index]
                .as_ref()
                .is_some_and(|pending| pending.due <= Instant::now())
            {
                self.apply_resource(resource).await?;
                self.pending[index] = None;
            }
        }
        self.access_and_input()?;
        self.persist()
    }

    pub(crate) fn changes(&self, before: &ClientStateSnapshot) -> Vec<(Resource, Instant)> {
        let (Some(old), Some(new), Some(old_input), Some(new_input)) = (
            &before.actual,
            &self.snapshot.actual,
            &before.input,
            &self.snapshot.input,
        ) else {
            return Vec::new();
        };
        let changed = [
            old.gateway != new.gateway
                || old_input.gateway_credential != new_input.gateway_credential,
            old.binding_access != new.binding_access || old_input.binding != new_input.binding,
            old.runtime_config != new.runtime_config,
            old.session_control != new.session_control,
            old.home != new.home,
            old.power != new.power,
        ];
        changed
            .into_iter()
            .enumerate()
            .filter(|(_, changed)| *changed)
            .filter_map(|(index, _)| {
                self.received[index].map(|received| (RESOURCES[index], received))
            })
            .collect()
    }

    fn persist(&mut self) -> Result<()> {
        self.identity.state.local = self.snapshot.encode_to_vec();
        self.identity.save()
    }

    async fn apply_resource(&mut self, resource: Resource) -> Result<()> {
        let target = self
            .target
            .as_ref()
            .and_then(|s| s.target.as_ref())
            .context(error::PeerSnafu {
                code: "TARGET_MISSING",
            })?;
        let actual = self.snapshot.actual.as_mut().context(error::InvalidSnafu {
            code: "ACTUAL_MISSING",
        })?;
        match resource {
            Resource::Gateway => self.gateway().await?,
            Resource::Binding => {
                let bound = target
                    .binding_access
                    .as_ref()
                    .and_then(|b| b.bound.as_ref());
                actual.binding_access = Some(BindingAccessActualState {
                    assignment_state: if bound.is_some() {
                        BindingArtifactState::Applied
                    } else {
                        BindingArtifactState::Absent
                    }
                    .into(),
                    credential_state: BindingArtifactState::Absent.into(),
                    context: None,
                });
                if bound.is_some() {
                    self.snapshot
                        .input
                        .as_mut()
                        .context(error::InvalidSnafu {
                            code: "INPUT_MISSING",
                        })?
                        .binding = None;
                }
            }
            Resource::Runtime => {
                actual.runtime_config = Some(RuntimeConfigActualState {
                    state: RuntimeConfigState::Applied.into(),
                    applied_domjudge_origin: target
                        .runtime_config
                        .as_ref()
                        .map(|r| r.domjudge_origin.clone()),
                });
            }
            Resource::Session => {
                let target = target.session_control.as_ref().context(error::PeerSnafu {
                    code: "SESSION_MISSING",
                })?;
                let prior = actual
                    .session_control
                    .as_ref()
                    .and_then(|s| s.completed_terminate_epoch);
                actual.session_control = Some(SessionControlActualState {
                    session_state: SessionState::Running.into(),
                    completed_terminate_epoch: prior.max(target.terminate_epoch),
                    foreground: if target.foreground_target == i32::from(ForegroundTarget::Contest)
                    {
                        SessionForeground::Contest
                    } else {
                        SessionForeground::Waiting
                    }
                    .into(),
                    waiting_ready: true,
                    contest_ready: true,
                });
            }
            Resource::Home => {
                let epoch = target.home.as_ref().and_then(|h| h.reset_epoch);
                let prior = actual.home.as_ref().and_then(|h| h.completed_reset_epoch);
                actual.home = Some(HomeActualState {
                    state: HomeState::Steady.into(),
                    completed_reset_epoch: prior.max(epoch),
                });
            }
            Resource::Power => {
                actual.power = Some(power_actual(
                    target.power.as_ref().context(error::PeerSnafu {
                        code: "POWER_MISSING",
                    })?,
                    self.identity.state.power_accepted_epoch,
                ));
                if let Some(power) = &actual.power
                    && power.state == i32::from(PowerState::Accepted)
                {
                    self.identity.state.power_accepted_epoch = self
                        .identity
                        .state
                        .power_accepted_epoch
                        .max(power.attempted_shutdown_epoch);
                }
            }
        }
        Ok(())
    }

    async fn gateway(&mut self) -> Result<()> {
        let target = self
            .target
            .as_ref()
            .and_then(|s| s.target.as_ref())
            .and_then(|s| s.gateway.as_ref())
            .context(error::PeerSnafu {
                code: "GATEWAY_MISSING",
            })?;
        if self
            .identity
            .state
            .gateway
            .as_ref()
            .is_none_or(|g| g.credential_id != target.credential_id)
        {
            let credential_id = target.credential_id.clone();
            self.identity.state.gateway = Some(
                tokio::task::spawn_blocking(move || {
                    let key = Zeroizing::new(
                        KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
                            .context(error::CertificateSnafu)?,
                    );
                    let csr = CertificateParams::default()
                        .serialize_request(&*key)
                        .context(error::CertificateSnafu)?;
                    Ok::<_, Error>(Gateway {
                        credential_id,
                        key_der: key.serialized_der().to_vec(),
                        csr_der: csr.der().to_vec(),
                        leaf_der: Vec::new(),
                    })
                })
                .await
                .context(error::JoinSnafu)??,
            );
        }
        let gateway = self
            .identity
            .state
            .gateway
            .as_mut()
            .context(error::InvalidSnafu {
                code: "GATEWAY_STATE",
            })?;
        if let Some(certificate) = &target.certificate {
            let (rest, leaf) = X509Certificate::from_der(&certificate.gateway_leaf_der)
                .context(error::X509Snafu)?;
            let key = Zeroizing::new(
                KeyPair::try_from(gateway.key_der.as_slice()).context(error::CertificateSnafu)?,
            );
            if !rest.is_empty()
                || leaf.public_key().subject_public_key.data.as_ref() != key.public_key_raw()
            {
                return Err(Error::Peer {
                    code: "GATEWAY_LEAF_KEY",
                });
            }
            gateway.leaf_der.clone_from(&certificate.gateway_leaf_der);
        }
        self.snapshot
            .input
            .as_mut()
            .context(error::InvalidSnafu {
                code: "INPUT_MISSING",
            })?
            .gateway_credential = Some(GatewayCredentialInput {
            credential_id: gateway.credential_id.clone(),
            gateway_csr_der: Some(gateway.csr_der.clone()),
        });
        self.snapshot
            .actual
            .as_mut()
            .context(error::InvalidSnafu {
                code: "ACTUAL_MISSING",
            })?
            .gateway = Some(GatewayActualState {
            credential_id: Some(gateway.credential_id.clone()),
            state: if gateway.leaf_der.is_empty() {
                GatewayState::Restoring
            } else {
                GatewayState::Blocked
            }
            .into(),
            gateway_leaf_sha256: (!gateway.leaf_der.is_empty())
                .then(|| Sha256::digest(&gateway.leaf_der).to_vec()),
        });
        Ok(())
    }

    fn access_and_input(&mut self) -> Result<()> {
        let wire = self.target.as_ref().context(error::PeerSnafu {
            code: "TARGET_MISSING",
        })?;
        let target = wire.target.as_ref().context(error::PeerSnafu {
            code: "TARGET_MISSING",
        })?;
        let actual = self.snapshot.actual.as_mut().context(error::InvalidSnafu {
            code: "ACTUAL_MISSING",
        })?;
        let home_ready = actual.home.as_ref().is_some_and(|h| {
            h.state == i32::from(HomeState::Steady)
                && h.completed_reset_epoch == target.home.as_ref().and_then(|h| h.reset_epoch)
        });
        let session_ready = actual.session_control.as_ref().is_some_and(|s| {
            s.session_state == i32::from(SessionState::Running)
                && s.contest_ready
                && s.completed_terminate_epoch
                    == target
                        .session_control
                        .as_ref()
                        .and_then(|s| s.terminate_epoch)
        });
        let bound = target
            .binding_access
            .as_ref()
            .and_then(|b| b.bound.as_ref());
        let access = home_ready && session_ready;
        let runtime_ready = actual.runtime_config.as_ref().is_some_and(|r| {
            r.applied_domjudge_origin.as_deref()
                == target
                    .runtime_config
                    .as_ref()
                    .map(|r| r.domjudge_origin.as_str())
        });
        let material_ready = actual.gateway.as_ref().is_some_and(|g| {
            target.gateway.as_ref().is_some_and(|t| {
                g.credential_id.as_deref() == Some(t.credential_id.as_str())
                    && t.certificate.as_ref().is_some_and(|c| {
                        g.gateway_leaf_sha256.as_deref()
                            == Some(Sha256::digest(&c.gateway_leaf_der).as_slice())
                    })
            })
        });
        let binding_ready = access
            && runtime_ready
            && material_ready
            && bound.is_some()
            && self.pending[1].is_none();
        if let Some(binding) = actual.binding_access.as_mut() {
            binding.credential_state = if binding_ready {
                BindingArtifactState::Applied
            } else {
                BindingArtifactState::Absent
            }
            .into();
            binding.context = if binding_ready {
                bound.and_then(|b| b.context.clone())
            } else {
                None
            };
        }
        if let Some(session) = actual.session_control.as_mut() {
            if !access || bound.is_none() {
                session.foreground = SessionForeground::Waiting.into();
            } else if self.pending[3].is_none() {
                session.foreground =
                    if target.session_control.as_ref().is_some_and(|s| {
                        s.foreground_target == i32::from(ForegroundTarget::Contest)
                    }) && binding_ready
                    {
                        SessionForeground::Contest
                    } else {
                        SessionForeground::Waiting
                    }
                    .into();
            }
        }
        if let Some(gateway) = actual.gateway.as_mut()
            && gateway.gateway_leaf_sha256.is_some()
        {
            gateway.state = if binding_ready {
                GatewayState::Ready
            } else {
                GatewayState::Blocked
            }
            .into();
        }
        update_binding_input(
            &mut self.snapshot.input,
            wire,
            actual,
            access,
            self.pending[1].is_none(),
            &self.identity.state.seat,
        )?;

        Ok(())
    }
}

fn update_binding_input(
    input: &mut Option<ClientInputState>,
    wire: &ServerStateSnapshot,
    actual: &ActualState,
    access: bool,
    applied: bool,
    seat: &str,
) -> Result<()> {
    let input = input.as_mut().context(error::InvalidSnafu {
        code: "INPUT_MISSING",
    })?;
    if let Some(intent) = wire.intent.as_ref().and_then(|i| i.binding.as_ref()) {
        if access
            && applied
            && actual.session_control.as_ref().is_some_and(|s| {
                s.foreground == i32::from(SessionForeground::Waiting) && s.waiting_ready
            })
            && input
                .binding
                .as_ref()
                .is_none_or(|b| b.negotiation_id != intent.negotiation_id)
        {
            // One simulated volunteer confirmation per negotiation; replay never advances it.
            input.binding = Some(BindingInput {
                negotiation_id: intent.negotiation_id.clone(),
                submission_epoch: 1,
                seat_code: seat.to_owned(),
            });
        }
    } else {
        input.binding = None;
    }
    Ok(())
}

fn power_actual(
    target: &natsume_workstation_protocol::generated::PowerControlTarget,
    accepted: Option<u64>,
) -> PowerControlActualState {
    let Some(epoch) = target.shutdown_epoch else {
        return PowerControlActualState {
            state: PowerState::Idle.into(),
            attempted_shutdown_epoch: accepted,
        };
    };
    let state = if accepted.is_some_and(|done| done >= epoch) {
        PowerState::Accepted
    } else if target
        .expires_at_unix_ms
        .is_some_and(|deadline| unix_ms() > deadline)
    {
        PowerState::Expired
    } else {
        PowerState::Accepted
    };
    PowerControlActualState {
        state: state.into(),
        attempted_shutdown_epoch: Some(epoch),
    }
}

fn initial() -> ClientStateSnapshot {
    ClientStateSnapshot {
        input: Some(ClientInputState::default()),
        actual: Some(ActualState {
            gateway: Some(GatewayActualState {
                state: GatewayState::Absent.into(),
                ..GatewayActualState::default()
            }),
            binding_access: Some(BindingAccessActualState {
                assignment_state: BindingArtifactState::Absent.into(),
                credential_state: BindingArtifactState::Absent.into(),
                context: None,
            }),
            runtime_config: Some(RuntimeConfigActualState {
                state: RuntimeConfigState::Absent.into(),
                applied_domjudge_origin: None,
            }),
            session_control: Some(SessionControlActualState {
                session_state: SessionState::Running.into(),
                foreground: SessionForeground::Waiting.into(),
                waiting_ready: true,
                contest_ready: true,
                completed_terminate_epoch: None,
            }),
            home: Some(HomeActualState {
                state: HomeState::Steady.into(),
                completed_reset_epoch: None,
            }),
            power: Some(PowerControlActualState {
                state: PowerState::Idle.into(),
                attempted_shutdown_epoch: None,
            }),
        }),
    }
}

pub(crate) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
fn unix_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use natsume_workstation_protocol::generated::{
        BindingAccessTarget, BindingNegotiationIntent, BoundTarget, ConcreteTargetState,
        GatewayCredentialIntent, GatewayTarget, HomeTarget, PowerControlTarget,
        RuntimeConfigTarget, ServerIntentState, SessionControlTarget,
    };
    use uuid::Uuid;
    use x509_parser::{certification_request::X509CertificationRequest, oid_registry::OID_EC_P256};

    fn target() -> ServerStateSnapshot {
        let id = Uuid::now_v7().to_string();
        ServerStateSnapshot {
            intent: Some(ServerIntentState {
                gateway_credential: Some(GatewayCredentialIntent {
                    credential_id: id.clone(),
                }),
                binding: Some(BindingNegotiationIntent {
                    negotiation_id: Uuid::now_v7().to_string(),
                    evaluation: None,
                }),
            }),
            target: Some(ConcreteTargetState {
                gateway: Some(GatewayTarget {
                    credential_id: id,
                    certificate: None,
                }),
                binding_access: Some(BindingAccessTarget { bound: None }),
                runtime_config: Some(RuntimeConfigTarget {
                    domjudge_origin: "https://judge.example".to_owned(),
                }),
                session_control: Some(SessionControlTarget {
                    foreground_target: ForegroundTarget::Waiting.into(),
                    terminate_epoch: None,
                }),
                home: Some(HomeTarget { reset_epoch: None }),
                power: Some(PowerControlTarget {
                    shutdown_epoch: None,
                    expires_at_unix_ms: None,
                }),
            }),
        }
    }

    fn bind_target(target: &mut ServerStateSnapshot, emulator: &Emulator) -> Result<()> {
        use natsume_workstation_protocol::generated::{
            BindingContext, GatewayCertificateGrant, SecretBytes, TeamPresentation,
        };
        let gateway = emulator
            .identity
            .state
            .gateway
            .as_ref()
            .context(error::InvalidSnafu {
                code: "TEST_GATEWAY",
            })?;
        let key = Zeroizing::new(
            KeyPair::try_from(gateway.key_der.as_slice()).context(error::CertificateSnafu)?,
        );
        let leaf = CertificateParams::default()
            .self_signed(&*key)
            .context(error::CertificateSnafu)?;
        target
            .intent
            .as_mut()
            .context(error::InvalidSnafu {
                code: "TEST_INTENT",
            })?
            .binding = None;
        let concrete = target.target.as_mut().context(error::InvalidSnafu {
            code: "TEST_TARGET",
        })?;
        concrete
            .gateway
            .as_mut()
            .context(error::InvalidSnafu {
                code: "TEST_GATEWAY",
            })?
            .certificate = Some(GatewayCertificateGrant {
            gateway_leaf_der: leaf.der().to_vec(),
        });
        concrete.binding_access = Some(BindingAccessTarget {
            bound: Some(BoundTarget {
                context: Some(BindingContext {
                    binding_id: Uuid::now_v7().to_string(),
                    account_id: Uuid::now_v7().to_string(),
                    seat_code: "001".to_owned(),
                    domjudge_username: "synthetic001".to_owned(),
                    credential_revision: 1,
                }),
                password: Some(SecretBytes {
                    value: b"Synthetic-Password".to_vec(),
                }),
                presentation: Some(TeamPresentation {
                    organization_id: "INST-001".to_owned(),
                    team_name_en: "Synthetic Team".to_owned(),
                    school_name_en: "Synthetic School".to_owned(),
                    ..TeamPresentation::default()
                }),
            }),
        });
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn healthy_bound_session_follows_foreground_and_completes_transition_epochs() -> Result<()>
    {
        let directory = tempfile::tempdir().context(error::IoSnafu)?;
        let identity = Identity::load(directory.path(), 1, 0, "001".to_owned())?;
        let mut emulator = Emulator::new(identity, 1, 0)?;
        let run = crate::cli::tests::run();
        let mut target = target();
        emulator.accept(target.clone(), &run)?;
        tokio::time::advance(run.resource_delay).await;
        emulator.apply().await?;
        bind_target(&mut target, &emulator)?;
        emulator.accept(target.clone(), &run)?;
        tokio::time::advance(run.resource_delay).await;
        emulator.apply().await?;
        for foreground in [
            ForegroundTarget::Contest,
            ForegroundTarget::Waiting,
            ForegroundTarget::Contest,
        ] {
            let concrete = target.target.as_mut().context(error::InvalidSnafu {
                code: "TEST_TARGET",
            })?;
            concrete.session_control = Some(SessionControlTarget {
                foreground_target: foreground.into(),
                terminate_epoch: Some(7),
            });
            concrete.home = Some(HomeTarget {
                reset_epoch: Some(8),
            });
            emulator.accept(target.clone(), &run)?;
            // The Session resource itself reports the requested role before full snapshot assembly.
            emulator.apply_resource(Resource::Session).await?;
            let expected = if foreground == ForegroundTarget::Contest {
                SessionForeground::Contest
            } else {
                SessionForeground::Waiting
            };
            assert_eq!(
                emulator
                    .snapshot
                    .actual
                    .as_ref()
                    .and_then(|a| a.session_control.as_ref())
                    .map(|s| s.foreground),
                Some(expected.into())
            );
            tokio::time::advance(run.resource_delay).await;
            emulator.apply().await?;
            let actual = emulator
                .snapshot
                .actual
                .as_ref()
                .context(error::InvalidSnafu {
                    code: "TEST_ACTUAL",
                })?;
            assert_eq!(
                actual.session_control.as_ref().map(|s| s.foreground),
                Some(expected.into())
            );
            assert_eq!(
                actual
                    .session_control
                    .as_ref()
                    .and_then(|s| s.completed_terminate_epoch),
                Some(7)
            );
            assert_eq!(
                actual.home.as_ref().and_then(|h| h.completed_reset_epoch),
                Some(8)
            );
        }
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_targets_keep_delay_and_replay_exact_durable_csr_and_binding() -> Result<()> {
        let directory = tempfile::tempdir().context(error::IoSnafu)?;
        let mut run = crate::cli::tests::run();
        run.resource_delay = Duration::from_millis(100);
        let identity = Identity::load(directory.path(), 1, 0, "001".to_owned())?;
        let mut emulator = Emulator::new(identity, 1, 0)?;
        let target = target();
        emulator.accept(target.clone(), &run)?;
        let deadline = emulator.deadline();
        tokio::time::advance(Duration::from_millis(50)).await;
        emulator.accept(target.clone(), &run)?;
        assert_eq!(emulator.deadline(), deadline);
        tokio::time::advance(Duration::from_millis(50)).await;
        emulator.apply().await?;
        let input = emulator
            .snapshot
            .input
            .as_ref()
            .context(error::InvalidSnafu { code: "TEST_INPUT" })?;
        let gateway = input
            .gateway_credential
            .as_ref()
            .context(error::InvalidSnafu {
                code: "TEST_GATEWAY",
            })?;
        let csr_bytes = gateway
            .gateway_csr_der
            .as_ref()
            .context(error::InvalidSnafu { code: "TEST_CSR" })?;
        let (rest, csr) =
            X509CertificationRequest::from_der(csr_bytes).context(error::X509Snafu)?;
        assert!(rest.is_empty() && csr.verify_signature().is_ok());
        assert_eq!(
            csr.certification_request_info
                .subject_pki
                .algorithm
                .parameters
                .as_ref()
                .and_then(|p| p.as_oid().ok()),
            Some(OID_EC_P256)
        );
        assert_eq!(
            input
                .binding
                .as_ref()
                .map(|b| (b.submission_epoch, b.seat_code.as_str())),
            Some((1, "001"))
        );
        let before = emulator.snapshot.clone();
        emulator.accept(target.clone(), &run)?;
        assert!(emulator.deadline().is_none());
        emulator.apply().await?;
        assert_eq!(before, emulator.snapshot);
        let identity = Identity::load(directory.path(), 1, 0, "001".to_owned())?;
        let mut restored = Emulator::new(identity, 1, 0)?;
        restored.accept(target, &run)?;
        tokio::time::advance(Duration::from_millis(100)).await;
        restored.apply().await?;
        assert_eq!(restored.snapshot.input, before.input);
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn new_home_target_withdraws_access_until_its_delay_completes() -> Result<()> {
        let directory = tempfile::tempdir().context(error::IoSnafu)?;
        let identity = Identity::load(directory.path(), 1, 0, "001".to_owned())?;
        let mut emulator = Emulator::new(identity, 1, 0)?;
        let mut run = crate::cli::tests::run();
        run.home_delay = Some(Duration::from_secs(1));
        let mut target = target();
        target
            .target
            .as_mut()
            .and_then(|t| t.home.as_mut())
            .context(error::InvalidSnafu { code: "TEST_HOME" })?
            .reset_epoch = Some(9);
        emulator.accept(target.clone(), &run)?;
        tokio::time::advance(Duration::from_millis(20)).await;
        emulator.apply().await?;
        assert!(
            emulator
                .snapshot
                .input
                .as_ref()
                .and_then(|i| i.binding.as_ref())
                .is_none()
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        emulator.apply().await?;
        assert!(
            emulator
                .snapshot
                .input
                .as_ref()
                .and_then(|i| i.binding.as_ref())
                .is_some()
        );
        assert_eq!(
            emulator
                .snapshot
                .actual
                .as_ref()
                .and_then(|a| a.home.as_ref())
                .and_then(|h| h.completed_reset_epoch),
            Some(9)
        );
        // A bound target without context/presentation/credentials must never yield an Actual.
        target
            .intent
            .as_mut()
            .context(error::InvalidSnafu {
                code: "TEST_INTENT",
            })?
            .binding = None;
        target
            .target
            .as_mut()
            .and_then(|t| t.binding_access.as_mut())
            .context(error::InvalidSnafu {
                code: "TEST_BINDING",
            })?
            .bound = Some(BoundTarget::default());
        assert!(emulator.accept(target, &run).is_err());
        Ok(())
    }

    #[test]
    fn expired_power_request_does_not_become_accepted_on_replay() {
        let target = PowerControlTarget {
            shutdown_epoch: Some(7),
            expires_at_unix_ms: Some(1),
        };
        let expired = power_actual(&target, None);
        assert_eq!(expired.state, i32::from(PowerState::Expired));
        assert_eq!(power_actual(&target, None), expired);
        let idle = power_actual(&PowerControlTarget::default(), Some(6));
        assert_eq!(idle.state, i32::from(PowerState::Idle));
        assert_eq!(idle.attempted_shutdown_epoch, Some(6));
    }
}
