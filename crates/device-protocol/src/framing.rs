use bytes::{Buf, BufMut, Bytes, BytesMut};
use natsume_error_code::{AsErrorCode, ErrorCode};
use prost::Message;
use snafu::Snafu;

use crate::generated::ControlEnvelope;

/// Hard upper bound for one encoded control payload.
pub const DEFAULT_MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Snafu)]
pub enum ProtocolFrameError {
    #[snafu(display("control payload length {frame_bytes} exceeds limit {max_frame_bytes}"))]
    FrameTooLarge {
        frame_bytes: usize,
        max_frame_bytes: usize,
    },

    #[snafu(display("control envelope encoding failed"))]
    Encode { source: prost::EncodeError },

    #[snafu(display("control envelope decoding failed"))]
    Decode { source: prost::DecodeError },
}

impl AsErrorCode for ProtocolFrameError {
    fn error_code(&self) -> ErrorCode {
        match self {
            Self::FrameTooLarge { .. } => ErrorCode::ProtocolFrameTooLarge,
            Self::Encode { .. } | Self::Decode { .. } => ErrorCode::ProtocolInvalidEnvelope,
        }
    }
}

/// Encodes one `u32` big-endian length prefix followed by one Protobuf envelope.
///
/// # Errors
///
/// Returns [`ProtocolFrameError::FrameTooLarge`] when the encoded payload exceeds the
/// configured limit or cannot fit in the wire `u32`; returns
/// [`ProtocolFrameError::Encode`] when Prost rejects the envelope.
pub fn encode_frame(
    envelope: &ControlEnvelope,
    max_frame_bytes: usize,
) -> Result<Bytes, ProtocolFrameError> {
    let payload_len = envelope.encoded_len();
    let Ok(payload_len_u32) = u32::try_from(payload_len) else {
        return FrameTooLargeSnafu {
            frame_bytes: payload_len,
            max_frame_bytes,
        }
        .fail();
    };
    if payload_len > max_frame_bytes {
        return FrameTooLargeSnafu {
            frame_bytes: payload_len,
            max_frame_bytes,
        }
        .fail();
    }

    let mut frame = BytesMut::with_capacity(4 + payload_len);
    frame.put_u32(payload_len_u32);
    envelope
        .encode(&mut frame)
        .map_err(|source| ProtocolFrameError::Encode { source })?;
    Ok(frame.freeze())
}

/// Decodes one complete frame without allocating until the advertised length is accepted.
///
/// `Ok(None)` means the streaming buffer does not yet contain a complete frame. A caller
/// that reaches EOF while bytes remain must report a truncated transport frame.
///
/// # Errors
///
/// Returns [`ProtocolFrameError::FrameTooLarge`] before allocation when the advertised
/// payload exceeds the configured limit, or [`ProtocolFrameError::Decode`] when a complete
/// payload is not a valid `ControlEnvelope`.
pub fn decode_frame(
    source: &mut BytesMut,
    max_frame_bytes: usize,
) -> Result<Option<ControlEnvelope>, ProtocolFrameError> {
    if source.len() < 4 {
        return Ok(None);
    }

    let payload_len = u32::from_be_bytes([source[0], source[1], source[2], source[3]]) as usize;
    if payload_len > max_frame_bytes {
        return FrameTooLargeSnafu {
            frame_bytes: payload_len,
            max_frame_bytes,
        }
        .fail();
    }

    let Some(frame_len) = payload_len.checked_add(4) else {
        return FrameTooLargeSnafu {
            frame_bytes: payload_len,
            max_frame_bytes,
        }
        .fail();
    };
    if source.len() < frame_len {
        return Ok(None);
    }

    source.advance(4);
    let payload = source.split_to(payload_len).freeze();
    ControlEnvelope::decode(payload)
        .map(Some)
        .map_err(|source| ProtocolFrameError::Decode { source })
}
