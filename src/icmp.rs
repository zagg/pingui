//! Native ICMP echo implementation — no external `ping` process involved.
//!
//! Socket strategy (best effort, in order):
//! * IPv6: `SOCK_DGRAM`/`IPPROTO_ICMPV6` (unprivileged on Linux/macOS), then raw.
//! * IPv4 Unix: raw `SOCK_RAW` (root/CAP_NET_RAW), then `SOCK_DGRAM` ICMP
//!   (unprivileged via `net.ipv4.ping_group_range`).
//! * IPv4 Windows: raw socket bound to the outbound interface + `SIO_RCVALL`
//!   when privileges allow (admin); on Windows 10+ usually fine.

use anyhow::{anyhow, Context, Result};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use crate::app::Sample;

pub const ICMPV4_ECHO_REQUEST: u8 = 8;
pub const ICMPV4_ECHO_REPLY: u8 = 0;
pub const ICMPV4_DEST_UNREACH: u8 = 3;
pub const ICMPV6_ECHO_REQUEST: u8 = 128;
pub const ICMPV6_ECHO_REPLY: u8 = 129;
/// First 8 payload bytes carry the send timestamp (for RTT).
const HEADER_TS_LEN: usize = 8;

pub struct EchoRequest {
    pub bytes: Vec<u8>,
}

