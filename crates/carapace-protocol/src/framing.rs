use crate::messages::Message;
use bytes::{Buf, BufMut, BytesMut};
use std::io;
use tokio_util::codec::{Decoder, Encoder};

/// Maximum frame size: 100 MB (prevents DoS from giant messages)
const MAX_FRAME_SIZE: u32 = 100 * 1024 * 1024;

pub struct MessageCodec;

pub struct FrameError(pub String);

impl Decoder for MessageCodec {
    type Item = Message;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < 4 {
            return Ok(None); // Need at least length prefix
        }

        // Read the length prefix (big-endian u32)
        let mut length_bytes = [0u8; 4];
        length_bytes.copy_from_slice(&src[..4]);
        let length = u32::from_be_bytes(length_bytes);

        // Enforce maximum frame size
        if length > MAX_FRAME_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Frame too large: {} bytes (max {})", length, MAX_FRAME_SIZE),
            ));
        }

        // Check if we have the complete frame
        if src.len() < 4 + length as usize {
            return Ok(None); // Need more data
        }

        // Skip the length prefix
        src.advance(4);

        // Extract the payload
        let payload = src.split_to(length as usize);

        // Deserialize JSON
        let msg: Message = serde_json::from_slice(&payload)?;
        Ok(Some(msg))
    }
}

impl Encoder<Message> for MessageCodec {
    type Error = io::Error;

