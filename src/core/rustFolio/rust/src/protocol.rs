// protocol.rs — Message framing and operation codes.
//
// Wire format:
//   [4 bytes] total message length (big-endian u32, excludes these 4 bytes)
//   [1 byte]  message type: request=0x01, response=0x02, error=0x03
//   [2 bytes] operation code (big-endian u16)
//   [N bytes] payload (operation-specific)
//
// Handshake (first message after connect):
//   Request:  [4 bytes magic "RFOL"] [2 bytes protocol version]
//   Response: [4 bytes magic "RFOL"] [2 bytes protocol version] [1 byte status 0x00=ok]

use std::io::{Read, Write};
use crate::error::{FolioError, Result};

pub const MAGIC: &[u8; 4] = b"RFOL";
pub const PROTOCOL_VERSION: u16 = 0x0001;
pub const MAX_MSG_SIZE: u32 = 64 * 1024 * 1024; // 64 MB

/// Message type byte
pub mod msg_type {
    pub const REQUEST:  u8 = 0x01;
    pub const RESPONSE: u8 = 0x02;
    pub const ERROR:    u8 = 0x03;
}

/// Operation codes
pub mod opcode {
    pub const OPEN:       u16 = 0x0001;
    pub const CLOSE:      u16 = 0x0002;
    pub const SYNC:       u16 = 0x0003;
    pub const READ_BY_ID:  u16 = 0x0010;
    pub const READ_BY_IDS: u16 = 0x0011;
    pub const READ_ALL:    u16 = 0x0012;
    pub const READ_COUNT:  u16 = 0x0013;
    pub const COMMIT_ALL:  u16 = 0x0020;
    pub const CUR_VER:     u16 = 0x0030;
    pub const FLUSH_MODE:  u16 = 0x0031;
    pub const FLUSH:       u16 = 0x0032;
    pub const HIS_READ:      u16 = 0x0040;
    pub const HIS_WRITE:     u16 = 0x0041;
    pub const HIS_STAT:      u16 = 0x0042;
    pub const BACKUP_CREATE: u16 = 0x0050;
    pub const SPEC_UPDATE:   u16 = 0x0060;
}

/// Length of the auth token: 32 raw bytes = 256 bits of entropy.
/// Transmitted as raw bytes in the handshake (stored as 64 hex chars in the port file).
pub const TOKEN_LEN: usize = 32;

/// Constant-time byte-slice equality.  Prevents timing oracles on token comparison.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Perform the version + auth handshake (server side).
///
/// Handshake wire format:
///   Client → Server: [magic 4][version 2][token TOKEN_LEN] = 38 bytes
///   Server → Client: [magic 4][version 2][status 1]        =  7 bytes
///     status 0x00 = ok
///     status 0x01 = version mismatch
///     status 0x02 = auth failed
pub fn server_handshake<S: Read + Write>(stream: &mut S, expected_token: &[u8; TOKEN_LEN]) -> Result<()> {
    // Read client handshake: 4 magic + 2 version + TOKEN_LEN token
    let mut buf = [0u8; 6 + TOKEN_LEN];
    stream.read_exact(&mut buf)?;

    if &buf[0..4] != MAGIC {
        return Err(FolioError::Protocol("Bad magic bytes in handshake".into()));
    }
    let client_ver = u16::from_be_bytes([buf[4], buf[5]]);
    if client_ver != PROTOCOL_VERSION {
        let mut resp = Vec::with_capacity(7);
        resp.extend_from_slice(MAGIC);
        resp.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
        resp.push(0x01); // status: version mismatch
        stream.write_all(&resp)?;
        return Err(FolioError::Protocol(format!(
            "Protocol version mismatch: client={:#06x} server={:#06x}",
            client_ver, PROTOCOL_VERSION
        )));
    }

    // Validate token with constant-time comparison.
    let client_token = &buf[6..6 + TOKEN_LEN];
    if !ct_eq(client_token, expected_token) {
        let mut resp = Vec::with_capacity(7);
        resp.extend_from_slice(MAGIC);
        resp.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
        resp.push(0x02); // status: auth failed
        stream.write_all(&resp)?;
        stream.flush()?;
        return Err(FolioError::AuthFailed);
    }

    // Write acceptance.
    let mut resp = Vec::with_capacity(7);
    resp.extend_from_slice(MAGIC);
    resp.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    resp.push(0x00); // status: ok
    stream.write_all(&resp)?;
    stream.flush()?;
    Ok(())
}

