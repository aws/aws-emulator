// SPDX-License-Identifier: CC0-1.0

//! What a connector's `Url` names, and WHICH refusal a value that names no endpoint
//! earns.
//!
//! A connector is the record of "the parameters for a connection for the AS2 or SFTP
//! protocol", and its `Url` is "the URL of the partner's AS2 or SFTP endpoint" — the
//! address the service itself will connect OUT to when a transfer runs on that
//! connector. So the member is not free text that is merely stored and echoed: a value
//! that names no endpoint the service could ever reach is refused when the connector is
//! written, rather than accepted into a connector no transfer can ever complete.
//!
//! `Url` is modelled `len 0..255` with no `@pattern`, so the whole rule is service-side
//! ([`endpoint_url_defect`]): the value has to be an ABSOLUTE URL — a scheme and an
//! authority — whose host is an address that can be resolved, i.e. an IP literal or a
//! DNS name under a delegated top-level domain. Everything the DNS can never answer for
//! ([`UNRESOLVABLE_TOP_LEVEL_DOMAINS`]) is the one class of unreachable host that is
//! knowable without resolving anything, which is why it is the whole catalogue: any
//! other name may have an address, so a connector to it is written and the connection
//! is attempted when a transfer asks for it.
//!
//! The service states TWO DISTINCT sentences here, one per condition, and a value earns
//! the one its own condition names:
//!
//! * a value that carries NO SCHEME is no URL at all — it has no authority to separate
//!   from the rest, so the whole of it is PATH information, and no protocol a connector
//!   speaks addresses its endpoint by a path with no host behind it. That is
//!   [`URL_CARRIES_PATH_INFORMATION`];
//! * a value that IS a URL but whose host could never be resolved does name an endpoint,
//!   just not one that can exist. That is [`URL_NOT_VALID`].
//!
//! The two are kept apart because the sentence is the only thing that tells a caller
//! WHICH way the value is wrong — whether to add the scheme or to fix the host — so
//! collapsing both onto one text sends it down the wrong path.
//!
//! Both `CreateConnector` and `UpdateConnector` judge the connector they would leave
//! behind through [`ConnectorConfig::cross_member_violation`], which is where this check
//! is applied — after the configuration requirements the model states across the
//! members, and before anything is written — so neither operation can store a connector
//! the other would refuse.

/// The fixed `Message` the service answers a `Url` that names no endpoint it could
/// reach with.
///
/// The service's own sentence for the condition, FIXED in its entirety: it states that
/// the URL submitted is not a valid one and has no slot for the value, the member or
/// anything else the caller sent, so every unreachable endpoint is refused with the
/// same text and a client can recognise the refusal.
pub const URL_NOT_VALID: &str = "The specified Url is not valid";

/// The fixed `Message` the service answers a `Url` that carries nothing but path
/// information with.
///
/// The service's own sentence for the OTHER way the member can be wrong, and likewise
/// fixed in its entirety: it reports that the URL is invalid and then WHY — the protocol
/// the connector configures takes no path information in its endpoint address — with no
/// slot for the value or the protocol's name.
pub const URL_CARRIES_PATH_INFORMATION: &str =
    "Invalid URL; Configured protocol does not support path information in the url";

/// The top-level domains no name can be resolved under, as the top-level domains that
/// are RESERVED never to be delegated: `.invalid` and `.test`, set aside for names that
/// are meant not to resolve and for testing, and `.localhost` and `.local`, which name
/// the local host and the local link rather than anything the Internet can reach
/// (RFC 2606 §2, RFC 6761 §6, RFC 6762 §3).
///
/// The documentation names `.example`, `example.com`, `example.net` and `example.org`
/// are NOT here: those are delegated and do resolve, so a connector to one is written
/// like a connector to any other partner host.
const UNRESOLVABLE_TOP_LEVEL_DOMAINS: [&str; 4] = ["invalid", "test", "localhost", "local"];

/// The longest a single DNS label can be (RFC 1035 §2.3.4).
const MAX_DNS_LABEL_LENGTH: usize = 63;

