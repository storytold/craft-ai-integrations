//! Which URLs a provider may point us at.
//!
//! A hosted provider hands back a link; the link is untrusted input. We follow it only when it is
//! https (plain http only for a loopback host the caller has allowed), carries no user-info, and
//! does not name a private, link-local or otherwise internal address. A name is held to the same
//! rule when it is resolved: downloads connect through [`GuardedResolver`], which hands the HTTP
//! client only allowed addresses.
//!
//! Names are compared without a trailing dot (`localhost.` is `localhost`), and an IPv6 address
//! that carries an IPv4 one (mapped, compatible, NAT64, 6to4) is judged by the IPv4 inside it.

use craft_ai_core::{AiError, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::NextTimeout;
use url::{Host, Url};

fn unsafe_link(why: &str) -> AiError {
    AiError::new(ErrorKind::Provider, format!("media link refused: {why}"))
}

/// What a link is answered with when its name reaches an address a VPN or a fake-IP proxy hands
/// out. A public name can legitimately resolve to one of these on such a setup, so the person is
/// told what to change rather than that the address is private.
const SHARED_ADDRESS: &str = "its name points at a shared address (100.64.0.0/10 for Tailscale, or 198.18.0.0/15 for a fake-IP proxy mode), so nothing was downloaded; turn that off, then try again";

/// Checks one URL. `allow_loopback` permits `http` and `https` to a loopback host.
pub fn check(url: &Url, allow_loopback: bool) -> Result<(), AiError> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err(unsafe_link("it carries credentials"));
    }
    let loopback = is_loopback(url);
    match url.scheme() {
        "https" => {}
        "http" if allow_loopback && loopback => {}
        "http" => return Err(unsafe_link("it is not https")),
        _ => return Err(unsafe_link("it is not an http link")),
    }
    if loopback {
        return if allow_loopback {
            Ok(())
        } else {
            Err(unsafe_link("it points at this computer"))
        };
    }
    match url.host() {
        Some(Host::Ipv4(ip)) if shared_v4(ip) => Err(unsafe_link(SHARED_ADDRESS)),
        Some(Host::Ipv4(ip)) if internal_v4(ip) => {
            Err(unsafe_link("it points at a private address"))
        }
        Some(Host::Ipv6(ip)) if internal_v6(ip) => {
            Err(unsafe_link("it points at a private address"))
        }
        Some(Host::Domain(name)) if private_name(name) => {
            Err(unsafe_link("it points at a private name"))
        }
        Some(_) => Ok(()),
        None => Err(unsafe_link("it has no host")),
    }
}

/// [`check`], and for a host name also looks it up and refuses a name that points at this
/// computer or a private network (`evil.example` with an A record of `127.0.0.1`).
///
/// This is a look before the connection that gives a plain refusal; the guarantee comes from
/// [`GuardedResolver`], which checks the addresses actually connected to, so a server that answers
/// differently the second time (DNS rebinding) is caught there. A lookup that fails is refused.
pub fn check_resolved(url: &Url, allow_loopback: bool) -> Result<(), AiError> {
    check(url, allow_loopback)?;
    let Some(Host::Domain(name)) = url.host() else {
        return Ok(());
    };
    // A loopback name was already judged (and allowed or refused) by `check`.
    if is_loopback(url) {
        return Ok(());
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs = (name, port)
        .to_socket_addrs()
        .map_err(|_| AiError::new(ErrorKind::Network, "could not look up the media host"))?;
    for ip in addrs.map(|a| a.ip()) {
        if shared_addr(ip) {
            return Err(unsafe_link(SHARED_ADDRESS));
        }
        if internal_addr(ip) {
            return Err(unsafe_link("its name points at a private address"));
        }
    }
    Ok(())
}

/// The name resolver every media and model download connects through. It hands the HTTP client
/// only addresses a link may reach, so the connection itself, on every redirect hop, cannot land
/// on this computer or a private network whatever a name answers at that moment (DNS rebinding
/// included). [`check_resolved`] stays as the early, plainly worded refusal.
///
/// The one host it leaves alone is the environment's own proxy: with a proxy set, the proxy looks
/// up the link's name, and only the proxy can hold that lookup to the same rule.
#[derive(Debug, Default)]
pub struct GuardedResolver {
    allow_loopback: bool,
    inner: DefaultResolver,
}

impl GuardedResolver {
    /// `allow_loopback` lets loopback addresses through, as [`check`] does for loopback links.
    pub fn new(allow_loopback: bool) -> Self {
        Self {
            allow_loopback,
            inner: DefaultResolver::default(),
        }
    }

    fn allowed(&self, ip: IpAddr) -> bool {
        !internal_addr(ip) || (self.allow_loopback && loopback_addr(ip))
    }
}

impl Resolver for GuardedResolver {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let addrs = self.inner.resolve(uri, config, timeout)?;
        let to_proxy = config.proxy().is_some_and(|proxy| {
            uri.host()
                .is_some_and(|host| bare(host).eq_ignore_ascii_case(bare(proxy.host())))
        });
        if to_proxy {
            return Ok(addrs);
        }
        let blocked = addrs.iter().map(|a| a.ip()).find(|ip| !self.allowed(*ip));
        match blocked {
            None => Ok(addrs),
            Some(ip) if shared_addr(ip) => Err(ureq::Error::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                SHARED_ADDRESS,
            ))),
            Some(_) => Err(ureq::Error::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the link's name points at a private address",
            ))),
        }
    }
}

