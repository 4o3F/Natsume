use prost::Message as _;
use sha2::{Digest as _, Sha256};

use crate::generated::{BindingAccessTarget, BoundTarget, ConcreteTargetState};

/// Returns 16 lowercase hex digits of SHA-256 over the password-free target encoding.
///
/// All message presence and fields except the Binding password are preserved.
/// The password is neither copied into the projection nor included in its encoding.
#[must_use]
pub fn target_digest(target: &ConcreteTargetState) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

    let hash = Sha256::digest(digest_projection(target).encode_to_vec());
    let mut digest = String::with_capacity(16);
    for byte in &hash[..8] {
        digest.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        digest.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    digest
}

fn digest_projection(target: &ConcreteTargetState) -> ConcreteTargetState {
    ConcreteTargetState {
        gateway: target.gateway.clone(),
        binding_access: target
            .binding_access
            .as_ref()
            .map(|binding| BindingAccessTarget {
                bound: binding.bound.as_ref().map(|bound| BoundTarget {
                    context: bound.context.clone(),
                    password: None,
                    presentation: bound.presentation.clone(),
                }),
            }),
        runtime_config: target.runtime_config.clone(),
        session_control: target.session_control,
        home: target.home,
        power: target.power,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated::{
        BindingContext, ForegroundTarget, GatewayCertificateGrant, GatewayTarget, HomeTarget,
        PowerControlTarget, RuntimeConfigTarget, SecretBytes, SessionControlTarget,
        TeamPresentation,
    };

    const CANARY_PASSWORD: &[u8] = b"fictitious-observability-password-canary";
    const GOLDEN_TARGET: &[u8] = include_bytes!("../testdata/target-digest.pb");
    const GOLDEN_DIGEST: &str = include_str!("../testdata/target-digest.txt");

    fn canonical_target() -> ConcreteTargetState {
        ConcreteTargetState {
            gateway: Some(GatewayTarget {
                credential_id: "01900000-0000-7000-8000-000000000001".to_owned(),
                certificate: None,
            }),
            binding_access: Some(BindingAccessTarget {
                bound: Some(BoundTarget {
                    context: Some(BindingContext {
                        binding_id: "01900000-0000-7000-8000-000000000002".to_owned(),
                        account_id: "01900000-0000-7000-8000-000000000003".to_owned(),
                        seat_code: "A-01".to_owned(),
                        domjudge_username: "fixture-team".to_owned(),
                        credential_revision: 3,
                    }),
                    password: Some(SecretBytes {
                        value: CANARY_PASSWORD.to_vec(),
                    }),
                    presentation: Some(TeamPresentation {
                        team_name_zh: "测试队伍".to_owned(),
                        team_name_en: "Fixture Team".to_owned(),
                        school_name_zh: "测试学校".to_owned(),
                        school_name_en: "Fixture School".to_owned(),
                        organization_id: "INST-001".to_owned(),
                    }),
                }),
            }),
            runtime_config: Some(RuntimeConfigTarget {
                domjudge_origin: "https://judge.example.test".to_owned(),
            }),
            session_control: Some(SessionControlTarget {
                foreground_target: ForegroundTarget::Contest.into(),
                terminate_epoch: Some(7),
            }),
            home: Some(HomeTarget {
                reset_epoch: Some(9),
            }),
            power: Some(PowerControlTarget {
                shutdown_epoch: None,
                expires_at_unix_ms: None,
            }),
        }
    }

    fn bound_mut(target: &mut ConcreteTargetState) -> &mut BoundTarget {
        target
            .binding_access
            .as_mut()
            .and_then(|binding| binding.bound.as_mut())
            .unwrap_or_else(|| panic!("canonical target must be bound"))
    }

    #[test]
    fn canonical_target_matches_the_fixed_encoding_and_digest() {
        let target = canonical_target();
        let digest = crate::target_digest(&target);

        assert_eq!(target.encode_to_vec(), GOLDEN_TARGET);
        assert_eq!(digest, GOLDEN_DIGEST.trim_end());
        assert_eq!(digest.len(), 16);
        assert!(
            digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert!(
            GOLDEN_TARGET
                .windows(CANARY_PASSWORD.len())
                .any(|bytes| bytes == CANARY_PASSWORD)
        );
    }

    #[test]
    fn projection_preserves_non_password_fields_and_leaves_the_source_unchanged() {
        let target = canonical_target();
        let original_encoding = target.encode_to_vec();
        let mut expected = canonical_target();
        bound_mut(&mut expected).password = None;

        let projection = digest_projection(&target);

        assert_eq!(projection, expected);
        assert_eq!(target.encode_to_vec(), original_encoding);
        assert!(
            !projection
                .encode_to_vec()
                .windows(CANARY_PASSWORD.len())
                .any(|bytes| bytes == CANARY_PASSWORD)
        );
    }

    #[test]
    fn digest_ignores_password_value_and_presence() {
        let mut target = canonical_target();
        let expected = target_digest(&target);

        for password in [Some(Vec::new()), Some(vec![b'x'; 512]), None] {
            bound_mut(&mut target).password = password.map(|value| SecretBytes { value });
            assert_eq!(target_digest(&target), expected);
        }
    }

    #[test]
    fn digest_changes_with_each_resource() {
        let expected = target_digest(&canonical_target());

        for resource in ["gateway", "binding", "runtime", "session", "home", "power"] {
            let mut target = canonical_target();
            match resource {
                "gateway" => target
                    .gateway
                    .get_or_insert_default()
                    .credential_id
                    .push('2'),
                "binding" => {
                    bound_mut(&mut target)
                        .context
                        .get_or_insert_default()
                        .credential_revision += 1;
                }
                "runtime" => {
                    target
                        .runtime_config
                        .get_or_insert_default()
                        .domjudge_origin = "https://other.example.test".to_owned();
                }
                "session" => {
                    target
                        .session_control
                        .get_or_insert_default()
                        .terminate_epoch = Some(8);
                }
                "home" => target.home.get_or_insert_default().reset_epoch = Some(10),
                "power" => target.power.get_or_insert_default().shutdown_epoch = Some(11),
                _ => unreachable!("unknown fixture resource"),
            }
            assert_ne!(target_digest(&target), expected, "resource {resource}");
        }
    }

    #[test]
    fn digest_includes_certificate_presentation_foreground_and_power_deadline() {
        let expected = target_digest(&canonical_target());
        let mut certificate = canonical_target();
        certificate.gateway.get_or_insert_default().certificate = Some(GatewayCertificateGrant {
            gateway_leaf_der: vec![1, 2, 3],
        });
        assert_ne!(target_digest(&certificate), expected);

        let mut presentation = canonical_target();
        bound_mut(&mut presentation)
            .presentation
            .get_or_insert_default()
            .team_name_en = "Another Fixture Team".to_owned();
        assert_ne!(target_digest(&presentation), expected);

        let mut foreground = canonical_target();
        foreground
            .session_control
            .get_or_insert_default()
            .foreground_target = ForegroundTarget::Waiting.into();
        assert_ne!(target_digest(&foreground), expected);

        let mut deadline = canonical_target();
        deadline.power.get_or_insert_default().expires_at_unix_ms = Some(1_800_000_000_000);
        assert_ne!(target_digest(&deadline), expected);
    }

    #[test]
    fn projection_preserves_each_resource_message_presence() {
        for presence in 0_u8..64 {
            let target = ConcreteTargetState {
                gateway: (presence & 1 != 0).then(GatewayTarget::default),
                binding_access: (presence & 2 != 0).then(BindingAccessTarget::default),
                runtime_config: (presence & 4 != 0).then(RuntimeConfigTarget::default),
                session_control: (presence & 8 != 0).then(SessionControlTarget::default),
                home: (presence & 16 != 0).then(HomeTarget::default),
                power: (presence & 32 != 0).then(PowerControlTarget::default),
            };
            assert_eq!(digest_projection(&target), target, "presence {presence}");
        }
    }

    #[test]
    fn projection_preserves_nested_and_optional_scalar_presence() {
        let mut target = ConcreteTargetState::default();
        let mut previous_digest = target_digest(&target);

        for binding in [
            BindingAccessTarget { bound: None },
            BindingAccessTarget {
                bound: Some(BoundTarget::default()),
            },
            BindingAccessTarget {
                bound: Some(BoundTarget {
                    context: Some(BindingContext::default()),
                    password: None,
                    presentation: Some(TeamPresentation::default()),
                }),
            },
        ] {
            target.binding_access = Some(binding);
            assert_eq!(digest_projection(&target), target);
            let digest = target_digest(&target);
            assert_ne!(digest, previous_digest);
            previous_digest = digest;
        }

        target.gateway = Some(GatewayTarget {
            credential_id: String::new(),
            certificate: Some(GatewayCertificateGrant::default()),
        });
        target.session_control = Some(SessionControlTarget {
            foreground_target: ForegroundTarget::Waiting.into(),
            terminate_epoch: Some(0),
        });
        target.home = Some(HomeTarget {
            reset_epoch: Some(0),
        });
        target.power = Some(PowerControlTarget {
            shutdown_epoch: Some(0),
            expires_at_unix_ms: Some(0),
        });
        assert_eq!(digest_projection(&target), target);
    }

    #[test]
    fn target_and_projection_encodings_are_stable_after_decode() {
        let target = ConcreteTargetState::decode(GOLDEN_TARGET)
            .unwrap_or_else(|error| panic!("canonical target decode: {error}"));
        assert_eq!(target, canonical_target());
        assert_eq!(target.encode_to_vec(), GOLDEN_TARGET);
        assert_eq!(target_digest(&target), GOLDEN_DIGEST.trim_end());

        let projection = digest_projection(&target);
        let encoded = projection.encode_to_vec();
        let decoded = ConcreteTargetState::decode(encoded.as_slice())
            .unwrap_or_else(|error| panic!("projected target decode: {error}"));
        assert_eq!(decoded, projection);
        assert_eq!(decoded.encode_to_vec(), encoded);
        assert_eq!(target_digest(&decoded), target_digest(&target));
    }
}
