use std::{
    io::{self, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};

use aes::Aes256;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use cbc::{
    cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit},
    Encryptor,
};
use hmac::{Hmac, Mac};
use rand_core::{OsRng, RngCore};
use rsa::{pkcs8::DecodePublicKey, Pkcs1v15Encrypt, RsaPublicKey};
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tungstenite::{
    client::{client, IntoClientRequest},
    http::header::SEC_WEBSOCKET_PROTOCOL,
    Message, WebSocket,
};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    config::{Credentials, CREDENTIALS_SCHEMA},
    probe::{probe_structure, ProbeError, TankSensorCandidate},
};

const PUBLIC_KEY_PATH: &str = "/jdev/sys/getPublicKey";
const WEBSOCKET_PATH: &str = "/ws/rfc6455";
const MAX_PUBLIC_KEY_RESPONSE: usize = 16 * 1024;
const MAX_STRUCTURE_BYTES: usize = 2 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Error)]
pub enum LoxoneError {
    #[error("Loxone network error: {0}")]
    Io(#[from] io::Error),
    #[error("Loxone WebSocket error: {0}")]
    WebSocket(#[from] tungstenite::Error),
    #[error("invalid Loxone JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Loxone sensor probe failed: {0}")]
    Probe(#[from] ProbeError),
    #[error("unsupported Loxone response: {0}")]
    Protocol(String),
    #[error("Loxone authentication failed")]
    Authentication,
    #[error("Loxone read timed out")]
    Timeout,
    #[error("Loxone connection closed")]
    ConnectionClosed,
    #[error("Miniserver is out of service")]
    OutOfService,
}

impl LoxoneError {
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Io(_)
                | Self::WebSocket(_)
                | Self::Timeout
                | Self::ConnectionClosed
                | Self::OutOfService
        )
    }
}

#[derive(Debug)]
pub struct Provisioned {
    pub credentials: Credentials,
    pub candidates: Vec<TankSensorCandidate>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Packet {
    Text(String),
    Values(Vec<(String, f64)>),
    KeepAlive,
    OutOfService,
    Other,
}

pub struct Session {
    socket: WebSocket<TcpStream>,
    cipher: CommandCipher,
}

impl Session {
    pub fn provision(
        host: &str,
        username: &str,
        password: &str,
        client_uuid: &str,
    ) -> Result<Provisioned, LoxoneError> {
        let mut session = Self::open(host)?;
        let username_path = percent_encode(username.as_bytes());
        let key_response =
            session.encrypted_request(&format!("jdev/sys/getkey2/{username_path}"))?;
        let key_data = response_value(&key_response)?;
        let key = required_string(key_data, "key")?;
        let user_salt = required_string(key_data, "salt")?;
        let algorithm = HashAlgorithm::parse(required_string(key_data, "hashAlg")?)?;
        let mut credential_hash = hash_credentials(username, password, user_salt, key, algorithm)?;

        let request = format!(
            "jdev/sys/getjwt/{credential_hash}/{username_path}/4/{client_uuid}/Venus%20GX%20Loxone%20Tanks"
        );
        let token_response = session.encrypted_request(&request)?;
        credential_hash.zeroize();
        let token_data = response_value(&token_response)?;
        let token = required_string(token_data, "token")?.to_owned();
        let valid_until = required_u64(token_data, "validUntil")?;
        let token_rights = optional_u64(token_data, "tokenRights").unwrap_or_default();

        let structure = session.fetch_structure()?;
        let candidates = probe_structure(&structure)?;
        let _ = session.socket.close(None);
        Ok(Provisioned {
            credentials: Credentials {
                schema: CREDENTIALS_SCHEMA,
                client_uuid: client_uuid.to_owned(),
                token,
                valid_until,
                token_rights,
            },
            candidates,
        })
    }

    pub fn authenticated(host: &str, username: &str, token: &str) -> Result<Self, LoxoneError> {
        let mut session = Self::open(host)?;
        let username = percent_encode(username.as_bytes());
        let response = session.encrypted_request(&format!("authwithtoken/{token}/{username}"))?;
        ensure_success(&response)?;
        Ok(session)
    }