fn loopback_addr(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback() || embedded_v4(v6).is_some_and(|v4| v4.is_loopback()),
    }
}

fn internal_addr(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => internal_v4(v4),
        IpAddr::V6(v6) => internal_v6(v6),
    }
}

/// An address a VPN or a fake-IP proxy puts in place of the real one: carrier-grade NAT
/// (100.64.0.0/10, which Tailscale uses) and the benchmarking range some fake-IP proxy modes
/// answer with (198.18.0.0/15). They stay refused, since a shared or make-believe address says
/// nothing about who answers there, but they get the message that says what to change.
fn shared_addr(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => shared_v4(v4),
        IpAddr::V6(v6) => embedded_v4(v6).is_some_and(shared_v4),
    }
}

fn shared_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    (a == 100 && (64..128).contains(&b)) || (a == 198 && (18..20).contains(&b))
}

/// Resolves a `Location` header against the current link. A redirect may not leave https; the
/// caller still runs [`check`] on the result before connecting.
pub fn redirect_target(current: &Url, location: &str) -> Result<Url, AiError> {
    let next = current
        .join(location)
        .map_err(|_| AiError::new(ErrorKind::Provider, "media link redirects nowhere"))?;
    if current.scheme() == "https" && next.scheme() != "https" {
        return Err(unsafe_link("a redirect leaves https"));
    }
    Ok(next)
}

/// A host name without its root dot: `localhost.` and `localhost` are the same name.
fn bare(name: &str) -> &str {
    name.trim_end_matches('.')
}

/// Names that only mean something on a private network.
fn private_name(name: &str) -> bool {
    let name = bare(name).to_ascii_lowercase();
    [
        "local",
        "internal",
        "localdomain",
        "lan",
        "intranet",
        "corp",
        "home",
        "home.arpa",
        "private",
    ]
    .iter()
    .any(|suffix| name == *suffix || name.ends_with(&format!(".{suffix}")))
}

/// The same loopback rule that gates `allow_loopback` — shared with the store and the other
/// downloaders that pass `allow_loopback` to [`check`], to keep the proxy off loopback requests.
pub fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(name)) => {
            let name = bare(name).to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => {
            ip.is_loopback() || embedded_v4(ip).is_some_and(|v4| v4.is_loopback())
        }
        None => false,
    }
}

/// The IPv4 address an IPv6 one carries, for the forms that route to it: IPv4-mapped
/// (`::ffff:a.b.c.d`), IPv4-compatible (`::a.b.c.d`), NAT64 (`64:ff9b::/96`) and 6to4
/// (`2002:aabb:ccdd::/48`).
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return Some(v4);
    }
    let s = ip.segments();
    let from = |hi: u16, lo: u16| {
        let [a, b] = hi.to_be_bytes();
        let [c, d] = lo.to_be_bytes();
        Ipv4Addr::new(a, b, c, d)
    };
    // `::` and `::1` are handled by the loopback and unspecified checks.
    let compatible = s[..6].iter().all(|x| *x == 0) && (s[6] != 0 || s[7] > 1);
    let nat64 = s[0] == 0x64 && s[1] == 0xff9b && s[2..6].iter().all(|x| *x == 0);
    if compatible || nat64 {
        Some(from(s[6], s[7]))
    } else if s[0] == 0x2002 {
        Some(from(s[1], s[2]))
    } else {
        None
    }
}

fn internal_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && ip.octets()[2] == 0)
        || (a == 198 && (18..20).contains(&b))
        || a == 0
        || a >= 240
}

