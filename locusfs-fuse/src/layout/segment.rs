use locusfs_graph::{GraphError, Result};

pub fn encode_segment(raw: &str) -> Result<String> {
    validate_segment(raw)?;
    let mut encoded = String::new();
    for byte in raw.bytes() {
        if is_plain_segment_byte(byte) {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte & 0x0f));
        }
    }
    Ok(encoded)
}

pub fn decode_segment(encoded: &str) -> Result<String> {
    validate_segment(encoded)?;
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(GraphError::InvalidEncoding {
                    segment: encoded.to_string(),
                });
            }
            let high = hex_value(bytes[index + 1]).ok_or_else(|| GraphError::InvalidEncoding {
                segment: encoded.to_string(),
            })?;
            let low = hex_value(bytes[index + 2]).ok_or_else(|| GraphError::InvalidEncoding {
                segment: encoded.to_string(),
            })?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    let value = String::from_utf8(decoded).map_err(|_| GraphError::InvalidEncoding {
        segment: encoded.to_string(),
    })?;
    validate_segment(&value)?;
    Ok(value)
}

fn validate_segment(value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(GraphError::InvalidPathSegment {
            segment: value.to_string(),
            reason: "empty",
        });
    }

    if value == "." || value == ".." {
        return Err(GraphError::InvalidPathSegment {
            segment: value.to_string(),
            reason: "reserved path segment",
        });
    }

    if value.contains('\0') {
        return Err(GraphError::InvalidPathSegment {
            segment: value.to_string(),
            reason: "contains NUL",
        });
    }

    Ok(())
}

fn is_plain_segment_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + value - 10) as char,
        _ => unreachable!("nibble is always <= 15"),
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
