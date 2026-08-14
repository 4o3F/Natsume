#![forbid(unsafe_code)]
//! Stable public error-code registry shared by Natsume boundaries.
//!
//! Module-local errors remain owned by their modules. Public adapters classify them into one of
//! these closed categories before encoding the stable string for a transport or `CommandStatus`.

macro_rules! define_error_codes {
    (
        $(#[$enum_meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident => $stable:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        pub enum $name {
            $(
                $(#[$variant_meta])*
                #[serde(rename = $stable)]
                $variant,
            )+
        }

        impl $name {
            /// Returns the stable wire/API string.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $stable,)+
                }
            }
        }

        #[cfg(test)]
        pub(crate) mod tests {
            use super::$name;

            pub(crate) const ALL: &'static [$name] = &[$($name::$variant,)+];
        }
    };
}

pub mod common;
pub mod control;
pub mod device;
pub mod enrollment;
pub mod home;
pub mod operator;
pub mod session;

use common::CommonErrorCode;
use control::ControlErrorCode;
use device::DeviceErrorCode;
use enrollment::EnrollmentErrorCode;
use home::HomeErrorCode;
use operator::OperatorErrorCode;
use session::SessionErrorCode;

/// A stable public error semantic from any Natsume boundary category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ErrorCode {
    /// Cross-cutting public boundary failure.
    Common(CommonErrorCode),
    /// Panel-to-Server operator failure.
    Operator(OperatorErrorCode),
    /// Provisioning or Enrollment failure.
    Enrollment(EnrollmentErrorCode),
    /// Command or typed device-control failure.
    Control(ControlErrorCode),
    /// Device identity, Gateway, or secret execution failure.
    Device(DeviceErrorCode),
    /// Local Session boundary failure.
    Session(SessionErrorCode),
    /// Local Home lifecycle failure.
    Home(HomeErrorCode),
}

impl ErrorCode {
    /// Returns the stable wire/API string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Common(code) => code.as_str(),
            Self::Operator(code) => code.as_str(),
            Self::Enrollment(code) => code.as_str(),
            Self::Control(code) => code.as_str(),
            Self::Device(code) => code.as_str(),
            Self::Session(code) => code.as_str(),
            Self::Home(code) => code.as_str(),
        }
    }
}

impl From<CommonErrorCode> for ErrorCode {
    fn from(code: CommonErrorCode) -> Self {
        Self::Common(code)
    }
}

impl From<OperatorErrorCode> for ErrorCode {
    fn from(code: OperatorErrorCode) -> Self {
        Self::Operator(code)
    }
}

impl From<EnrollmentErrorCode> for ErrorCode {
    fn from(code: EnrollmentErrorCode) -> Self {
        Self::Enrollment(code)
    }
}

impl From<ControlErrorCode> for ErrorCode {
    fn from(code: ControlErrorCode) -> Self {
        Self::Control(code)
    }
}

impl From<DeviceErrorCode> for ErrorCode {
    fn from(code: DeviceErrorCode) -> Self {
        Self::Device(code)
    }
}

impl From<SessionErrorCode> for ErrorCode {
    fn from(code: SessionErrorCode) -> Self {
        Self::Session(code)
    }
}

impl From<HomeErrorCode> for ErrorCode {
    fn from(code: HomeErrorCode) -> Self {
        Self::Home(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_defined_code_round_trips_through_serde() {
        let mut count = 0;

        for &code in common::tests::ALL {
            assert_round_trip(code.into());
            count += 1;
        }
        for &code in operator::tests::ALL {
            assert_round_trip(code.into());
            count += 1;
        }
        for &code in enrollment::tests::ALL {
            assert_round_trip(code.into());
            count += 1;
        }
        for &code in control::tests::ALL {
            assert_round_trip(code.into());
            count += 1;
        }
        for &code in device::tests::ALL {
            assert_round_trip(code.into());
            count += 1;
        }
        for &code in session::tests::ALL {
            assert_round_trip(code.into());
            count += 1;
        }
        for &code in home::tests::ALL {
            assert_round_trip(code.into());
            count += 1;
        }

        assert_eq!(count, 32);
    }

    fn assert_round_trip(code: ErrorCode) {
        let encoded = match serde_json::to_string(&code) {
            Ok(encoded) => encoded,
            Err(error) => panic!("failed to serialize {}: {error}", code.as_str()),
        };
        let decoded: ErrorCode = match serde_json::from_str(&encoded) {
            Ok(decoded) => decoded,
            Err(error) => panic!("failed to deserialize {}: {error}", code.as_str()),
        };

        assert_eq!(encoded, format!("\"{}\"", code.as_str()));
        assert_eq!(decoded, code);
    }
}