fn internal_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = embedded_v4(ip) {
        return internal_v4(v4);
    }
    let [first, second, ..] = ip.segments();
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80
        || (first & 0xffc0) == 0xfec0
        // Documentation (2001:db8::/32) and Teredo (2001::/32) have no business as media hosts.
        || (first == 0x2001 && (second == 0xdb8 || second == 0))
}

#[cfg(test)]
mod tests {

    /// A tiny loopback server that answers every request with 200, and acts as a CONNECT proxy
    /// when asked to (it then answers the tunnelled request the same way).
    fn answering_server() -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    let mut read_head = |s: &mut std::net::TcpStream, head: &mut Vec<u8>| {
                        head.clear();
                        while !head.ends_with(b"\r\n\r\n") && s.read(&mut byte).unwrap_or(0) == 1 {
                            head.push(byte[0]);
                        }
                    };
                    read_head(&mut s, &mut head);
                    if head.starts_with(b"CONNECT") {
                        let _ = s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n");
                        read_head(&mut s, &mut head);
                    }
                    let _ = s.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    );
                });
            }
        });
        port
    }

    /// The agent the downloaders build, with the shared TLS and proxy settings.
    fn agent(allow_loopback: bool) -> ureq::Agent {
        ureq::Agent::with_parts(
            ureq::Agent::config_builder()
                .tls_config(craft_ai_http::tls_config())
                .proxy(craft_ai_http::loopback_proxy())
                .build(),
            ureq::unversioned::transport::DefaultConnector::new(),
            GuardedResolver::new(allow_loopback),
        )
    }

    #[test]
    fn the_resolver_refuses_an_internal_answer_at_connect_time() {
        let port = answering_server();
        let url = format!("http://localhost:{port}/");
        // `localhost` stands in for any name whose answer is internal: no address is connected to.
        let err = agent(false).get(&url).call().unwrap_err();
        assert!(err.to_string().contains("private address"), "{err}");
        assert_eq!(agent(true).get(&url).call().unwrap().status(), 200);
    }

    #[test]
    fn the_environments_proxy_is_still_reached() {
        let port = answering_server();
        let proxy = format!("http://localhost:{port}");
        // The link's name is looked up by the proxy, never here, so it may be anything. The proxy
        // is set per request, as the store sets the environment's proxy hop by hop.
        let resp = agent(false)
            .get("http://media.example.invalid/x")
            .config()
            .proxy(Some(ureq::Proxy::new(&proxy).unwrap()))
            .build()
            .call()
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    use super::*;

    fn ok(url: &str, loopback: bool) -> bool {
        check(&Url::parse(url).unwrap(), loopback).is_ok()
    }

    #[test]
    fn ordinary_https_links_pass() {
        assert!(ok("https://cdn.example.com/a.png?sig=abc", false));
        assert!(ok("https://8.8.8.8/a.png", false));
        assert!(!ok("https://203.0.113.9/a.png", false)); // documentation range
    }

    #[test]
    fn plain_http_and_other_schemes_are_refused() {
        assert!(!ok("http://cdn.example.com/a.png", false));
        assert!(!ok("http://cdn.example.com/a.png", true));
        assert!(!ok("ftp://cdn.example.com/a.png", false));
        assert!(!ok("file:///etc/passwd", true));
    }

    #[test]
    fn a_redirect_resolves_relative_links_and_never_leaves_https() {
        let here = Url::parse("https://cdn.example.com/a/b.png").unwrap();
        assert_eq!(
            redirect_target(&here, "/c.png").unwrap().as_str(),
            "https://cdn.example.com/c.png"
        );
        assert!(redirect_target(&here, "http://cdn.example.com/c.png").is_err());
        assert!(redirect_target(&here, "http://127.0.0.1/c.png").is_err());
        assert!(redirect_target(&here, "").is_ok()); // same link; the hop limit ends any loop
        let plain = Url::parse("http://127.0.0.1/a").unwrap();
        assert!(redirect_target(&plain, "https://cdn.example.com/c.png").is_ok());
    }

    #[test]
    fn credentials_in_the_link_are_refused() {
        assert!(!ok("https://user:pw@cdn.example.com/a.png", false));
        assert!(!ok("https://user@cdn.example.com/a.png", false));
    }

    #[test]
    fn a_shared_vpn_address_says_what_to_change() {
        for u in ["https://100.64.0.1/a", "https://198.18.0.1/a"] {
            let error = check(&Url::parse(u).unwrap(), false).unwrap_err();
            assert!(error.message.contains("shared address"), "{u}: {error:?}");
            assert!(error.message.contains("Tailscale"), "{u}: {error:?}");
            assert!(error.message.contains("turn that off"), "{u}: {error:?}");
        }
        // A genuinely private address keeps the plain wording.
        let error = check(&Url::parse("https://10.0.0.5/a").unwrap(), false).unwrap_err();
        assert!(error.message.contains("private address"), "{error:?}");
        for shared in [
            "100.64.0.1",
            "100.127.255.254",
            "198.18.0.1",
            "198.19.255.255",
            "::ffff:100.64.0.1",
        ] {
            assert!(shared_addr(shared.parse().unwrap()), "{shared}");
        }
        for ordinary in [
            "100.63.255.255",
            "100.128.0.1",
            "198.17.255.255",
            "198.20.0.1",
            "8.8.8.8",
        ] {
            assert!(!shared_addr(ordinary.parse().unwrap()), "{ordinary}");
        }
    }

    #[test]
    fn internal_addresses_are_refused() {
        for u in [
            "https://127.0.0.1/a",
            "https://localhost/a",
            "https://[::1]/a",
            "https://10.0.0.5/a",
            "https://192.168.1.2/a",
            "https://172.16.0.1/a",
            "https://169.254.169.254/latest/meta-data",
            "https://100.64.0.1/a",
            "https://0.0.0.0/a",
            "https://[fd00::1]/a",
            "https://[fe80::1]/a",
            "https://[::ffff:10.0.0.1]/a",
            "https://printer.local/a",
            "https://db.internal/a",
            // A root dot is the same name.
            "https://localhost./a",
            "https://LocalHost./a",
            "https://metadata.google.internal./a",
            "https://printer.local./a",
            "https://nas.lan/a",
            "https://foo.localhost./a",
            // IPv6 forms that carry an IPv4 address.
            "https://[::7f00:1]/a",
            "https://[::a00:1]/a",
            "https://[64:ff9b::a9fe:a9fe]/a",
            "https://[64:ff9b::7f00:1]/a",
            "https://[2002:a9fe:a9fe::1]/a",
            "https://[2002:7f00:1::1]/a",
            "https://[::ffff:7f00:1]/a",
            "https://[fec0::1]/a",
            "https://[ff02::1]/a",
            "https://[2001:db8::1]/a",
            "https://[2001:0:4136:e378:8000:63bf:3fff:fdd2]/a",
            // Other reserved IPv4 ranges.
            "https://192.0.0.1/a",
            "https://198.18.0.1/a",
            "https://224.0.0.1/a",
        ] {
            assert!(!ok(u, false), "{u}");
        }
    }

    #[test]
    fn an_address_a_name_resolves_to_is_judged_like_a_literal_one() {
        for bad in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.169.254",
            "::1",
            "fd00::1",
            "::ffff:192.168.0.1",
            "64:ff9b::a00:1",
        ] {
            assert!(internal_addr(bad.parse().unwrap()), "{bad}");
        }
        for good in ["8.8.8.8", "2606:4700:4700::1111"] {
            assert!(!internal_addr(good.parse().unwrap()), "{good}");
        }
        // Addresses and loopback names need no lookup.
        assert!(check_resolved(&Url::parse("https://8.8.8.8/a").unwrap(), false).is_ok());
        assert!(check_resolved(&Url::parse("http://localhost:8080/a").unwrap(), true).is_ok());
        assert!(check_resolved(&Url::parse("https://localhost/a").unwrap(), false).is_err());
    }

    #[test]
    fn public_addresses_inside_the_wrapped_forms_still_pass() {
        // The IPv4 inside decides, so a public one stays reachable.
        for u in [
            "https://[64:ff9b::808:808]/a",
            "https://[2002:808:808::1]/a",
            "https://[2606:4700:4700::1111]/a",
            "https://notlocalhost.example.com/a",
            "https://localhost.example.com/a",
            "https://corporate.example.com/a",
            "https://cdn.homepage.example/a",
        ] {
            assert!(ok(u, false), "{u}");
        }
    }

    #[test]
    fn loopback_is_only_allowed_when_the_caller_says_so() {
        assert!(ok("http://127.0.0.1:8080/a.png", true));
        assert!(ok("http://localhost:8080/a.png", true));
        assert!(ok("http://localhost.:8080/a.png", true));
        assert!(!ok("http://127.0.0.1:8080/a.png", false));
        // Allowing loopback does not allow the rest of the private ranges.
        assert!(!ok("http://10.0.0.5/a.png", true));
        assert!(!ok("https://169.254.169.254/a", true));
    }
}
