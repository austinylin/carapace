use bytes::BytesMut;
/// Error Handling and Malformed Data Tests
///
/// Tests that the system handles malformed data, oversized messages, and
/// error conditions gracefully without panicking or hanging.
use carapace_protocol::{CliRequest, Message, MessageCodec};
use std::collections::HashMap;
use tokio_util::codec::{Decoder, Encoder};

#[test]
fn test_decode_invalid_utf8_json() {
    let mut codec = MessageCodec;
    let mut buffer = BytesMut::new();

    // Create a buffer with invalid UTF-8 in the JSON
    buffer.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // Invalid UTF-8 length prefix
    buffer.extend_from_slice(b"invalid json");

    let result = codec.decode(&mut buffer);
    assert!(result.is_err(), "Should reject invalid UTF-8");
}

#[test]
fn test_decode_truncated_length_prefix() {
    let mut codec = MessageCodec;
    let mut buffer = BytesMut::new();

    // Only 3 bytes when we need 4
    buffer.extend_from_slice(&[0x00, 0x00, 0x00]);

    let result = codec.decode(&mut buffer);
    assert!(
        result.is_ok(),
        "Should return Ok(None) for incomplete frame, not error"
    );
    assert!(
        result.unwrap().is_none(),
        "Should return None for incomplete frame"
    );
}

#[test]
fn test_decode_invalid_json_payload() {
    let mut codec = MessageCodec;
    let mut buffer = BytesMut::new();

    // Valid length prefix, but invalid JSON
    let invalid_json = b"not json at all";
    buffer.extend_from_slice(&(invalid_json.len() as u32).to_be_bytes());
    buffer.extend_from_slice(invalid_json);

    let result = codec.decode(&mut buffer);
    assert!(result.is_err(), "Should reject invalid JSON payload");
}

#[test]
fn test_decode_wrong_message_type() {
    let mut codec = MessageCodec;
    let mut buffer = BytesMut::new();

    // Valid JSON but missing required fields
    let json = b"{}";
    buffer.extend_from_slice(&(json.len() as u32).to_be_bytes());
    buffer.extend_from_slice(json);

    let result = codec.decode(&mut buffer);
    assert!(
        result.is_err(),
        "Should reject JSON without required message fields"
    );
}

#[test]
fn test_decode_null_bytes_in_payload() {
    let mut codec = MessageCodec;
    let mut buffer = BytesMut::new();

    // JSON with null bytes - invalid UTF-8 sequence
    let payload = b"{\x00\x00invalid}";
    buffer.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    buffer.extend_from_slice(payload);

    let result = codec.decode(&mut buffer);
    // Should either error or return None (incomplete), but not panic
    let _ = result;
}

#[test]
fn test_decode_negative_length_value() {
    let mut codec = MessageCodec;
    let mut buffer = BytesMut::new();

    // Negative number as length prefix (as unsigned u32, this is a huge number)
    buffer.extend_from_slice(&(-1i32 as u32).to_be_bytes());
    buffer.extend_from_slice(b"some data");

    let result = codec.decode(&mut buffer);
    assert!(result.is_err(), "Should reject frame larger than max size");
}

#[test]
fn test_encode_message_preserves_all_fields() {
    let mut codec = MessageCodec;

    let msg = Message::CliRequest(CliRequest {
        id: "test-001".to_string(),
        tool: "gh".to_string(),
        argv: vec!["pr".to_string(), "list".to_string()],
        env: {
            let mut map = HashMap::new();
            map.insert("PATH".to_string(), "/usr/bin".to_string());
            map.insert("HOME".to_string(), "/home/user".to_string());
            map
        },
        stdin: Some("input data".to_string()),
        cwd: "/home/user".to_string(),
    });

    let mut buffer = BytesMut::new();
    codec
        .encode(msg.clone(), &mut buffer)
        .expect("Encode failed");

    let decoded = codec
        .decode(&mut buffer)
        .expect("Decode failed")
        .expect("No message");

    match (&msg, &decoded) {
        (Message::CliRequest(orig), Message::CliRequest(dec)) => {
            assert_eq!(orig.id, dec.id);
            assert_eq!(orig.tool, dec.tool);
            assert_eq!(orig.argv, dec.argv);
            assert_eq!(orig.env, dec.env);
            assert_eq!(orig.stdin, dec.stdin);
            assert_eq!(orig.cwd, dec.cwd);
        }
        _ => panic!("Message type mismatch"),
    }
}

#[test]
fn test_decode_empty_string_fields() {
    let mut codec = MessageCodec;
    let mut buffer = BytesMut::new();

    let msg = Message::CliRequest(CliRequest {
        id: "".to_string(),   // Empty
        tool: "".to_string(), // Empty
        argv: vec![],         // Empty
        env: HashMap::new(),  // Empty
        stdin: None,
        cwd: "".to_string(), // Empty
    });

    codec
        .encode(msg.clone(), &mut buffer)
        .expect("Encode failed");

    let decoded = codec
        .decode(&mut buffer)
        .expect("Decode failed")
        .expect("No message");

    match (&msg, &decoded) {
        (Message::CliRequest(orig), Message::CliRequest(dec)) => {
            assert_eq!(orig.id, dec.id);
            assert!(orig.id.is_empty());
            assert!(dec.id.is_empty());
        }
        _ => panic!("Message type mismatch"),
    }
}