    pub fn fetch_and_probe(&mut self) -> Result<Vec<TankSensorCandidate>, LoxoneError> {
        let structure = self.fetch_structure()?;
        Ok(probe_structure(&structure)?)
    }

    pub fn refresh_token(
        &mut self,
        username: &str,
        credentials: &Credentials,
    ) -> Result<Credentials, LoxoneError> {
        let username = percent_encode(username.as_bytes());
        let response = self.encrypted_request(&format!(
            "jdev/sys/refreshjwt/{}/{username}",
            credentials.token
        ))?;
        let value = response_value(&response)?;
        Ok(Credentials {
            schema: CREDENTIALS_SCHEMA,
            client_uuid: credentials.client_uuid.clone(),
            token: required_string(value, "token")?.to_owned(),
            valid_until: required_u64(value, "validUntil")?,
            token_rights: optional_u64(value, "tokenRights").unwrap_or(credentials.token_rights),
        })
    }

    pub fn enable_updates(&mut self) -> Result<(), LoxoneError> {
        let command = self.cipher.encrypt("jdev/sps/enablebinstatusupdate")?;
        self.send_text(&command)
    }

    pub fn keep_alive(&mut self) -> Result<(), LoxoneError> {
        self.send_text("keepalive")
    }

    pub fn read_packet(&mut self) -> Result<Packet, LoxoneError> {
        let mut header = loop {
            let message = self.read_message()?;
            let Message::Binary(bytes) = message else {
                continue;
            };
            if !is_message_header(&bytes) {
                continue;
            }
            if bytes[2] & 0x01 != 0 {
                continue;
            }
            break bytes;
        };

        match header[1] {
            5 => Ok(Packet::OutOfService),
            6 => Ok(Packet::KeepAlive),
            mut identifier => {
                let first = self.read_message()?;
                let payload = if let Message::Binary(bytes) = &first {
                    if is_message_header(bytes) {
                        header = bytes.clone();
                        identifier = header[1];
                        if matches!(identifier, 5 | 6) {
                            return Ok(if identifier == 5 {
                                Packet::OutOfService
                            } else {
                                Packet::KeepAlive
                            });
                        }
                        self.read_message()?
                    } else {
                        first
                    }
                } else {
                    first
                };
                let announced_length = header_payload_length(&header);
                if message_length(&payload) != announced_length {
                    return Err(LoxoneError::Protocol(
                        "Loxone payload length did not match its header".to_owned(),
                    ));
                }
                decode_packet_payload(identifier, payload)
            }
        }
    }

    fn open(host: &str) -> Result<Self, LoxoneError> {
        let public_key = fetch_public_key(host)?;
        let socket_address = (host, 80)
            .to_socket_addrs()
            .map_err(LoxoneError::Io)?
            .find(|address| address.is_ipv4())
            .ok_or_else(|| LoxoneError::Protocol("Miniserver did not resolve".to_owned()))?;
        let stream = TcpStream::connect_timeout(&socket_address, IO_TIMEOUT)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;

        let mut request = format!("ws://{host}{WEBSOCKET_PATH}")
            .into_client_request()
            .map_err(|error| LoxoneError::Protocol(error.to_string()))?;
        request.headers_mut().insert(
            SEC_WEBSOCKET_PROTOCOL,
            "remotecontrol"
                .parse()
                .map_err(|_| LoxoneError::Protocol("invalid WebSocket protocol".to_owned()))?,
        );
        let (socket, _) = client(request, stream).map_err(|error| {
            LoxoneError::Protocol(format!("WebSocket handshake failed: {error}"))
        })?;
        let cipher = CommandCipher::new(&public_key)?;
        let exchange = format!("jdev/sys/keyexchange/{}", cipher.session_key);
        let mut session = Self { socket, cipher };
        session.send_text(&exchange)?;
        let response = session.read_text_response()?;
        ensure_success(&response)?;
        Ok(session)
    }

    fn encrypted_request(&mut self, command: &str) -> Result<Value, LoxoneError> {
        let command = self.cipher.encrypt(command)?;
        self.send_text(&command)?;
        let response = self.read_text_response()?;
        ensure_success(&response)?;
        Ok(response)
    }

