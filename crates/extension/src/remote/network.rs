use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use async_trait::async_trait;
use futures_util::TryStreamExt;
use reqwest::{Client, StatusCode};
use tough::{
    HttpTransport, HttpTransportBuilder, Transport, TransportError, TransportErrorKind,
    TransportStream,
};
use url::{Host, Url};

use super::validate_download_url;

const MAX_RESOLVED_ADDRESSES: usize = 16;
const PUBLIC_DNS_TIMEOUT: Duration = Duration::from_secs(5);

/// Network boundary enforced for every online Registry request.
///
/// `Standard` retains the local CLI behavior, including loopback HTTP test
/// registries. Multi-tenant or otherwise untrusted control planes should use
/// `PublicInternet`, which requires HTTPS, rejects non-public address space,
/// pins the checked DNS answers into the HTTP client, disables proxies, and
/// disables automatic redirects. Bounded signed-target redirects are resolved
/// and checked again before every hop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryNetworkPolicy {
    #[default]
    Standard,
    PublicInternet,
}

#[derive(Clone, Debug)]
pub(super) enum RegistryTransport {
    Standard(Box<HttpTransport>),
    PublicInternet(PublicInternetTransport),
}

impl RegistryTransport {
    pub(super) fn new(policy: RegistryNetworkPolicy) -> Self {
        match policy {
            RegistryNetworkPolicy::Standard => Self::Standard(Box::new(
                HttpTransportBuilder::new()
                    .timeout(Duration::from_secs(300))
                    .connect_timeout(Duration::from_secs(15))
                    .tries(3)
                    .build(),
            )),
            RegistryNetworkPolicy::PublicInternet => Self::PublicInternet(PublicInternetTransport),
        }
    }
}

#[async_trait]
impl Transport for RegistryTransport {
    async fn fetch(&self, url: Url) -> Result<TransportStream, TransportError> {
        match self {
            Self::Standard(transport) => transport.fetch(url).await,
            Self::PublicInternet(transport) => transport.fetch(url).await,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PublicInternetTransport;

#[async_trait]
impl Transport for PublicInternetTransport {
    async fn fetch(&self, url: Url) -> Result<TransportStream, TransportError> {
        let client =
            public_internet_client(&url, Duration::from_secs(15), Duration::from_secs(300))
                .await
                .map_err(|error| {
                    TransportError::new_with_cause(TransportErrorKind::Other, url.as_str(), error)
                })?;
        let response = client.get(url.clone()).send().await.map_err(|error| {
            TransportError::new_with_cause(TransportErrorKind::Other, url.as_str(), error)
        })?;
        if !response.status().is_success() {
            let kind = match response.status() {
                StatusCode::FORBIDDEN | StatusCode::NOT_FOUND | StatusCode::GONE => {
                    TransportErrorKind::FileNotFound
                }
                _ => TransportErrorKind::Other,
            };
            return Err(TransportError::new(kind, url.as_str()));
        }
        let stream_url = url.clone();
        Ok(Box::pin(response.bytes_stream().map_err(move |error| {
            TransportError::new_with_cause(TransportErrorKind::Other, stream_url.as_str(), error)
        })))
    }
}

pub(super) async fn public_internet_client(
    url: &Url,
    connect_timeout: Duration,
    timeout: Duration,
) -> UseResult<Client> {
    validate_download_url(url)?;
    if url.scheme() != "https" {
        return Err(network_denied(
            "The public Registry policy requires HTTPS for every request.",
        ));
    }
    let host = url
        .host()
        .ok_or_else(|| network_denied("The public Registry URL has no host."))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| network_denied("The public Registry URL has no valid port."))?;
    let mut builder = Client::builder()
        .user_agent("a3s-use-extension/0.3")
        .connect_timeout(connect_timeout)
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();

    match host {
        Host::Ipv4(address) => require_public_address(IpAddr::V4(address))?,
        Host::Ipv6(address) => require_public_address(IpAddr::V6(address))?,
        Host::Domain(domain) => {
            if domain.eq_ignore_ascii_case("localhost")
                || domain.ends_with(".localhost")
                || domain.ends_with(".local")
                || domain.ends_with(".internal")
                || domain.eq_ignore_ascii_case("home.arpa")
                || domain.ends_with(".home.arpa")
            {
                return Err(network_denied(
                    "The public Registry host is a local-use domain.",
                ));
            }
            let addresses = resolve_public_addresses(domain, port).await?;
            builder = builder.resolve_to_addrs(domain, &addresses);
        }
    }

    builder.build().map_err(|error| {
        UseError::new(
            "use.extension.registry_download_failed",
            format!("Failed to build the public Registry client: {error}"),
        )
    })
}

async fn resolve_public_addresses(domain: &str, port: u16) -> UseResult<Vec<SocketAddr>> {
    let resolved =
        tokio::time::timeout(PUBLIC_DNS_TIMEOUT, tokio::net::lookup_host((domain, port)))
            .await
            .map_err(|_| network_denied("Public Registry DNS resolution timed out."))?
            .map_err(|error| {
                network_denied(format!("Public Registry DNS resolution failed: {error}"))
            })?;
    let mut addresses = BTreeSet::new();
    for address in resolved {
        require_public_address(address.ip())?;
        addresses.insert(address);
        if addresses.len() > MAX_RESOLVED_ADDRESSES {
            return Err(network_denied(
                "The public Registry host resolved to too many addresses.",
            ));
        }
    }
    if addresses.is_empty() {
        return Err(network_denied(
            "The public Registry host resolved to no addresses.",
        ));
    }
    Ok(addresses.into_iter().collect())
}

fn require_public_address(address: IpAddr) -> UseResult<()> {
    let public = match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    };
    if public {
        Ok(())
    } else {
        Err(network_denied(
            "The public Registry host resolved to non-public address space.",
        ))
    }
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [first, second, third, _] = address.octets();
    !(first == 0
        || first == 10
        || first == 127
        || first >= 224
        || (first == 100 && (64..=127).contains(&second))
        || (first == 169 && second == 254)
        || (first == 172 && (16..=31).contains(&second))
        || (first == 192 && second == 168)
        || (first == 192 && second == 0 && third == 0)
        || (first == 192 && second == 0 && third == 2)
        || (first == 192 && second == 88 && third == 99)
        || (first == 198 && (18..=19).contains(&second))
        || (first == 198 && second == 51 && third == 100)
        || (first == 203 && second == 0 && third == 113))
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }
    let segments = address.segments();
    let global_unicast = segments[0] & 0xe000 == 0x2000;
    let ietf_special = segments[0] == 0x2001 && segments[1] <= 0x01ff;
    let documentation = (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || (segments[0] == 0x3fff && segments[1] < 0x1000);
    let six_to_four = segments[0] == 0x2002;
    global_unicast && !ietf_special && !documentation && !six_to_four
}

fn network_denied(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.registry_network_denied", message)
}

#[cfg(test)]
mod tests {
    use super::{is_public_ipv4, is_public_ipv6, public_internet_client};
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::time::Duration;
    use url::Url;