/// RFC 1071 internet checksum. Summing big-endian words makes the result
/// independent of host endianness; a packet containing its own checksum
/// re-verifyies to `0`.
pub fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u32::from(u16::from_be_bytes([data[i], data[i + 1]]));
        i += 2;
    }
    if i < data.len() {
        sum += u32::from(data[i]) << 8;
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

pub fn build_request(
    ip: IpAddr,
    id: u16,
    seq: u16,
    payload_len: usize,
    now: Instant,
    session_start: Instant,
) -> EchoRequest {
    let micros = now
        .duration_since(session_start)
        .as_micros()
        .min(u64::MAX as u128) as u64;
    let (ty, v6) = match ip {
        IpAddr::V4(_) => (ICMPV4_ECHO_REQUEST, false),
        IpAddr::V6(_) => (ICMPV6_ECHO_REQUEST, true),
    };
    let mut body = Vec::with_capacity(64 + payload_len);
    body.extend_from_slice(&[ty, 0, 0, 0]);
    body.extend_from_slice(&id.to_be_bytes());
    body.extend_from_slice(&seq.to_be_bytes());
    body.extend_from_slice(&micros.to_be_bytes());
    for i in 0..payload_len.max(16) - HEADER_TS_LEN {
        body.push(b'a' + (i % 26) as u8);
    }
    if !v6 {
        // IPv6 checksum is computed by the kernel (pseudohdr) — must stay 0.
        let ck = checksum(&body);
        body[2..4].copy_from_slice(&ck.to_be_bytes());
    }
    EchoRequest { bytes: body }
}

pub struct ParsedIcmp {
    pub msg_type: u8,
    pub id: u16,
    pub seq: u16,
    pub ttl: Option<u8>,
    pub body: Vec<u8>,
}

/// Parse an ICMP datagram, tolerating the leading IPv4 header that stacks
/// include (or omit) inconsistently across platforms and socket types.
pub fn parse(data: &[u8]) -> Option<ParsedIcmp> {
    if data.len() >= 20 && data[0] >> 4 == 4 {
        let ihl = ((data[0] & 0x0f) as usize) * 4;
        let total = u16::from_be_bytes([data[2], data[3]]) as usize;
        if ihl >= 20 && total >= ihl + 8 && total <= data.len() {
            return parse_icmp(&data[ihl..], Some(data[8]));
        }
    }
    parse_icmp(data, None)
}

fn parse_icmp(data: &[u8], ttl: Option<u8>) -> Option<ParsedIcmp> {
    if data.len() < 8 {
        return None;
    }
    Some(ParsedIcmp {
        msg_type: data[0],
        id: u16::from_be_bytes([data[4], data[5]]),
        seq: u16::from_be_bytes([data[6], data[7]]),
        ttl,
        body: data[8..].to_vec(),
    })
}

/// Owns the ICMP socket, matches replies back to requests, tracks RTT.
pub struct Pinger {
    sock: Socket,
    dst: SocketAddr,
    id: u16,
    next_seq: u16,
    session_start: Instant,
    sent: HashMap<u16, Instant>,
    payload_len: usize,
    recv_buf: [u8; 2048],
    pub protocol_label: &'static str,
}

impl Pinger {
    pub fn new(dst: SocketAddr, payload_len: usize) -> Result<Self> {
        let (sock, id, protocol_label) = open_socket(dst)?;
        let _ = sock.set_nonblocking(true); // le pump TUI/GUI ne bloque jamais une frame
        Ok(Pinger {
            sock,
            dst,
            id,
            next_seq: 0,
            session_start: Instant::now(),
            sent: HashMap::new(),
            payload_len,
            recv_buf: [0u8; 2048],
            protocol_label,
        })
    }

    /// Changer la taille du chargement pour les prochains paquets.
    pub fn set_payload(&mut self, len: usize) {
        self.payload_len = len.max(16).min(60_000);
    }

    pub fn send(&mut self) -> Result<()> {
        let now = Instant::now();
        let seq = self.next_seq.wrapping_add(1);
        self.next_seq = seq;
        let req = build_request(self.dst.ip(), self.id, seq, self.payload_len, now, self.session_start);
        self.sent.insert(seq, now);
        let target = SockAddr::from(self.dst);
        match self.sock.send_to(&req.bytes, &target) {
            Ok(_) => Ok(()),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Ok(()),
            Err(e) => Err(e).with_context(|| format!("send_to {}", self.dst)),
        }
    }

    /// Drain every reply (and ICMP error) currently pending on the socket.
    pub fn recv(&mut self) -> Vec<Sample> {
        let mut out = Vec::new();
        loop {
            // socket2 ≥0.5.7 takes MaybeUninit slices; the buffer is ours, the
            // kernel only fills `n` bytes, reading them back as u8 is sound.
            let raw = self.recv_buf.as_mut_ptr();
            let buf: &mut [std::mem::MaybeUninit<u8>] =
                unsafe { std::slice::from_raw_parts_mut(raw.cast(), self.recv_buf.len()) };
            let (n, src) = match self.sock.recv_from(buf) {
                Ok(v) => v,
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => break,
                Err(_) => break,
            };
            let Some(pkt) = parse(&self.recv_buf[..n]) else { continue };
            let v6 = self.dst.ip().is_ipv6();
            let expect_reply = if v6 { ICMPV6_ECHO_REPLY } else { ICMPV4_ECHO_REPLY };
            if pkt.msg_type == expect_reply && pkt.id == self.id {
                if self.sent.remove(&pkt.seq).is_some() && pkt.body.len() >= HEADER_TS_LEN {
                    let micros = u64::from_be_bytes([
                        pkt.body[0],
                        pkt.body[1],
                        pkt.body[2],
                        pkt.body[3],
                        pkt.body[4],
                        pkt.body[5],
                        pkt.body[6],
                        pkt.body[7],
                    ]);
                    let arrived = self.session_start + Duration::from_micros(micros);
                    let ms = (arrived.elapsed().as_secs_f64().max(0.0) * 1000.0).min(1_000_000.0);
                    out.push(Sample {
                        seq: u64::from(pkt.seq),
                        ms,
                        ttl: pkt.ttl,
                        lost: false,
                        src: src.as_socket().map(|a| a.ip()),
                        at: Instant::now(),
                    });
                }
            } else if !v6 && pkt.msg_type == ICMPV4_DEST_UNREACH && pkt.body.len() >= 28 {
                // ICMP error embedding our request: original header sits at
                // body[20..28] (id at 24..26, seq at 26..28).
                let inner_id = u16::from_be_bytes([pkt.body[24], pkt.body[25]]);
                let inner_seq = u16::from_be_bytes([pkt.body[26], pkt.body[27]]);
                if inner_id == self.id {
                    if let Some(sent_at) = self.sent.remove(&inner_seq) {
                        let _ = sent_at;
                        out.push(Sample {
                            seq: u64::from(inner_seq),
                            ms: 0.0,
                            ttl: None,
                            lost: true,
                            src: src.as_socket().map(|a| a.ip()),
                            at: Instant::now(),
                        });
                    }
                }
            }
            if out.len() >= 256 {
                break;
            }
        }
        out
    }

    /// Requests older than `timeout` with no answer → lost.
    pub fn expire(&mut self, timeout: Duration) -> Vec<u64> {
        let now = Instant::now();
        let stale: Vec<u16> = self
            .sent
            .iter()
            .filter(|(_, t)| now.duration_since(**t) > timeout)
            .map(|(s, _)| *s)
            .collect();
        for s in &stale {
            self.sent.remove(s);
        }
        stale.into_iter().map(u64::from).collect()
    }
}

// ---------------------------------------------------------------- sockets --

fn open_socket(dst: SocketAddr) -> Result<(Socket, u16, &'static str)> {
    let ip = dst.ip();
    let pid_id = (std::process::id() & 0xffff) as u16;
    if ip.is_ipv6() {
        #[cfg(not(windows))]
        if let Some(s) = open_dgram(Domain::IPV6, Protocol::ICMPV6) {
            let id = socket_port(&s).unwrap_or(pid_id);
            return Ok((s, id, "SOCK_DGRAM ICMPv6 (sans privilège)"));
        }
        #[cfg(windows)]
        if let Some(s) = open_dgram(Domain::IPV6, Protocol::ICMPV6) {
            let id = socket_port(&s).unwrap_or(pid_id);
            return Ok((s, id, "SOCK_DGRAM ICMPv6"));
        }
        if let Some(s) = open_raw(Domain::IPV6, Protocol::ICMPV6, None) {
            return Ok((s, pid_id, "raw socket ICMPv6"));
        }
        return Err(anyhow!(
            "impossible d'ouvrir une socket ICMPv6 (root/CAP_NET_RAW requis ?)"
        ));
    }

    // IPv4
    #[cfg(windows)]
    {
        if let Some(s) = open_raw(Domain::IPV4, Protocol::ICMPV4, Some(dst)) {
            set_rcvall(&s);
            return Ok((s, pid_id, "raw socket ICMPv4"));
        }
        return Err(anyhow!(
            "impossible d'ouvrir la socket ICMPv4 raw — essayez « Exécuter en tant \
             qu'administrateur », ou vérifiez le pare-feu tiers (WinDivert)"
        ));
    }
    #[cfg(not(windows))]
    {
        // Unprivileged DGRAM first on Linux (kernel rewrites the id to the
        // socket port, which we read back), then raw for the classic path.
        if let Some(s) = open_dgram(Domain::IPV4, Protocol::ICMPV4) {
            let id = socket_port(&s).unwrap_or(pid_id);
            return Ok((s, id, "SOCK_DGRAM ICMPv4 (sans privilège)"));
        }
        if let Some(s) = open_raw(Domain::IPV4, Protocol::ICMPV4, None) {
            set_recv_ttl(&s);
            return Ok((s, pid_id, "raw socket ICMPv4"));
        }
        return Err(anyhow!(
            "impossible d'ouvrir une socket ICMP v4 : relancez en root/CAP_NET_RAW, \
             ou ouvrez net.ipv4.ping_group_range au groupe de l'utilisateur"
        ));
    }
}

/// DGRAM ICMP: must be bound for the kernel to allocate the echo id/port.
#[cfg(not(windows))]
fn open_dgram(domain: Domain, proto: Protocol) -> Option<Socket> {
    let s = Socket::new(domain, Type::DGRAM, Some(proto)).ok()?;
    let unspecified = match domain {
        d if d == Domain::IPV4 => SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 0)),
        _ => SocketAddr::new(IpAddr::V6(Ipv6Addr_unspec()), 0),
    };
    let _ = s.bind(&SockAddr::from(unspecified));
    s.set_nonblocking(true).ok()?;
    Some(s)
}

