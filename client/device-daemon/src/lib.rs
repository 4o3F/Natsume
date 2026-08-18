#![forbid(unsafe_code)]
//! Device Daemon-owned contracts and install-time endpoint validation.

mod atomic_write;
mod client_configuration;
pub mod control;
pub mod enrollment;
mod identity_record;
pub(crate) mod journal;
pub mod startup;

use std::{net::IpAddr, num::NonZeroU16, str::FromStr};

use natsume_error_code::{ErrorCode, common::CommonErrorCode};
use snafu::Snafu;
use uuid::Uuid;

pub(crate) fn canonical_uuid(value: &str) -> Option<Uuid> {
    Uuid::parse_str(value)
        .ok()
        .filter(|uuid| uuid.hyphenated().to_string() == value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalEndpoint {
    ip: IpAddr,
    port: NonZeroU16,
}

impl CanonicalEndpoint {
    #[must_use]
    pub const fn ip(self) -> IpAddr {
        self.ip
    }

    #[must_use]
    pub const fn port(self) -> NonZeroU16 {
        self.port
    }
}

#[derive(Debug, Snafu)]
pub enum EndpointError {
    #[snafu(display("invalid Natsume Server IP literal"))]
    Ip,

    #[snafu(display("invalid Natsume Server port"))]
    Port,
}

impl EndpointError {
    /// Maps endpoint validation failures to the stable invalid-request semantic.
    #[must_use]
    pub const fn error_code(&self) -> ErrorCode {
        match self {
            Self::Ip | Self::Port => ErrorCode::Common(CommonErrorCode::InvalidRequest),
        }
    }
}

/// Parses an IP literal and non-zero TCP/UDP port into canonical typed values.
///
/// # Errors
///
/// Returns [`EndpointError::Ip`] for a hostname, bracketed address or malformed IP literal,
/// and [`EndpointError::Port`] for zero or a value outside the `u16` range.
pub fn parse_endpoint(ip: &str, port: &str) -> Result<CanonicalEndpoint, EndpointError> {
    let ip = IpAddr::from_str(ip).map_err(|_| EndpointError::Ip)?;
    let port = port.parse::<u16>().map_err(|_| EndpointError::Port)?;
    let Some(port) = NonZeroU16::new(port) else {
        return Err(EndpointError::Port);
    };
    Ok(CanonicalEndpoint { ip, port })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_ipv4_and_ipv6_literals() {
        let Ok(ipv4) = parse_endpoint("192.0.2.10", "8443") else {
            panic!("valid IPv4 endpoint must parse");
        };
        assert_eq!(ipv4.ip().to_string(), "192.0.2.10");
        assert_eq!(ipv4.port().get(), 8443);

        let Ok(ipv6) = parse_endpoint("2001:0db8:0:0:0:0:0:1", "443") else {
            panic!("valid IPv6 endpoint must parse");
        };
        assert_eq!(ipv6.ip().to_string(), "2001:db8::1");
        assert_eq!(ipv6.port().get(), 443);
    }

    #[test]
    fn rejects_hostnames_brackets_and_zero_port() {
        assert!(parse_endpoint("server.example", "8443").is_err());
        assert!(parse_endpoint("[2001:db8::1]", "8443").is_err());
        assert!(parse_endpoint("192.0.2.10", "0").is_err());
        assert!(parse_endpoint("192.0.2.10", "65536").is_err());
    }

    #[test]
    fn endpoint_failures_share_the_stable_invalid_request_semantic() {
        let expected = ErrorCode::Common(CommonErrorCode::InvalidRequest);

        assert_eq!(EndpointError::Ip.error_code(), expected);
        assert_eq!(EndpointError::Port.error_code(), expected);
    }
}