    fn fetch_structure(&mut self) -> Result<Vec<u8>, LoxoneError> {
        self.send_text("data/LoxAPP3.json")?;
        match self.read_packet()? {
            Packet::Text(text) if text.len() <= MAX_STRUCTURE_BYTES => Ok(text.into_bytes()),
            Packet::Text(_) => Err(LoxoneError::Protocol(
                "Loxone Structure File is too large".to_owned(),
            )),
            _ => Err(LoxoneError::Protocol(
                "Miniserver did not return a Structure File".to_owned(),
            )),
        }
    }

    fn read_text_response(&mut self) -> Result<Value, LoxoneError> {
        match self.read_packet()? {
            Packet::Text(text) => Ok(serde_json::from_str(&text)?),
            _ => Err(LoxoneError::Protocol(
                "Miniserver did not return a text response".to_owned(),
            )),
        }
    }

    fn send_text(&mut self, command: &str) -> Result<(), LoxoneError> {
        self.socket.send(Message::Text(command.to_owned().into()))?;
        Ok(())
    }

    fn read_message(&mut self) -> Result<Message, LoxoneError> {
        loop {
            match self.socket.read() {
                Ok(Message::Ping(payload)) => {
                    self.socket.send(Message::Pong(payload))?;
                }
                Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
                Ok(Message::Close(_)) => {
                    return Err(LoxoneError::ConnectionClosed);
                }
                Ok(message) => return Ok(message),
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(LoxoneError::Timeout);
                }
                Err(error) => return Err(LoxoneError::WebSocket(error)),
            }
        }
    }
}

fn is_message_header(bytes: &[u8]) -> bool {
    bytes.len() == 8 && bytes[0] == 0x03
}

fn header_payload_length(header: &[u8]) -> usize {
    u32::from_le_bytes(
        header[4..8]
            .try_into()
            .expect("Loxone message header has four length bytes"),
    ) as usize
}

fn message_length(message: &Message) -> usize {
    match message {
        Message::Text(text) => text.len(),
        Message::Binary(bytes) => bytes.len(),
        _ => 0,
    }
}

fn decode_packet_payload(identifier: u8, payload: Message) -> Result<Packet, LoxoneError> {
    match (identifier, payload) {
        (0 | 1, Message::Text(text)) => Ok(Packet::Text(text.to_string())),
        (0 | 1, Message::Binary(bytes)) => String::from_utf8(bytes.to_vec())
            .map(Packet::Text)
            .map_err(|_| LoxoneError::Protocol("invalid text response".to_owned())),
        (2, Message::Binary(bytes)) => Ok(Packet::Values(parse_value_events(&bytes)?)),
        _ => Ok(Packet::Other),
    }
}

struct CommandCipher {
    key: [u8; 32],
    iv: [u8; 16],
    salt: Option<String>,
    session_key: String,
}

impl CommandCipher {
    fn new(public_key: &RsaPublicKey) -> Result<Self, LoxoneError> {
        let mut key = [0_u8; 32];
        let mut iv = [0_u8; 16];
        OsRng.fill_bytes(&mut key);
        OsRng.fill_bytes(&mut iv);
        let session_plaintext = Zeroizing::new(format!("{}:{}", hex_lower(&key), hex_lower(&iv)));
        let encrypted = public_key
            .encrypt(&mut OsRng, Pkcs1v15Encrypt, session_plaintext.as_bytes())
            .map_err(|error| LoxoneError::Protocol(format!("RSA key exchange failed: {error}")))?;
        Ok(Self {
            key,
            iv,
            salt: None,
            session_key: BASE64.encode(encrypted),
        })
    }

