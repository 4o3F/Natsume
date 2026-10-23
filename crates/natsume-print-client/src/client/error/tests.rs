use snafu::IntoError as _;

use super::*;

#[test]
fn rejected_settings_remain_visible_through_the_client_error() {
    let error = PrintSnafu.into_error(crate::SettingsRejectedSnafu.build());

    assert_eq!(
        error.to_string(),
        "native print operation failed: printer driver did not accept A4, monochrome, single-copy settings"
    );
}

#[test]
fn driver_failure_keeps_the_operation_and_return_code() {
    let error = PrintSnafu.into_error(
        crate::DriverCallSnafu {
            operation: "validate printer settings",
            code: -1,
        }
        .build(),
    );

    assert_eq!(
        error.to_string(),
        "native print operation failed: printer driver operation failed: validate printer settings (code -1)"
    );
}

#[test]
fn invalid_input_keeps_the_failed_validation_field() {
    let error = PrintSnafu.into_error(
        crate::InvalidSnafu {
            field: "A4 page size",
        }
        .build(),
    );

    assert_eq!(
        error.to_string(),
        "native print operation failed: invalid print input: A4 page size"
    );
}

#[test]
fn filesystem_failure_keeps_context_without_exposing_source_details() {
    let sensitive = "private document contents and credentials";
    let error = PrintSnafu.into_error(
        crate::IoSnafu {
            operation: "read immutable input",
        }
        .into_error(std::io::Error::other(sensitive)),
    );

    assert_eq!(
        error.to_string(),
        "native print operation failed: print filesystem operation failed: read immutable input"
    );
    assert!(!error.to_string().contains(sensitive));
    let ClientError::Print {
        source: crate::PrintError::Io { source, .. },
    } = error
    else {
        panic!("filesystem fixture did not preserve the typed source");
    };
    assert_eq!(source.to_string(), sensitive);
}

#[cfg(windows)]
#[test]
fn windows_failure_keeps_the_operation_and_hresult_without_exposing_source_details() {
    let sensitive = "private document contents and credentials";
    let source =
        windows::core::Error::new(windows::core::HRESULT(0x8007_0057_u32 as i32), sensitive);
    let error = PrintSnafu.into_error(
        crate::NativeSnafu {
            operation: "open PDF document",
        }
        .into_error(source),
    );

    assert_eq!(
        error.to_string(),
        "native print operation failed: Windows print operation failed: open PDF document (HRESULT 0x80070057)"
    );
    assert!(!error.to_string().contains(sensitive));
    let ClientError::Print {
        source: crate::PrintError::Native { source, .. },
    } = error
    else {
        panic!("Windows fixture did not preserve the typed source");
    };
    assert_eq!(source.code().0 as u32, 0x8007_0057);
    assert_eq!(source.message(), sensitive);
}