/// Perform the version + auth handshake (client side).
/// Used only in tests to drive the server handshake without a live Fantom process.
#[cfg(test)]
pub fn client_handshake<S: Read + Write>(stream: &mut S, token: &[u8; TOKEN_LEN]) -> Result<()> {
    // Send: 4 magic + 2 version + TOKEN_LEN token
    let mut req = Vec::with_capacity(6 + TOKEN_LEN);
    req.extend_from_slice(MAGIC);
    req.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    req.extend_from_slice(token);
    stream.write_all(&req)?;
    stream.flush()?;

    // Read server response: 7 bytes
    let mut buf = [0u8; 7];
    stream.read_exact(&mut buf)?;

    if &buf[0..4] != MAGIC {
        return Err(FolioError::Protocol("Bad magic in server handshake response".into()));
    }
    match buf[6] {
        0x00 => Ok(()),
        0x01 => Err(FolioError::Protocol("Server rejected handshake: version mismatch".into())),
        0x02 => Err(FolioError::AuthFailed),
        s    => Err(FolioError::Protocol(format!("Server rejected handshake: unknown status {:#04x}", s))),
    }
}

/// A decoded request message.
/// Used only in tests; the live server decodes directly in read_message.
#[cfg(test)]
#[derive(Debug)]
pub struct Request {
    pub opcode:  u16,
    pub payload: Vec<u8>,
}

/// Read one framed message from the stream.
pub fn read_message<S: Read>(stream: &mut S) -> Result<(u8, u16, Vec<u8>)> {
    // Read 4-byte length
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let total_len = u32::from_be_bytes(len_buf);

    if total_len > MAX_MSG_SIZE {
        return Err(FolioError::Protocol(format!(
            "Message too large: {} bytes (max {})", total_len, MAX_MSG_SIZE
        )));
    }

    // Read body: 1 msg_type + 2 opcode + N payload
    let body_len = total_len as usize;
    if body_len < 3 {
        return Err(FolioError::Protocol("Message too short".into()));
    }
    let mut body = vec![0u8; body_len];
    stream.read_exact(&mut body)?;

    let msg_type = body[0];
    let opcode   = u16::from_be_bytes([body[1], body[2]]);
    let payload  = body[3..].to_vec();

    Ok((msg_type, opcode, payload))
}

/// Write a response message to the stream.
pub fn write_response<S: Write>(stream: &mut S, opcode: u16, payload: &[u8]) -> Result<()> {
    write_message(stream, msg_type::RESPONSE, opcode, payload)
}

/// Write an error response message.
pub fn write_error<S: Write>(stream: &mut S, opcode: u16, err: &FolioError) -> Result<()> {
    let code = err.wire_code();
    let msg = err.to_string();
    let msg_bytes = msg.as_bytes();

    let mut payload = Vec::with_capacity(4 + msg_bytes.len());
    payload.extend_from_slice(&code.to_be_bytes());
    write_str_to(&mut payload, &msg);
    write_message(stream, msg_type::ERROR, opcode, &payload)
}

fn write_message<S: Write>(stream: &mut S, mtype: u8, opcode: u16, payload: &[u8]) -> Result<()> {
    // total_len = 1 (msg_type) + 2 (opcode) + payload.len()
    let total_len = (1 + 2 + payload.len()) as u32;

    let mut buf = Vec::with_capacity(4 + total_len as usize);
    buf.extend_from_slice(&total_len.to_be_bytes());
    buf.push(mtype);
    buf.extend_from_slice(&opcode.to_be_bytes());
    buf.extend_from_slice(payload);

    stream.write_all(&buf)?;
    stream.flush()?;
    Ok(())
}