    fn encrypt(&mut self, command: &str) -> Result<Zeroizing<String>, LoxoneError> {
        let next_salt = random_salt();
        let mut plaintext = match self.salt.replace(next_salt.clone()) {
            Some(previous) => {
                Zeroizing::new(format!("nextSalt/{previous}/{next_salt}/{command}\0"))
            }
            None => Zeroizing::new(format!("salt/{next_salt}/{command}\0")),
        };
        let encrypted = Encryptor::<Aes256>::new(&self.key.into(), &self.iv.into())
            .encrypt_padded_vec_mut::<Pkcs7>(plaintext.as_bytes());
        plaintext.zeroize();
        let encoded = BASE64.encode(encrypted);
        Ok(Zeroizing::new(format!(
            "jdev/sys/enc/{}",
            percent_encode(encoded.as_bytes())
        )))
    }
}

impl Drop for CommandCipher {
    fn drop(&mut self) {
        self.key.zeroize();
        self.iv.zeroize();
        self.session_key.zeroize();
        if let Some(salt) = &mut self.salt {
            salt.zeroize();
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum HashAlgorithm {
    Sha1,
    Sha256,
}

impl HashAlgorithm {
    fn parse(value: &str) -> Result<Self, LoxoneError> {
        match value.to_ascii_uppercase().as_str() {
            "SHA1" => Ok(Self::Sha1),
            "SHA256" => Ok(Self::Sha256),
            _ => Err(LoxoneError::Protocol(format!(
                "unsupported Loxone hash algorithm {value}"
            ))),
        }
    }
}

fn hash_credentials(
    username: &str,
    password: &str,
    raw_user_salt: &str,
    hex_key: &str,
    algorithm: HashAlgorithm,
) -> Result<String, LoxoneError> {
    let key = decode_hex(hex_key)?;
    let password_input = Zeroizing::new(format!("{password}:{raw_user_salt}"));
    let password_hash = match algorithm {
        HashAlgorithm::Sha1 => hex_upper(&Sha1::digest(password_input.as_bytes())),
        HashAlgorithm::Sha256 => hex_upper(&Sha256::digest(password_input.as_bytes())),
    };
    let message = Zeroizing::new(format!("{username}:{password_hash}"));
    match algorithm {
        HashAlgorithm::Sha1 => {
            let mut hmac = <Hmac<Sha1> as Mac>::new_from_slice(&key)
                .map_err(|_| LoxoneError::Protocol("invalid Loxone HMAC key".to_owned()))?;
            hmac.update(message.as_bytes());
            Ok(hex_lower(&hmac.finalize().into_bytes()))
        }
        HashAlgorithm::Sha256 => {
            let mut hmac = <Hmac<Sha256> as Mac>::new_from_slice(&key)
                .map_err(|_| LoxoneError::Protocol("invalid Loxone HMAC key".to_owned()))?;
            hmac.update(message.as_bytes());
            Ok(hex_lower(&hmac.finalize().into_bytes()))
        }
    }
}

pub fn client_uuid(identity: &str) -> String {
    let digest = Sha256::digest(format!("venus-loxone-tanks\0{identity}").as_bytes());
    format!(
        "{}-{}-{}-{}",
        hex_lower(&digest[0..4]),
        hex_lower(&digest[4..6]),
        hex_lower(&digest[6..8]),
        hex_lower(&digest[8..16])
    )
}

pub fn machine_identity(fallback: &str) -> String {
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(value) = std::fs::read_to_string(path) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_owned();
            }
        }
    }
    fallback.to_owned()
}

fn fetch_public_key(host: &str) -> Result<RsaPublicKey, LoxoneError> {
    let response = http_get(host, PUBLIC_KEY_PATH, MAX_PUBLIC_KEY_RESPONSE)?;
    let json: Value = serde_json::from_slice(&response)?;
    ensure_success(&json)?;
    let value = response_value(&json)?
        .as_str()
        .ok_or_else(|| LoxoneError::Protocol("missing Miniserver public key".to_owned()))?;
    let pem = normalize_public_key_pem(value)?;
    RsaPublicKey::from_public_key_pem(&pem)
        .map_err(|error| LoxoneError::Protocol(format!("invalid Miniserver public key: {error}")))
}

fn normalize_public_key_pem(value: &str) -> Result<String, LoxoneError> {
    let body = value
        .strip_prefix("-----BEGIN CERTIFICATE-----")
        .and_then(|value| value.strip_suffix("-----END CERTIFICATE-----"))
        .ok_or_else(|| LoxoneError::Protocol("invalid Miniserver public key markers".to_owned()))?;
    if body.is_empty()
        || !body
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
    {
        return Err(LoxoneError::Protocol(
            "invalid Miniserver public key encoding".to_owned(),
        ));
    }

    let mut pem = String::from("-----BEGIN PUBLIC KEY-----\n");
    for line in body.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(line).expect("base64 public key is ASCII"));
        pem.push('\n');
    }
    pem.push_str("-----END PUBLIC KEY-----\n");
    Ok(pem)
}