/// The refusal text a connector `Url` that names no reachable endpoint carries, or
/// `None` when the value names one.
///
/// A value is an endpoint when it parses as an absolute URL — anything relative carries
/// no scheme, so it has no authority to name a host with and is read to its end as path
/// information ([`URL_CARRIES_PATH_INFORMATION`]) — and its host is an address that can
/// be resolved: an IP literal, or a DNS name whose labels are well-formed and whose
/// top-level domain is one the DNS delegates ([`UNRESOLVABLE_TOP_LEVEL_DOMAINS`]). The
/// parse is the `url` crate's, so the forms accepted are the ones the URL standard
/// defines rather than a hand-read prefix, and a value it reports as relative is exactly
/// one that carries no scheme.
pub fn endpoint_url_defect(url: &str) -> Option<&'static str> {
    let Ok(parsed) = ::url::Url::parse(url) else {
        return Some(URL_CARRIES_PATH_INFORMATION);
    };
    match parsed.host() {
        // An opaque host — what a URL of a scheme the standard calls "not special"
        // (`sftp:`) carries — reaches this arm whether it spells a name or an address,
        // so the address case is answered here rather than by the arm below.
        Some(::url::Host::Domain(host)) => (!resolvable_host(host)).then_some(URL_NOT_VALID),
        // An IP literal is its own address: there is nothing to resolve.
        Some(::url::Host::Ipv4(_) | ::url::Host::Ipv6(_)) => None,
        None => Some(URL_NOT_VALID),
    }
}

/// Can `host` be resolved to an address?
///
/// True for an IP literal, and for a DNS name of at least two well-formed labels whose
/// top-level domain is alphabetic and delegated. The trailing root dot is dropped and
/// the comparison is case-insensitive, because neither distinguishes two DNS names.
fn resolvable_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    // A single label is a name under the DNS ROOT, which delegates to top-level domains
    // and holds no host of its own, so it resolves nowhere on the Internet.
    if labels.len() < 2 || !labels.iter().all(|label| is_dns_label(label)) {
        return false;
    }
    let top_level = labels.last().copied().unwrap_or_default();
    top_level.chars().all(|c| c.is_ascii_alphabetic())
        && !UNRESOLVABLE_TOP_LEVEL_DOMAINS.contains(&top_level)
}