#[cfg(windows)]
fn open_dgram(domain: Domain, proto: Protocol) -> Option<Socket> {
    let s = Socket::new(domain, Type::DGRAM, Some(proto)).ok()?;
    let unspecified = match domain {
        d if d == Domain::IPV4 => SocketAddr::from(([0, 0, 0, 0], 0)),
        _ => SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 0),
    };
    let _ = s.bind(&SockAddr::from(unspecified));
    s.set_nonblocking(true).ok()?;
    Some(s)
}

#[cfg(not(windows))]
use std::net::Ipv6Addr;
#[cfg(not(windows))]
fn Ipv6Addr_unspec() -> Ipv6Addr {
    Ipv6Addr::UNSPECIFIED
}

#[cfg(windows)]
fn open_raw(domain: Domain, proto: Protocol, bind_hint: Option<SocketAddr>) -> Option<Socket> {
    let s = Socket::new(domain, Type::RAW, Some(proto)).ok()?;
    // Windows requires a concrete bind for meaningful raw receive: pick the
    // interface that routes to the destination (UDP connect trick).
    let bind_ip = bind_hint
        .and_then(|d| local_ipv4_for(d.ip()))
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    let _ = s.bind(&SockAddr::from(SocketAddr::new(bind_ip, 0)));
    s.set_nonblocking(true).ok()?;
    Some(s)
}

