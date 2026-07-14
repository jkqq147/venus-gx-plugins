use std::{
    collections::BTreeSet,
    fs, io,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket},
    time::{Duration, Instant},
};

use thiserror::Error;

const CFG_API_PATH: &str = "/jdev/cfg/api";
const MAX_CANDIDATES: usize = 32;
const MAX_RESPONSE_BYTES: u64 = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredMiniserver {
    pub address: String,
    pub serial: String,
    pub version: String,
}

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("invalid Miniserver address")]
    InvalidAddress,
    #[error("Miniserver address could not be resolved")]
    Resolve,
    #[error("Miniserver connection failed: {0}")]
    Io(#[from] io::Error),
    #[error("the endpoint is not a Loxone Miniserver")]
    NotLoxone,
}

pub fn discover(timeout: Duration) -> Vec<DiscoveredMiniserver> {
    let mut candidates = arp_candidates("/proc/net/arp");
    candidates.extend(ssdp_candidates(timeout));
    candidates
        .into_iter()
        .take(MAX_CANDIDATES)
        .filter_map(|address| {
            verify_miniserver(&address.to_string(), Duration::from_millis(500)).ok()
        })
        .collect()
}

pub fn verify_miniserver(
    address: &str,
    timeout: Duration,
) -> Result<DiscoveredMiniserver, DiscoveryError> {
    let host = normalize_address(address)?;
    let socket = (host.as_str(), 80)
        .to_socket_addrs()
        .map_err(|_| DiscoveryError::Resolve)?
        .find(|address| address.is_ipv4())
        .ok_or(DiscoveryError::Resolve)?;
    let mut stream = TcpStream::connect_timeout(&socket, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    write!(
        stream,
        "GET {CFG_API_PATH} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )?;
    let mut response = Vec::new();
    stream
        .take(MAX_RESPONSE_BYTES)
        .read_to_end(&mut response)
        .map_err(DiscoveryError::Io)?;
    let mut parsed = parse_cfg_api_response(&response)?;
    parsed.address = host;
    Ok(parsed)
}

pub fn normalize_address(address: &str) -> Result<String, DiscoveryError> {
    let mut value = address.trim();
    if let Some(stripped) = value.strip_prefix("http://") {
        value = stripped;
    } else if value.starts_with("https://") {
        return Err(DiscoveryError::InvalidAddress);
    }
    value = value.trim_end_matches('/');
    if value.is_empty()
        || value.len() > 253
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(DiscoveryError::InvalidAddress);
    }
    Ok(value.to_owned())
}

fn arp_candidates(path: &str) -> BTreeSet<IpAddr> {
    fs::read_to_string(path)
        .ok()
        .map(|contents| parse_arp(&contents))
        .unwrap_or_default()
}

fn parse_arp(contents: &str) -> BTreeSet<IpAddr> {
    contents
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 4 || fields[2] != "0x2" {
                return None;
            }
            fields[0].parse::<IpAddr>().ok()
        })
        .collect()
}

fn ssdp_candidates(timeout: Duration) -> BTreeSet<IpAddr> {
    let mut candidates = BTreeSet::new();
    let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else {
        return candidates;
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));
    let request = b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 1\r\nST: ssdp:all\r\n\r\n";
    if socket
        .send_to(request, SocketAddr::from(([239, 255, 255, 250], 1900)))
        .is_err()
    {
        return candidates;
    }

    let deadline = Instant::now() + timeout;
    let mut buffer = [0_u8; 2048];
    while Instant::now() < deadline && candidates.len() < MAX_CANDIDATES {
        match socket.recv_from(&mut buffer) {
            Ok((_, source)) => {
                candidates.insert(source.ip());
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    candidates
}

fn parse_cfg_api_response(response: &[u8]) -> Result<DiscoveredMiniserver, DiscoveryError> {
    let text = std::str::from_utf8(response).map_err(|_| DiscoveryError::NotLoxone)?;
    let (headers, body) = text
        .split_once("\r\n\r\n")
        .ok_or(DiscoveryError::NotLoxone)?;
    if !headers
        .lines()
        .next()
        .is_some_and(|line| line.contains(" 200 "))
    {
        return Err(DiscoveryError::NotLoxone);
    }
    let serial = extract_single_quoted(body, "snr").ok_or(DiscoveryError::NotLoxone)?;
    let version = extract_single_quoted(body, "version").ok_or(DiscoveryError::NotLoxone)?;
    if !serial.contains(':') || !version.contains('.') || !body.contains("\"LL\"") {
        return Err(DiscoveryError::NotLoxone);
    }
    Ok(DiscoveredMiniserver {
        address: String::new(),
        serial,
        version,
    })
}

fn extract_single_quoted(body: &str, key: &str) -> Option<String> {
    let marker = format!("'{key}'");
    let remainder = body.split_once(&marker)?.1;
    let remainder = remainder.split_once(':')?.1.trim_start();
    let remainder = remainder.strip_prefix('\'')?;
    Some(remainder.split_once('\'')?.0.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_cfg_api_shape() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"LL\": { \"control\": \"dev/cfg/api\", \"value\": \"{'snr': '50:4F:94:11:C5:0F', 'version':'15.2.10.14', 'local':true}\", \"Code\": \"200\"}}";
        let parsed = parse_cfg_api_response(response).unwrap();
        assert_eq!(parsed.serial, "50:4F:94:11:C5:0F");
        assert_eq!(parsed.version, "15.2.10.14");
    }

    #[test]
    fn rejects_ordinary_http_service() {
        let response = b"HTTP/1.1 200 OK\r\n\r\n<html>hello</html>";
        assert!(matches!(
            parse_cfg_api_response(response),
            Err(DiscoveryError::NotLoxone)
        ));
    }

    #[test]
    fn only_complete_arp_neighbors_are_candidates() {
        let arp = "IP address HW type Flags HW address Mask Device\n192.0.2.2 0x1 0x2 50:4f:94:11:c5:0f * eth0\n192.0.2.9 0x1 0x0 00:00:00:00:00:00 * eth0\n";
        assert_eq!(
            parse_arp(arp),
            BTreeSet::from(["192.0.2.2".parse().unwrap()])
        );
    }

    #[test]
    fn normalizes_ip_hostname_and_http_url() {
        assert_eq!(normalize_address(" 192.0.2.2 ").unwrap(), "192.0.2.2");
        assert_eq!(
            normalize_address("http://Loxone.local/").unwrap(),
            "Loxone.local"
        );
        assert!(normalize_address("https://Loxone.local").is_err());
        assert!(normalize_address("host/path").is_err());
    }
}
