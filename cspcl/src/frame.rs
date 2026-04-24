#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Bundle { id: u64, payload: Vec<u8> },
    BundleAck { id: u64 },
    Heartbeat,
    HeartbeatAck,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("frame too short")]
    FrameTooShort,
    #[error("unsupported frame version {0}")]
    UnsupportedVersion(u8),
    #[error("bundle frame too short")]
    BundleTooShort,
    #[error("bundle-ack frame must include exactly 8-byte id, got {0} bytes")]
    InvalidBundleAckLength(usize),
    #[error("heartbeat frame must not include payload, got {0} extra bytes")]
    InvalidHeartbeatLength(usize),
    #[error("heartbeat-ack frame must not include payload, got {0} extra bytes")]
    InvalidHeartbeatAckLength(usize),
    #[error("unknown frame type {0}")]
    UnknownFrameType(u8),
}

const VERSION: u8 = 1;
const TYPE_BUNDLE: u8 = 1;
const TYPE_HEARTBEAT: u8 = 2;
const TYPE_HEARTBEAT_ACK: u8 = 3;
const TYPE_BUNDLE_ACK: u8 = 4;

impl Into<Vec<u8>> for Frame {
    fn into(self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(VERSION);
        match self {
            Frame::Bundle { id, payload } => {
                out.push(TYPE_BUNDLE);
                out.extend_from_slice(&id.to_be_bytes());
                out.extend_from_slice(&payload);
            }
            Frame::BundleAck { id } => {
                out.push(TYPE_BUNDLE_ACK);
                out.extend_from_slice(&id.to_be_bytes());
            }
            Frame::Heartbeat => out.push(TYPE_HEARTBEAT),
            Frame::HeartbeatAck => out.push(TYPE_HEARTBEAT_ACK),
        }
        out
    }
}

impl Frame {
    pub fn bundle_id(&self) -> Option<u64> {
        match self {
            Frame::Bundle { id, .. } => Some(*id),
            _ => None,
        }
    }
}

impl TryFrom<&Vec<u8>> for Frame {
    type Error = DecodeError;

    fn try_from(input: &Vec<u8>) -> Result<Self, Self::Error> {
        if input.len() < 2 {
            return Err(DecodeError::FrameTooShort);
        }

        if input[0] != VERSION {
            return Err(DecodeError::UnsupportedVersion(input[0]));
        }

        match input[1] {
            TYPE_BUNDLE => {
                if input.len() < 10 {
                    return Err(DecodeError::BundleTooShort);
                }
                let mut id = [0u8; 8];
                id.copy_from_slice(&input[2..10]);
                Ok(Frame::Bundle {
                    id: u64::from_be_bytes(id),
                    payload: input[10..].to_vec(),
                })
            }
            TYPE_BUNDLE_ACK => {
                if input.len() != 10 {
                    return Err(DecodeError::InvalidBundleAckLength(input.len()));
                }
                let mut id = [0u8; 8];
                id.copy_from_slice(&input[2..10]);
                Ok(Frame::BundleAck {
                    id: u64::from_be_bytes(id),
                })
            }
            TYPE_HEARTBEAT => {
                if input.len() != 2 {
                    return Err(DecodeError::InvalidHeartbeatLength(input.len() - 2));
                }
                Ok(Frame::Heartbeat)
            }
            TYPE_HEARTBEAT_ACK => {
                if input.len() != 2 {
                    return Err(DecodeError::InvalidHeartbeatAckLength(input.len() - 2));
                }
                Ok(Frame::HeartbeatAck)
            }
            t => Err(DecodeError::UnknownFrameType(t)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trips() {
        let cases = [
            Frame::Bundle {
                id: 42,
                payload: vec![1, 2, 3],
            },
            Frame::BundleAck { id: 1234 },
            Frame::Heartbeat,
            Frame::HeartbeatAck,
        ];

        for frame in cases {
            let encoded: Vec<u8> = frame.clone().into();
            let decoded = Frame::try_from(&encoded).expect("frame should decode");
            assert_eq!(decoded, frame);
        }
    }

    #[test]
    fn malformed_frames_are_rejected() {
        assert!(Frame::try_from(&vec![]).is_err());
        assert!(Frame::try_from(&vec![2, TYPE_HEARTBEAT]).is_err());
        assert!(Frame::try_from(&vec![VERSION, TYPE_BUNDLE]).is_err());
        assert!(Frame::try_from(&vec![VERSION, TYPE_BUNDLE_ACK, 0]).is_err());
        assert!(Frame::try_from(&vec![VERSION, TYPE_HEARTBEAT, 0]).is_err());
        assert!(Frame::try_from(&vec![VERSION, TYPE_HEARTBEAT_ACK, 0]).is_err());
        assert!(Frame::try_from(&vec![VERSION, 0xff]).is_err());
    }

    #[test]
    fn malformed_frames_return_typed_errors() {
        assert_eq!(Frame::try_from(&vec![]), Err(DecodeError::FrameTooShort));
        assert_eq!(
            Frame::try_from(&vec![2, TYPE_HEARTBEAT]),
            Err(DecodeError::UnsupportedVersion(2))
        );
        assert_eq!(
            Frame::try_from(&vec![VERSION, TYPE_BUNDLE_ACK, 0]),
            Err(DecodeError::InvalidBundleAckLength(3))
        );
        assert_eq!(
            Frame::try_from(&vec![VERSION, TYPE_HEARTBEAT, 0]),
            Err(DecodeError::InvalidHeartbeatLength(1))
        );
        assert_eq!(
            Frame::try_from(&vec![VERSION, TYPE_HEARTBEAT_ACK, 0]),
            Err(DecodeError::InvalidHeartbeatAckLength(1))
        );
        assert_eq!(
            Frame::try_from(&vec![VERSION, 0xff]),
            Err(DecodeError::UnknownFrameType(0xff))
        );
    }
}