/// Is `label` one well-formed DNS label — non-empty, within the length limit, letters,
/// digits and interior hyphens only (RFC 1035 §2.3.1, RFC 1123 §2.1)?
fn is_dns_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= MAX_DNS_LABEL_LENGTH
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Url` names an endpoint when it is an absolute URL whose host can be resolved,
    /// and names none otherwise — with the sentence for the way it fails to: no scheme
    /// at all, so nothing but path information, or a host that can never be resolved.
    ///
    /// The rows span what the rule generalises over: the scheme (the two AS2 schemes and
    /// the SFTP one), the form of the host (a delegated DNS name, a deep one, a
    /// punycoded one, an IPv4 and an IPv6 literal), a port and a path or their absence,
    /// each way a value carries no scheme, each reserved top-level domain the DNS never
    /// delegates, and both ends of the DNS label-length limit.
    #[test]
    fn a_connector_url_names_an_endpoint_only_when_its_host_can_be_resolved() {
        let longest_label = "a".repeat(MAX_DNS_LABEL_LENGTH);
        let over_long_label = "a".repeat(MAX_DNS_LABEL_LENGTH + 1);
        let cases: Vec<(&str, String, Option<&str>)> = vec![
            // Reachable endpoints: a delegated name, every scheme a connector speaks,
            // and the addresses that need no resolution at all.
            ("an https AS2 endpoint", "https://partner.example.com/as2".to_string(), None),
            ("an http AS2 endpoint on a port", "http://partner.example.com:8080/as2".to_string(), None),
            ("an sftp endpoint", "sftp://partner.example.com".to_string(), None),
            ("an sftp endpoint on a port", "sftp://partner.example.com:2222".to_string(), None),
            ("a deep host name", "sftp://s-0123456789abcdef0.server.transfer.us-east-1.amazonaws.com".to_string(), None),
            ("a host name in another top-level domain", "sftp://files.partner.co.uk".to_string(), None),
            ("a host name whose case differs", "SFTP://Partner.Example.COM".to_string(), None),
            ("a host name with the root dot", "sftp://partner.example.com./inbox".to_string(), None),
            ("a punycoded internationalised host", "https://xn--bcher-kva.example/as2".to_string(), None),
            ("an IPv4 literal", "sftp://192.0.2.10:22".to_string(), None),
            ("an IPv6 literal", "sftp://[2001:db8::1]:22".to_string(), None),
            ("a label at the length limit", format!("sftp://{longest_label}.example.com"), None),
            // No scheme: nothing separates an authority from the rest, so the value is
            // read to its end as path information.
            ("a bare host name with no scheme", "ftp.example.com".to_string(), Some(URL_CARRIES_PATH_INFORMATION)),
            ("a host and a path with no scheme", "partner.example.com/as2".to_string(), Some(URL_CARRIES_PATH_INFORMATION)),
            ("a scheme-relative reference", "//partner.example.com/as2".to_string(), Some(URL_CARRIES_PATH_INFORMATION)),
            ("a path only", "/as2".to_string(), Some(URL_CARRIES_PATH_INFORMATION)),
            ("prose", "the partner's endpoint".to_string(), Some(URL_CARRIES_PATH_INFORMATION)),
            // A scheme, but no host the service could ever connect to.
            ("a scheme and nothing else", "sftp:".to_string(), Some(URL_NOT_VALID)),
            ("a scheme with an empty authority", "sftp://".to_string(), Some(URL_NOT_VALID)),
            ("a scheme whose URL carries no host", "mailto:partner@example.com".to_string(), Some(URL_NOT_VALID)),
            ("a file URL, which has no host", "file:///inbox".to_string(), Some(URL_NOT_VALID)),
            // No endpoint: a host the DNS can never answer for.
            ("a host under the reserved .invalid", "https://partner.invalid/as2".to_string(), Some(URL_NOT_VALID)),
            ("a deep host under .invalid", "https://as2.partner.example.invalid/as2".to_string(), Some(URL_NOT_VALID)),
            ("a host under the reserved .test", "sftp://partner.test".to_string(), Some(URL_NOT_VALID)),
            ("a host under the reserved .localhost", "sftp://partner.localhost".to_string(), Some(URL_NOT_VALID)),
            ("a host on the local link", "sftp://partner.local".to_string(), Some(URL_NOT_VALID)),
            ("the reserved TLD in upper case", "https://PARTNER.INVALID/as2".to_string(), Some(URL_NOT_VALID)),
            ("the reserved TLD with the root dot", "sftp://partner.invalid.".to_string(), Some(URL_NOT_VALID)),
            // No endpoint: a host no DNS name can take the shape of.
            ("a single-label host", "sftp://partner".to_string(), Some(URL_NOT_VALID)),
            ("a host with an empty label", "sftp://partner..example.com".to_string(), Some(URL_NOT_VALID)),
            ("a host whose label starts with a hyphen", "sftp://-partner.example.com".to_string(), Some(URL_NOT_VALID)),
            ("a host whose label ends with a hyphen", "sftp://partner-.example.com".to_string(), Some(URL_NOT_VALID)),
            ("a host whose top-level domain is numeric", "sftp://partner.123".to_string(), Some(URL_NOT_VALID)),
            ("a label one past the length limit", format!("sftp://{over_long_label}.example.com"), Some(URL_NOT_VALID)),
        ];

        for (case, url, expected) in &cases {
            let defect = endpoint_url_defect(url);
            assert_eq!(
                defect, *expected,
                "case {case:?}: expected {url:?} to be judged {expected:?}, got {defect:?}"
            );
        }
    }
}