fn http_get(host: &str, path: &str, limit: usize) -> Result<Vec<u8>, LoxoneError> {
    let address = (host, 80)
        .to_socket_addrs()
        .map_err(LoxoneError::Io)?
        .find(|address| address.is_ipv4())
        .ok_or_else(|| LoxoneError::Protocol("Miniserver did not resolve".to_owned()))?;
    let mut stream = TcpStream::connect_timeout(&address, IO_TIMEOUT)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = Vec::new();
    stream
        .take((limit + 4096) as u64)
        .read_to_end(&mut response)?;
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
        .ok_or_else(|| LoxoneError::Protocol("invalid Miniserver HTTP response".to_owned()))?;
    let status = response
        .get(..header_end)
        .and_then(|header| std::str::from_utf8(header).ok())
        .and_then(|header| header.lines().next())
        .unwrap_or_default();
    if !status.contains(" 200 ") {
        return Err(LoxoneError::Protocol(format!(
            "Miniserver HTTP request failed: {status}"
        )));
    }
    let body = response.split_off(header_end);
    if body.len() > limit {
        return Err(LoxoneError::Protocol(
            "Miniserver HTTP response is too large".to_owned(),
        ));
    }
    Ok(body)
}

fn ensure_success(response: &Value) -> Result<(), LoxoneError> {
    let code = response
        .get("LL")
        .and_then(Value::as_object)
        .and_then(|value| value.get("Code").or_else(|| value.get("code")))
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_u64().map(|v| v.to_string()))
        })
        .ok_or_else(|| LoxoneError::Protocol("missing Loxone response code".to_owned()))?;
    match code.as_str() {
        "200" => Ok(()),
        "401" | "403" | "423" => Err(LoxoneError::Authentication),
        _ => Err(LoxoneError::Protocol(format!(
            "Miniserver returned code {code}"
        ))),
    }
}

fn response_value(response: &Value) -> Result<&Value, LoxoneError> {
    response
        .get("LL")
        .and_then(|value| value.get("value"))
        .ok_or_else(|| LoxoneError::Protocol("missing Loxone response value".to_owned()))
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str, LoxoneError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| LoxoneError::Protocol(format!("missing Loxone {key}")))
}

fn required_u64(value: &Value, key: &str) -> Result<u64, LoxoneError> {
    optional_u64(value, key).ok_or_else(|| LoxoneError::Protocol(format!("missing Loxone {key}")))
}

