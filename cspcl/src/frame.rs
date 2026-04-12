#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Bundle(Vec<u8>),
    Heartbeat,
    HeartbeatAck,
}

const VERSION: u8 = 1;
const TYPE_BUNDLE: u8 = 1;
const TYPE_HEARTBEAT: u8 = 2;
const TYPE_HEARTBEAT_ACK: u8 = 3;

pub fn encode(frame: Frame) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(VERSION);
    match frame {
        Frame::Bundle(payload) => {
            out.push(TYPE_BUNDLE);
            out.extend_from_slice(&payload);
        }
        Frame::Heartbeat => out.push(TYPE_HEARTBEAT),
        Frame::HeartbeatAck => out.push(TYPE_HEARTBEAT_ACK),
    }
    out
}

pub fn decode(input: &[u8]) -> Result<Frame, String> {
    if input.len() < 2 {
        return Err("frame too short".to_string());
    }

    if input[0] != VERSION {
        return Err(format!("unsupported frame version {}", input[0]));
    }

    match input[1] {
        TYPE_BUNDLE => Ok(Frame::Bundle(input[2..].to_vec())),
        TYPE_HEARTBEAT => {
            if input.len() != 2 {
                return Err("heartbeat frame must not include payload".to_string());
            }
            Ok(Frame::Heartbeat)
        }
        TYPE_HEARTBEAT_ACK => {
            if input.len() != 2 {
                return Err("heartbeat-ack frame must not include payload".to_string());
            }
            Ok(Frame::HeartbeatAck)
        }
        t => Err(format!("unknown frame type {t}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trips() {
        let cases = [
            Frame::Bundle(vec![1, 2, 3]),
            Frame::Heartbeat,
            Frame::HeartbeatAck,
        ];

        for frame in cases {
            let encoded = encode(frame.clone());
            let decoded = decode(&encoded).expect("frame should decode");
            assert_eq!(decoded, frame);
        }
    }

    #[test]
    fn malformed_frames_are_rejected() {
        assert!(decode(&[]).is_err());
        assert!(decode(&[2, TYPE_HEARTBEAT]).is_err());
        assert!(decode(&[VERSION, TYPE_HEARTBEAT, 0]).is_err());
        assert!(decode(&[VERSION, TYPE_HEARTBEAT_ACK, 0]).is_err());
        assert!(decode(&[VERSION, 0xff]).is_err());
    }
}