// ── Payload helpers ─────────────────────────────────────────────────────────

/// Write a length-prefixed UTF-8 string.
/// Strings up to 65534 bytes: u16 length prefix.
/// Strings 65535+ bytes: 0xFFFF marker + u32 length prefix.
pub fn write_str_to(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    if bytes.len() < 0xFFFF {
        buf.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    } else {
        buf.extend_from_slice(&[0xFF, 0xFF]);
        buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    }
    buf.extend_from_slice(bytes);
}

/// Read a length-prefixed UTF-8 string from a cursor.
pub fn read_str_from(data: &[u8], pos: &mut usize) -> Result<String> {
    let len = read_u16(data, pos)? as usize;
    let len = if len == 0xFFFF {
        read_u32(data, pos)? as usize
    } else {
        len
    };
    let end = *pos + len;
    if end > data.len() {
        return Err(FolioError::Protocol("String extends beyond buffer".into()));
    }
    let s = std::str::from_utf8(&data[*pos..end])
        .map_err(|e| FolioError::Protocol(format!("Invalid UTF-8: {}", e)))?
        .to_string();
    *pos = end;
    Ok(s)
}

pub fn read_u8(data: &[u8], pos: &mut usize) -> Result<u8> {
    if *pos >= data.len() {
        return Err(FolioError::Protocol("Unexpected end of data (u8)".into()));
    }
    let v = data[*pos];
    *pos += 1;
    Ok(v)
}

pub fn read_u16(data: &[u8], pos: &mut usize) -> Result<u16> {
    if *pos + 2 > data.len() {
        return Err(FolioError::Protocol("Unexpected end of data (u16)".into()));
    }
    let v = u16::from_be_bytes([data[*pos], data[*pos+1]]);
    *pos += 2;
    Ok(v)
}

pub fn read_u32(data: &[u8], pos: &mut usize) -> Result<u32> {
    if *pos + 4 > data.len() {
        return Err(FolioError::Protocol("Unexpected end of data (u32)".into()));
    }
    let v = u32::from_be_bytes([data[*pos], data[*pos+1], data[*pos+2], data[*pos+3]]);
    *pos += 4;
    Ok(v)
}

pub fn read_u64(data: &[u8], pos: &mut usize) -> Result<u64> {
    if *pos + 8 > data.len() {
        return Err(FolioError::Protocol("Unexpected end of data (u64)".into()));
    }
    let v = u64::from_be_bytes(data[*pos..*pos+8].try_into().expect("8-byte slice guaranteed by bounds check above"));
    *pos += 8;
    Ok(v)
}

pub fn read_i64(data: &[u8], pos: &mut usize) -> Result<i64> {
    if *pos + 8 > data.len() {
        return Err(FolioError::Protocol("Unexpected end of data (i64)".into()));
    }
    let v = i64::from_be_bytes(data[*pos..*pos+8].try_into().expect("8-byte slice guaranteed by bounds check above"));
    *pos += 8;
    Ok(v)
}

pub fn read_f64(data: &[u8], pos: &mut usize) -> Result<f64> {
    if *pos + 8 > data.len() {
        return Err(FolioError::Protocol("Unexpected end of data (f64)".into()));
    }
    let bits = u64::from_be_bytes(data[*pos..*pos+8].try_into().expect("8-byte slice guaranteed by bounds check above"));
    *pos += 8;
    Ok(f64::from_bits(bits))
}

pub fn write_u8(buf: &mut Vec<u8>, v: u8) {
    buf.push(v);
}

pub fn write_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_be_bytes());
}

pub fn write_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

pub fn write_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_be_bytes());
}

pub fn write_i64(buf: &mut Vec<u8>, v: i64) {
    buf.extend_from_slice(&v.to_be_bytes());
}

pub fn write_f64(buf: &mut Vec<u8>, v: f64) {
    buf.extend_from_slice(&v.to_bits().to_be_bytes());
}