#[cfg(not(windows))]
fn open_raw(domain: Domain, proto: Protocol, _bind_hint: Option<SocketAddr>) -> Option<Socket> {
    let s = Socket::new(domain, Type::RAW, Some(proto)).ok()?;
    s.set_nonblocking(true).ok()?;
    Some(s)
}

fn socket_port(s: &Socket) -> Option<u16> {
    let a = s.local_addr().ok()?;
    a.as_socket_ipv4()
        .map(|v4| v4.port())
        .or_else(|| a.as_socket_ipv6().map(|v6| v6.port()))
}

#[cfg(windows)]
fn local_ipv4_for(ip: IpAddr) -> Option<IpAddr> {
    let udp = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    udp.connect(SocketAddr::new(ip, 53)).ok()?;
    match udp.local_addr().ok()? {
        SocketAddr::V4(v4) => Some(IpAddr::V4(*v4.ip())),
        _ => None,
    }
}

#[cfg(windows)]
fn set_rcvall(s: &Socket) {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{WSAIoctl, SIO_RCVALL};
    let on: u32 = 1;
    let mut returned: u32 = 0;
    unsafe {
        WSAIoctl(
            s.as_raw_socket() as usize,
            SIO_RCVALL,
            &on as *const u32 as *const core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned as *mut u32,
            std::ptr::null_mut(),
            None,
        );
    }
}