#[test]
fn test_decode_very_large_string_fields() {
    let mut codec = MessageCodec;
    let mut buffer = BytesMut::new();

    let large_string = "x".repeat(10_000);

    let msg = Message::CliRequest(CliRequest {
        id: large_string.clone(),
        tool: large_string.clone(),
        argv: vec![large_string.clone(); 100],
        env: HashMap::new(),
        stdin: Some(large_string.clone()),
        cwd: large_string.clone(),
    });

    codec
        .encode(msg.clone(), &mut buffer)
        .expect("Encode failed");

    let decoded = codec
        .decode(&mut buffer)
        .expect("Decode failed")
        .expect("No message");

    match (&msg, &decoded) {
        (Message::CliRequest(orig), Message::CliRequest(dec)) => {
            assert_eq!(orig.id, dec.id);
            assert_eq!(orig.argv.len(), dec.argv.len());
        }
        _ => panic!("Message type mismatch"),
    }
}

#[test]
fn test_decode_unicode_in_all_fields() {
    let mut codec = MessageCodec;
    let mut buffer = BytesMut::new();

    let msg = Message::CliRequest(CliRequest {
        id: "测试-🎉-العربية".to_string(),
        tool: "тест-работа".to_string(),
        argv: vec!["αργument".to_string(), "参数".to_string()],
        env: {
            let mut map = HashMap::new();
            map.insert("变量".to_string(), "值🔧".to_string());
            map
        },
        stdin: Some("Ελληνικά 中文 עברית".to_string()),
        cwd: "/home/用户".to_string(),
    });

    codec
        .encode(msg.clone(), &mut buffer)
        .expect("Encode failed");

    let decoded = codec
        .decode(&mut buffer)
        .expect("Decode failed")
        .expect("No message");

    match (&msg, &decoded) {
        (Message::CliRequest(orig), Message::CliRequest(dec)) => {
            assert_eq!(orig.id, dec.id);
            assert_eq!(orig.tool, dec.tool);
            assert_eq!(orig.argv, dec.argv);
            assert_eq!(orig.cwd, dec.cwd);
        }
        _ => panic!("Message type mismatch"),
    }
}

#[test]
fn test_decode_special_characters_in_fields() {
    let mut codec = MessageCodec;
    let mut buffer = BytesMut::new();

    let msg = Message::CliRequest(CliRequest {
        id: "test\x00with\x00nulls".to_string(),
        tool: "test\nwith\nnewlines".to_string(),
        argv: vec!["tab\tseparated".to_string(), "quote\"inside".to_string()],
        env: {
            let mut map = HashMap::new();
            map.insert("key\rwith\rcarriage".to_string(), "value".to_string());
            map
        },
        stdin: None,
        cwd: "/path/with\\backslash".to_string(),
    });

    codec
        .encode(msg.clone(), &mut buffer)
        .expect("Encode failed");

    let decoded = codec
        .decode(&mut buffer)
        .expect("Decode failed")
        .expect("No message");

    match (&msg, &decoded) {
        (Message::CliRequest(orig), Message::CliRequest(dec)) => {
            assert_eq!(orig.id, dec.id);
            assert!(orig.id.contains('\0'));
        }
        _ => panic!("Message type mismatch"),
    }
}

#[test]
fn test_multiple_messages_in_buffer_with_errors() {
    let mut codec = MessageCodec;
    let mut buffer = BytesMut::new();

    // First valid message
    let msg1 = Message::CliRequest(CliRequest {
        id: "first".to_string(),
        tool: "test".to_string(),
        argv: vec![],
        env: HashMap::new(),
        stdin: None,
        cwd: "/".to_string(),
    });

    codec
        .encode(msg1.clone(), &mut buffer)
        .expect("First encode failed");

    // Decode the first message
    let decoded1 = codec
        .decode(&mut buffer)
        .expect("First decode failed")
        .expect("No first message");

    assert!(matches!(decoded1, Message::CliRequest(ref r) if r.id == "first"));

    // Now buffer is empty, try to decode more
    let result = codec.decode(&mut buffer);
    assert!(result.is_ok());
    assert!(result.unwrap().is_none()); // No message available
}

#[test]
fn test_frame_size_boundary_conditions() {
    let mut codec = MessageCodec;

    // Test with a message that's exactly at common boundaries
    let test_sizes = vec![
        1,           // Minimum
        255,         // 2^8 - 1
        256,         // 2^8
        65535,       // 2^16 - 1
        65536,       // 2^16
        1_000_000,   // 1MB
        10_000_000,  // 10MB
        100_000_000, // 100MB (near max)
    ];

    for size in test_sizes {
        let argv = vec!["x".repeat(size)];
        let msg = Message::CliRequest(CliRequest {
            id: "boundary-test".to_string(),
            tool: "test".to_string(),
            argv,
            env: HashMap::new(),
            stdin: None,
            cwd: "/".to_string(),
        });

        let mut buffer = BytesMut::new();
        let encode_result = codec.encode(msg.clone(), &mut buffer);

        // Messages under limit should encode successfully
        if encode_result.is_ok() {
            let decode_result = codec.decode(&mut buffer);
            assert!(
                decode_result.is_ok(),
                "Should decode message of size {}",
                size
            );
            assert!(
                decode_result.unwrap().is_some(),
                "Should have decoded a message"
            );
        }
        // If encode fails, that's also acceptable for huge messages
    }
}