fn optional_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(|value| {
        value
            .as_u64()
            .or_else(|| {
                value
                    .as_f64()
                    .filter(|value| *value >= 0.0)
                    .map(|value| value as u64)
            })
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn parse_value_events(payload: &[u8]) -> Result<Vec<(String, f64)>, LoxoneError> {
    if !payload.len().is_multiple_of(24) {
        return Err(LoxoneError::Protocol(
            "invalid Loxone value event table".to_owned(),
        ));
    }
    let mut values = Vec::with_capacity(payload.len() / 24);
    for entry in payload.chunks_exact(24) {
        let uuid = binary_uuid(&entry[..16]);
        let value = f64::from_le_bytes(
            entry[16..24]
                .try_into()
                .expect("value event has exactly eight value bytes"),
        );
        if value.is_finite() {
            values.push((uuid, value));
        }
    }
    Ok(values)
}

fn binary_uuid(bytes: &[u8]) -> String {
    let first = u32::from_le_bytes(bytes[0..4].try_into().expect("UUID first field"));
    let second = u16::from_le_bytes(bytes[4..6].try_into().expect("UUID second field"));
    let third = u16::from_le_bytes(bytes[6..8].try_into().expect("UUID third field"));
    format!(
        "{first:08x}-{second:04x}-{third:04x}-{}",
        hex_lower(&bytes[8..16])
    )
}

fn random_salt() -> String {
    let mut salt = [0_u8; 8];
    OsRng.fill_bytes(&mut salt);
    hex_lower(&salt)
}

fn decode_hex(value: &str) -> Result<Vec<u8>, LoxoneError> {
    if !value.len().is_multiple_of(2) {
        return Err(LoxoneError::Protocol(
            "invalid hexadecimal value".to_owned(),
        ));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(value: u8) -> Result<u8, LoxoneError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(LoxoneError::Protocol(
            "invalid hexadecimal value".to_owned(),
        )),
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn hex_upper(bytes: &[u8]) -> String {
    hex_lower(bytes).to_ascii_uppercase()
}

fn percent_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::with_capacity(bytes.len());
    for byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(*byte as char);
        } else {
            output.push('%');
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loxone_binary_uuid_uses_mixed_endianness() {
        let bytes = [
            0xaf, 0x65, 0x55, 0x10, 0x94, 0x01, 0x1c, 0x3c, 0xff, 0xff, 0xfd, 0x7d, 0x9b, 0x15,
            0x18, 0xeb,
        ];
        assert_eq!(binary_uuid(&bytes), "105565af-0194-3c1c-fffffd7d9b1518eb");
    }

    #[test]
    fn value_events_are_parsed_without_persistence() {
        let mut payload = vec![
            0xaf, 0x65, 0x55, 0x10, 0x94, 0x01, 0x1c, 0x3c, 0xff, 0xff, 0xfd, 0x7d, 0x9b, 0x15,
            0x18, 0xeb,
        ];
        payload.extend_from_slice(&66.8_f64.to_le_bytes());
        let values = parse_value_events(&payload).unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].0, "105565af-0194-3c1c-fffffd7d9b1518eb");
        assert!((values[0].1 - 66.8).abs() < f64::EPSILON);
    }

    #[test]
    fn client_uuid_is_stable_and_loxone_shaped() {
        let first = client_uuid("device-a");
        assert_eq!(first, client_uuid("device-a"));
        assert_ne!(first, client_uuid("device-b"));
        assert_eq!(
            first.split('-').map(str::len).collect::<Vec<_>>(),
            [8, 4, 4, 16]
        );
    }

    #[test]
    fn percent_encoding_matches_websocket_commands() {
        assert_eq!(percent_encode(b"a+b/c="), "a%2Bb%2Fc%3D");
    }

    #[test]
    fn gen1_single_line_public_key_is_normalized_and_parsed() {
        let value = "-----BEGIN CERTIFICATE-----MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDZX6k4i7DUOK4euk+jsI22/GfWrCtByHNU2tQWhFptXDxpsHEsoOhs7iDicM0PCM/6RxNtMJsi2DDNMmJ1enuRx9Mv/uV184iPI07Kqz1KPxdueI2rYtoeGRwatafSmYHuhEnge0xFO/PEFMUCB/DVd8JO+In1BbHJtZO2KZYoMQIDAQAB-----END CERTIFICATE-----";
        let pem = normalize_public_key_pem(value).unwrap();
        assert!(pem.lines().nth(1).unwrap().len() <= 64);
        assert!(RsaPublicKey::from_public_key_pem(&pem).is_ok());
    }

    #[test]
    fn structure_file_frames_accept_the_gen1_file_identifier() {
        let packet =
            decode_packet_payload(1, Message::Binary(br#"{"controls":{}}"#.to_vec().into()))
                .unwrap();
        assert_eq!(packet, Packet::Text(r#"{"controls":{}}"#.to_owned()));
    }

    #[test]
    fn message_header_length_is_little_endian() {
        let header = [0x03, 0, 0, 0, 0x34, 0x12, 0, 0];
        assert!(is_message_header(&header));
        assert_eq!(header_payload_length(&header), 0x1234);
    }
}