/// Best-effort TTL reporting for raw IPv4 sockets (Linux/BSD).
#[cfg(target_os = "linux")]
fn set_recv_ttl(s: &Socket) {
    use std::os::unix::io::AsRawFd;
    let on: libc::c_int = 1;
    unsafe {
        libc::setsockopt(
            s.as_raw_fd(),
            libc::IPPROTO_IP,
            libc::IP_RECVTTL,
            &on as *const libc::c_int as *const core::ffi::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn set_recv_ttl(_s: &Socket) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_self_verifies() {
        let start = Instant::now();
        let req = build_request(IpAddr::from([127, 0, 0, 1]), 0x1234, 7, 24, start, start);
        let stored = u16::from_be_bytes([req.bytes[2], req.bytes[3]]);
        assert_ne!(stored, 0);
        let mut zeroed = req.bytes.clone();
        zeroed[2..4].copy_from_slice(&0u16.to_be_bytes());
        assert_eq!(checksum(&zeroed), stored);
        // A packet carrying its own checksum verifies to 0.
        assert_eq!(checksum(&req.bytes), 0);
    }

    #[test]
    fn build_and_parse_roundtrip_v4_and_v6() {
        let start = Instant::now();
        for ip in [
            IpAddr::from([8, 8, 8, 8]),
            IpAddr::V6("2001:4860:4842::8888".parse().unwrap()),
        ] {
            let req = build_request(ip, 42, 9, 32, start, start);
            let p = parse(&req.bytes).unwrap();
            assert_eq!(p.id, 42);
            assert_eq!(p.seq, 9);
            assert!(p.body.len() >= 32);
            assert_eq!(p.ttl, None);
        }
    }

    #[test]
    fn reply_flips_type_and_keeps_identity() {
        let start = Instant::now();
        let req = build_request(IpAddr::from([1, 0, 0, 1]), 7, 3, 16, start, start);
        let mut reply = req.bytes.clone();
        reply[0] = ICMPV4_ECHO_REPLY;
        reply[2..4].copy_from_slice(&0u16.to_be_bytes());
        let ck = checksum(&reply);
        reply[2..4].copy_from_slice(&ck.to_be_bytes());
        let p = parse(&reply).unwrap();
        assert_eq!(p.msg_type, ICMPV4_ECHO_REPLY);
        assert_eq!(p.id, 7);
        assert_eq!(p.seq, 3);
    }

    #[test]
    fn parse_skips_leading_ipv4_header_and_reads_ttl() {
        let start = Instant::now();
        let req = build_request(IpAddr::from([1, 1, 1, 1]), 5, 6, 8, start, start);
        let mut pkt = vec![0x45, 0x00];
        pkt.extend_from_slice(&((20 + req.bytes.len()) as u16).to_be_bytes());
        pkt.extend_from_slice(&[0u8; 4]); // id/flags+frag
        pkt.push(64); // TTL
        pkt.push(1); // proto ICMP
        pkt.extend_from_slice(&[0u8; 2]); // hdr cksum
        pkt.extend_from_slice(&[127, 0, 0, 1, 127, 0, 0, 1]);
        pkt.extend_from_slice(&req.bytes);
        let p = parse(&pkt).unwrap();
        assert_eq!(p.seq, 6);
        assert_eq!(p.id, 5);
        assert_eq!(p.ttl, Some(64));
    }

    #[test]
    fn v6_request_checksum_field_left_zero() {
        let start = Instant::now();
        let req = build_request(
            IpAddr::V6("::1".parse().unwrap()),
            1,
            2,
            8,
            start,
            start,
        );
        assert_eq!(req.bytes[0], ICMPV6_ECHO_REQUEST);
        assert_eq!(&req.bytes[2..4], &[0, 0]);
    }

    #[test]
    fn unreachable_error_extracts_inner_identity() {
        // Error body must embed at least the original ICMP header (8) +
        // original IP header (20) = 28 bytes after the 8-byte error header.
        let start = Instant::now();
        let req = build_request(IpAddr::from([9, 9, 9, 9]), 11, 22, 8, start, start);
        let mut err = vec![ICMPV4_DEST_UNREACH, 1, 0, 0, 11, 0, 0, 0];
        err.extend_from_slice(&[0u8; 20]); // fake original IP header
        err.extend_from_slice(&req.bytes[..8]);
        let p = parse(&err).unwrap();
        assert_eq!(p.body.len(), 28);
        assert_eq!(u16::from_be_bytes([p.body[24], p.body[25]]), 11);
        assert_eq!(u16::from_be_bytes([p.body[26], p.body[27]]), 22);
    }

    #[test]
    fn tiny_or_garbage_returns_none() {
        assert!(parse(&[]).is_none());
        assert!(parse(&[8, 0]).is_none());
        assert!(parse(&[0x45, 0, 0, 4, 0, 0]).is_none()); // truncated datagram
    }

    #[test]
    fn odd_length_and_classic_vectors() {
        assert_eq!(checksum(&[0xff]), 0x00ff);
        assert_eq!(checksum(&[0x00, 0x01, 0x00, 0x02]), 0xfffc); // ~(1+2)
        assert_eq!(checksum(&[0x12, 0x34, 0x00, 0x00]), 0xedcb); // ~0x1234
    }
}