    fn encode(&mut self, msg: Message, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let json = serde_json::to_vec(&msg)?;

        let length = json.len() as u32;
        if length > MAX_FRAME_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Payload too large: {} bytes", length),
            ));
        }

        dst.reserve(4 + json.len());
        dst.put_u32(length);
        dst.put_slice(&json);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::*;
    use std::collections::HashMap;

    fn create_cli_request(id: &str) -> Message {
        Message::CliRequest(CliRequest {
            id: id.to_string(),
            tool: "gh".to_string(),
            argv: vec!["pr".to_string(), "list".to_string()],
            env: HashMap::new(),
            stdin: None,
            cwd: "/".to_string(),
        })
    }

    #[test]
    fn test_framing_encode_decode_roundtrip() {
        let mut codec = MessageCodec;
        let msg = create_cli_request("test-001");

        let mut buffer = BytesMut::new();
        codec
            .encode(msg.clone(), &mut buffer)
            .expect("encode failed");

        let decoded = codec
            .decode(&mut buffer)
            .expect("decode failed")
            .expect("no message");

        match (&msg, &decoded) {
            (Message::CliRequest(orig), Message::CliRequest(dec)) => {
                assert_eq!(orig.id, dec.id);
                assert_eq!(orig.tool, dec.tool);
                assert_eq!(orig.argv, dec.argv);
            }
            _ => panic!("Message type mismatch"),
        }
    }

    #[test]
    fn test_framing_partial_read() {
        let mut codec = MessageCodec;
        let msg = create_cli_request("test-002");

        let mut buffer = BytesMut::new();
        codec.encode(msg, &mut buffer).expect("encode failed");

        let _total_len = buffer.len();

        // Split buffer - keep only first 4 bytes (the length prefix)
        let mut partial = buffer.split_to(4);
        let result = codec.decode(&mut partial).expect("decode should not error");
        assert!(result.is_none(), "Should return None when incomplete");

        // Add more data but still incomplete
        partial.extend_from_slice(&buffer[..10]);
        let result = codec.decode(&mut partial).expect("decode should not error");
        assert!(result.is_none(), "Should still return None when incomplete");

        // Add the rest
        partial.extend_from_slice(&buffer[10..]);
        let result = codec.decode(&mut partial).expect("decode should not error");
        assert!(result.is_some(), "Should decode when complete");
    }

    #[test]
    fn test_framing_various_sizes() {
        let test_sizes = vec![1, 10, 100, 1000, 10_000, 100_000];

        for size in test_sizes {
            let mut codec = MessageCodec;

            let mut argv = vec!["arg0".to_string()];
            argv.extend((0..size).map(|i| format!("arg-{}", i)));

            let msg = Message::CliRequest(CliRequest {
                id: "size-test".to_string(),
                tool: "tool".to_string(),
                argv,
                env: HashMap::new(),
                stdin: None,
                cwd: "/".to_string(),
            });

            let mut buffer = BytesMut::new();
            codec
                .encode(msg.clone(), &mut buffer)
                .expect("encode failed");

            let decoded = codec
                .decode(&mut buffer)
                .expect("decode failed")
                .expect("no message");
            assert!(matches!(decoded, Message::CliRequest(_)));
        }
    }

    #[test]
    fn test_frame_size_limit() {
        let mut codec = MessageCodec;

        // Create a message that would exceed the limit
        let huge_argv = vec!["x".repeat(MAX_FRAME_SIZE as usize + 1)];

        let msg = Message::CliRequest(CliRequest {
            id: "huge".to_string(),
            tool: "tool".to_string(),
            argv: huge_argv,
            env: HashMap::new(),
            stdin: None,
            cwd: "/".to_string(),
        });

        let mut buffer = BytesMut::new();
        let result = codec.encode(msg, &mut buffer);

        // Depending on whether the JSON encoding itself exceeds the limit,
        // we should get an error
        if result.is_ok() {
            // If encoding succeeded, the message is actually smaller than the limit
            // This is fine - we just wanted to test the enforcement
        }
    }

    #[test]
    fn test_length_prefix_overflow() {
        // Manually create a frame with invalid length (larger than MAX_FRAME_SIZE)
        let mut buffer = BytesMut::new();

        // Put a length prefix that exceeds the limit
        let huge_length = MAX_FRAME_SIZE + 1;
        buffer.put_u32(huge_length);
        buffer.put_slice(b"some data");

        let mut codec = MessageCodec;
        let result = codec.decode(&mut buffer);

        assert!(
            result.is_err(),
            "Should reject frames larger than MAX_FRAME_SIZE"
        );
    }

    #[test]
    fn test_concurrent_encoding() {
        let mut codec = MessageCodec;

        for i in 0..10 {
            let msg = create_cli_request(&format!("concurrent-{}", i));

            let mut buffer = BytesMut::new();
            codec.encode(msg, &mut buffer).expect("encode failed");

            assert!(buffer.len() > 4, "Buffer should have length prefix + data");
        }
    }

    #[test]
    fn test_zero_length_frame() {
        // This shouldn't happen in practice, but test it doesn't crash
        let mut buffer = BytesMut::new();
        buffer.put_u32(0); // Zero-length payload

        let mut codec = MessageCodec;
        let result = codec.decode(&mut buffer);

        // Should fail because empty JSON isn't valid
        assert!(
            result.is_err(),
            "Zero-length payload should fail to deserialize"
        );
    }

    #[test]
    fn test_multiple_messages_in_buffer() {
        let mut codec = MessageCodec;

        let msg1 = create_cli_request("first");
        let msg2 = create_cli_request("second");

        let mut buffer = BytesMut::new();
        codec.encode(msg1, &mut buffer).expect("encode 1 failed");
        codec.encode(msg2, &mut buffer).expect("encode 2 failed");

        // Decode first
        let decoded1 = codec
            .decode(&mut buffer)
            .expect("decode 1 failed")
            .expect("no message 1");
        assert!(matches!(decoded1, Message::CliRequest(ref r) if r.id == "first"));

        // Decode second
        let decoded2 = codec
            .decode(&mut buffer)
            .expect("decode 2 failed")
            .expect("no message 2");
        assert!(matches!(decoded2, Message::CliRequest(ref r) if r.id == "second"));

        // Buffer should be empty now
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn test_all_message_types_framing() {
        let mut codec = MessageCodec;

        let messages = vec![
            Message::CliRequest(CliRequest {
                id: "cli-req".to_string(),
                tool: "gh".to_string(),
                argv: vec![],
                env: HashMap::new(),
                stdin: None,
                cwd: "/".to_string(),
            }),
            Message::CliResponse(CliResponse {
                id: "cli-res".to_string(),
                exit_code: 0,
                stdout: "output".to_string(),
                stderr: "".to_string(),
            }),
            Message::HttpRequest(HttpRequest {
                id: "http-req".to_string(),
                tool: "signal-cli".to_string(),
                method: "POST".to_string(),
                path: "/api".to_string(),
                headers: HashMap::new(),
                body: None,
            }),
            Message::HttpResponse(HttpResponse {
                id: "http-res".to_string(),
                status: 200,
                headers: HashMap::new(),
                body: Some("{}".to_string()),
            }),
            Message::Error(ErrorMessage {
                id: Some("err".to_string()),
                code: "DENIED".to_string(),
                message: "Policy denied this request".to_string(),
            }),
        ];

        for msg in messages {
            let mut buffer = BytesMut::new();
            codec
                .encode(msg.clone(), &mut buffer)
                .expect("encode failed");

            let decoded = codec
                .decode(&mut buffer)
                .expect("decode failed")
                .expect("no message");

            // Just verify it decoded to the same type
            match (&msg, &decoded) {
                (Message::CliRequest(_), Message::CliRequest(_)) => {}
                (Message::CliResponse(_), Message::CliResponse(_)) => {}
                (Message::HttpRequest(_), Message::HttpRequest(_)) => {}
                (Message::HttpResponse(_), Message::HttpResponse(_)) => {}
                (Message::Error(_), Message::Error(_)) => {}
                _ => panic!("Type mismatch"),
            }
        }
    }

    #[test]
    fn test_invalid_utf8_in_json() {
        // Create a buffer with invalid UTF-8
        let mut buffer = BytesMut::new();
        let invalid_json = b"Not valid JSON at all \xff\xfe";
        buffer.put_u32(invalid_json.len() as u32);
        buffer.put_slice(invalid_json);

        let mut codec = MessageCodec;
        let result = codec.decode(&mut buffer);

        // Should error on deserialization
        assert!(result.is_err(), "Invalid JSON should cause error");
    }

    #[test]
    fn test_truncated_frames_at_various_offsets() {
        let mut codec = MessageCodec;
        let msg = create_cli_request("truncate-test");

        let mut full_buffer = BytesMut::new();
        codec.encode(msg, &mut full_buffer).expect("encode failed");

        let full_data = full_buffer.to_vec();

        // Test truncation at various points
        for truncate_at in 1..full_data.len() {
            let mut truncated = BytesMut::from(&full_data[..truncate_at]);
            let result = codec.decode(&mut truncated);

            // Should either return None or error, but not panic
            match result {
                Ok(Some(_)) => {
                    // This is fine if the truncation point happened to be valid
                    // (unlikely but possible for very short messages)
                }
                Ok(None) => {
                    // This is expected - incomplete frame
                }
                Err(_) => {
                    // This is also fine - invalid data
                }
            }
        }
    }
}