    #[test]
    fn public_policy_rejects_special_address_space() {
        for address in [
            Ipv4Addr::new(0, 0, 0, 0),
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(100, 64, 0, 1),
            Ipv4Addr::new(127, 0, 0, 1),
            Ipv4Addr::new(169, 254, 0, 1),
            Ipv4Addr::new(172, 16, 0, 1),
            Ipv4Addr::new(192, 168, 0, 1),
            Ipv4Addr::new(198, 51, 100, 1),
            Ipv4Addr::new(224, 0, 0, 1),
        ] {
            assert!(!is_public_ipv4(address), "accepted {address}");
        }
        assert!(is_public_ipv4(Ipv4Addr::new(8, 8, 8, 8)));

        for address in [
            Ipv6Addr::LOCALHOST,
            "fc00::1".parse().expect("unique local address"),
            "fe80::1".parse().expect("link local address"),
            "2001:db8::1".parse().expect("documentation address"),
            "2002:0a00:0001::1".parse().expect("6to4 address"),
        ] {
            assert!(!is_public_ipv6(address), "accepted {address}");
        }
        assert!(is_public_ipv6(
            "2606:4700:4700::1111".parse().expect("public IPv6 address")
        ));
    }

    #[tokio::test]
    async fn public_client_rejects_loopback_before_connecting() {
        let error = public_internet_client(
            &Url::parse("https://127.0.0.1/metadata/").expect("URL"),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .await
        .expect_err("loopback must be rejected");
        assert_eq!(error.code, "use.extension.registry_network_denied");
    }

    #[tokio::test]
    async fn public_client_rejects_local_domain_before_resolving() {
        let error = public_internet_client(
            &Url::parse("https://metadata.internal/metadata/").expect("URL"),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .await
        .expect_err("local-use domain must be rejected");
        assert_eq!(error.code, "use.extension.registry_network_denied");
    }
}
