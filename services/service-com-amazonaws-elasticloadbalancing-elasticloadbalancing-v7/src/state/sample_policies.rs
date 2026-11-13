// SPDX-License-Identifier: CC0-1.0

//! The sample SSL negotiation policies this service predefines — the catalog
//! `DescribeLoadBalancerPolicies` describes when a request names no load balancer: "the
//! action returns descriptions of the specified sample policies, or descriptions of all
//! sample policies. The names of the sample policies have the `ELBSample-` prefix."
//!
//! A sample policy belongs to no load balancer and nothing a caller does changes it, so the
//! catalog lives in constants rather than in the store.
//!
//! Each sample policy is described by the WHOLE attribute set of the SSL negotiation policy
//! type — every protocol attribute, the server cipher-order attribute and one attribute per
//! cipher suite the type publishes — each valued `"true"` or `"false"` according to whether
//! that policy's published configuration enables it. The attribute set is therefore
//! declared ONCE ([`ssl_negotiation_attribute_names`]) and every policy is stated as the
//! subset it enables, so no policy can be described with a short attribute list.
//!
//! Source: the SSL negotiation configurations the Classic Load Balancer documentation
//! publishes — its table of predefined SSL security policies (the protocol rows, the server
//! cipher-order row and the cipher rows, in the order that table lists them), and the two
//! sample cipher configurations published beside it, the load balancer's own default and
//! OpenSSL's. Each predefined policy enables the suites of the one before it minus the
//! suites its own publication removed, which is how the chain below is written.

use super::{PolicyAttributeEntry, SSL_NEGOTIATION_POLICY_TYPE};

/// The protocol attributes of the SSL negotiation policy type, in the order a description
/// reports them: one per SSL/TLS protocol version a negotiation configuration can turn on.
///
/// SSLv2 is not among them — no Classic load balancer negotiates it, so the policy type
/// carries no attribute for it and no description reports one.
pub const SSL_PROTOCOL_ATTRIBUTES: &[&str] = &[
    "Protocol-TLSv1",
    "Protocol-SSLv3",
    "Protocol-TLSv1.1",
    "Protocol-TLSv1.2",
];

/// The attribute that says the SERVER's cipher order wins over the client's.
pub const SERVER_DEFINED_CIPHER_ORDER_ATTRIBUTE: &str = "Server-Defined-Cipher-Order";

/// The cipher attributes of the SSL negotiation policy type, in the published order: one per
/// cipher suite a negotiation configuration can turn on.
pub const SSL_CIPHER_ATTRIBUTES: &[&str] = &[
    "ECDHE-ECDSA-AES128-GCM-SHA256",
    "ECDHE-RSA-AES128-GCM-SHA256",
    "ECDHE-ECDSA-AES128-SHA256",
    "ECDHE-RSA-AES128-SHA256",
    "ECDHE-ECDSA-AES128-SHA",
    "ECDHE-RSA-AES128-SHA",
    "ECDHE-ECDSA-AES256-GCM-SHA384",
    "ECDHE-RSA-AES256-GCM-SHA384",
    "ECDHE-ECDSA-AES256-SHA384",
    "ECDHE-RSA-AES256-SHA384",
    "ECDHE-RSA-AES256-SHA",
    "ECDHE-ECDSA-AES256-SHA",
    "AES128-GCM-SHA256",
    "AES128-SHA256",
    "AES128-SHA",
    "AES256-GCM-SHA384",
    "AES256-SHA256",
    "AES256-SHA",
    "DHE-RSA-AES128-SHA",
    "DHE-DSS-AES128-SHA",
    "CAMELLIA128-SHA",
    "EDH-RSA-DES-CBC3-SHA",
    "DES-CBC3-SHA",
    "ECDHE-RSA-RC4-SHA",
    "RC4-SHA",
    "ECDHE-ECDSA-RC4-SHA",
    "RC4-MD5",
];

/// The value an attribute a policy's configuration turns ON is reported with.
pub const ATTRIBUTE_VALUE_ENABLED: &str = "true";

/// The value an attribute a policy's configuration leaves OFF is reported with: an
/// attribute of the policy type the configuration does not enable is reported as disabled,
/// not left out.
pub const ATTRIBUTE_VALUE_DISABLED: &str = "false";

/// Every attribute of the SSL negotiation policy type, in the published order: the protocol
/// attributes, then the server cipher-order attribute, then the cipher attributes.
///
/// This is the attribute set EVERY sample policy is described by — the one list a
/// description is projected over, so the catalog cannot describe two policies by two
/// different attribute sets.
pub fn ssl_negotiation_attribute_names() -> impl Iterator<Item = &'static str> {
    SSL_PROTOCOL_ATTRIBUTES
        .iter()
        .copied()
        .chain(::std::iter::once(SERVER_DEFINED_CIPHER_ORDER_ATTRIBUTE))
        .chain(SSL_CIPHER_ATTRIBUTES.iter().copied())
}

/// One sample policy: a policy the SERVICE predefines rather than one a caller created.
///
/// The configuration is held as the protocols and cipher suites it ENABLES; the attribute
/// list a describe reports follows from it ([`SamplePolicy::attribute_entries`]) over the
/// whole attribute set of the policy type.
#[derive(Debug)]
pub struct SamplePolicy {
    pub name: &'static str,
    pub policy_type_name: &'static str,
    /// The protocol attributes this configuration negotiates; every other protocol
    /// attribute of the policy type is reported disabled.
    protocols: &'static [&'static str],
    /// Whether this configuration lets the server's cipher order win.
    server_defined_cipher_order: bool,
    /// The cipher suites this configuration enables, named in groups so each policy is
    /// stated as the one before it plus or minus the group its publication changed.
    cipher_groups: &'static [&'static [&'static str]],
}

impl SamplePolicy {
    /// Whether this configuration enables the policy-type attribute named `attribute`.
    pub fn enables(&self, attribute: &str) -> bool {
        if attribute == SERVER_DEFINED_CIPHER_ORDER_ATTRIBUTE {
            return self.server_defined_cipher_order;
        }
        self.protocols.contains(&attribute)
            || self
                .cipher_groups
                .iter()
                .any(|group| group.contains(&attribute))
    }

    /// This policy's `PolicyAttributeDescriptions`: EVERY attribute of the SSL negotiation
    /// policy type, each carrying the value this configuration gives it.
    pub fn attribute_entries(&self) -> Vec<PolicyAttributeEntry> {
        ssl_negotiation_attribute_names()
            .map(|name| PolicyAttributeEntry {
                name: name.to_string(),
                value: Some(
                    if self.enables(name) {
                        ATTRIBUTE_VALUE_ENABLED
                    } else {
                        ATTRIBUTE_VALUE_DISABLED
                    }
                    .to_string(),
                ),
            })
            .collect()
    }
}

/// The protocols a configuration that negotiates TLS only turns on.
const TLS_PROTOCOLS: &[&str] = &["Protocol-TLSv1", "Protocol-TLSv1.1", "Protocol-TLSv1.2"];

/// The protocols a configuration whose minimum is TLS 1.1 turns on.
const TLS_1_1_AND_LATER_PROTOCOLS: &[&str] = &["Protocol-TLSv1.1", "Protocol-TLSv1.2"];

/// The protocols a configuration whose minimum is TLS 1.2 turns on.
const TLS_1_2_PROTOCOLS: &[&str] = &["Protocol-TLSv1.2"];

/// The protocols a configuration published before SSLv3 was withdrawn turns on.
const SSLV3_AND_TLS_PROTOCOLS: &[&str] = &[
    "Protocol-SSLv3",
    "Protocol-TLSv1",
    "Protocol-TLSv1.1",
    "Protocol-TLSv1.2",
];

/// The protocols the oldest published configuration turns on: SSLv3 and TLS 1.0, the only
/// versions it predates.
const SSLV3_AND_TLSV1_PROTOCOLS: &[&str] = &["Protocol-SSLv3", "Protocol-TLSv1"];

/// The forward-secret and AES suites every configuration published from 2014 on enables.
const ECDHE_AND_AES_SUITES: &[&str] = &[
    "ECDHE-ECDSA-AES128-GCM-SHA256",
    "ECDHE-RSA-AES128-GCM-SHA256",
    "ECDHE-ECDSA-AES128-SHA256",
    "ECDHE-RSA-AES128-SHA256",
    "ECDHE-ECDSA-AES128-SHA",
    "ECDHE-RSA-AES128-SHA",
    "ECDHE-ECDSA-AES256-GCM-SHA384",
    "ECDHE-RSA-AES256-GCM-SHA384",
    "ECDHE-ECDSA-AES256-SHA384",
    "ECDHE-RSA-AES256-SHA384",
    "ECDHE-RSA-AES256-SHA",
    "ECDHE-ECDSA-AES256-SHA",
    "AES128-GCM-SHA256",
    "AES128-SHA256",
    "AES128-SHA",
    "AES256-GCM-SHA384",
    "AES256-SHA256",
    "AES256-SHA",
];

/// The AES suites of the oldest published configuration, which predates the forward-secret
/// and GCM suites above.
const LEGACY_AES_SUITES: &[&str] = &["AES128-SHA", "AES256-SHA"];

/// The ephemeral Diffie-Hellman suites, dropped by the configuration published after the
/// weak-DH disclosure.
const DHE_SUITES: &[&str] = &["DHE-RSA-AES128-SHA", "DHE-DSS-AES128-SHA"];

/// The Camellia suite the older configurations enable.
const CAMELLIA_SUITES: &[&str] = &["CAMELLIA128-SHA"];

/// The triple-DES suite re-enabled for older clients and dropped again by the configuration
/// published after the birthday-bound disclosure.
const DES_CBC3_SUITES: &[&str] = &["DES-CBC3-SHA"];

/// The ephemeral-DH triple-DES suite only the older configurations enable.
const EDH_DES_CBC3_SUITES: &[&str] = &["EDH-RSA-DES-CBC3-SHA"];

/// The forward-secret RC4 suites, dropped by the configuration published when RC4 was
/// withdrawn.
const ECDHE_RC4_SUITES: &[&str] = &["ECDHE-RSA-RC4-SHA", "ECDHE-ECDSA-RC4-SHA"];

/// The plain RC4 suite, dropped by the same publication.
const RC4_SHA_SUITES: &[&str] = &["RC4-SHA"];

/// The RC4-MD5 suite, which only the oldest configuration and OpenSSL's default enable.
const RC4_MD5_SUITES: &[&str] = &["RC4-MD5"];

/// The cipher suites the configurations published in 2014 enable: the forward-secret and
/// AES suites plus the legacy suites they had not yet dropped.
const SUITES_2014: &[&[&str]] = &[
    ECDHE_AND_AES_SUITES,
    DHE_SUITES,
    CAMELLIA_SUITES,
    EDH_DES_CBC3_SUITES,
    DES_CBC3_SUITES,
    ECDHE_RC4_SUITES,
    RC4_SHA_SUITES,
];

/// The sample policies this service predefines — what `DescribeLoadBalancerPolicies`
/// describes when the request names no load balancer, newest configuration first and the
/// two default-cipher samples last, as the published table orders them.
///
/// Every name carries the documented [`super::SAMPLE_POLICY_NAME_PREFIX`], and every entry
/// is of the SSL negotiation policy type, which is what a sample policy configures.
pub const SAMPLE_POLICIES: &[SamplePolicy] = &[
    // The current configuration: the forward-secret and AES suites only — triple-DES
    // dropped relative to the one below it.
    SamplePolicy {
        name: "ELBSample-ELBSecurityPolicy-2016-08",
        policy_type_name: SSL_NEGOTIATION_POLICY_TYPE,
        protocols: TLS_PROTOCOLS,
        server_defined_cipher_order: true,
        cipher_groups: &[ECDHE_AND_AES_SUITES],
    },
    // The same suites with TLS 1.2 as the minimum protocol.
    SamplePolicy {
        name: "ELBSample-ELBSecurityPolicy-TLS-1-2-2017-01",
        policy_type_name: SSL_NEGOTIATION_POLICY_TYPE,
        protocols: TLS_1_2_PROTOCOLS,
        server_defined_cipher_order: true,
        cipher_groups: &[ECDHE_AND_AES_SUITES],
    },
    // The same suites with TLS 1.1 as the minimum protocol.
    SamplePolicy {
        name: "ELBSample-ELBSecurityPolicy-TLS-1-1-2017-01",
        policy_type_name: SSL_NEGOTIATION_POLICY_TYPE,
        protocols: TLS_1_1_AND_LATER_PROTOCOLS,
        server_defined_cipher_order: true,
        cipher_groups: &[ECDHE_AND_AES_SUITES],
    },
    // Triple-DES still enabled, the ephemeral-DH suites dropped.
    SamplePolicy {
        name: "ELBSample-ELBSecurityPolicy-2015-05",
        policy_type_name: SSL_NEGOTIATION_POLICY_TYPE,
        protocols: TLS_PROTOCOLS,
        server_defined_cipher_order: true,
        cipher_groups: &[ECDHE_AND_AES_SUITES, DES_CBC3_SUITES],
    },
    // The ephemeral-DH suites still enabled, triple-DES re-enabled for older clients.
    SamplePolicy {
        name: "ELBSample-ELBSecurityPolicy-2015-03",
        policy_type_name: SSL_NEGOTIATION_POLICY_TYPE,
        protocols: TLS_PROTOCOLS,
        server_defined_cipher_order: true,
        cipher_groups: &[ECDHE_AND_AES_SUITES, DHE_SUITES, DES_CBC3_SUITES],
    },
    // The configuration that dropped RC4, Camellia and triple-DES.
    SamplePolicy {
        name: "ELBSample-ELBSecurityPolicy-2015-02",
        policy_type_name: SSL_NEGOTIATION_POLICY_TYPE,
        protocols: TLS_PROTOCOLS,
        server_defined_cipher_order: true,
        cipher_groups: &[ECDHE_AND_AES_SUITES, DHE_SUITES],
    },
    // The 2014 suites with SSLv3 withdrawn.
    SamplePolicy {
        name: "ELBSample-ELBSecurityPolicy-2014-10",
        policy_type_name: SSL_NEGOTIATION_POLICY_TYPE,
        protocols: TLS_PROTOCOLS,
        server_defined_cipher_order: true,
        cipher_groups: SUITES_2014,
    },
    // The first configuration to negotiate TLS 1.1 and 1.2, SSLv3 still on.
    SamplePolicy {
        name: "ELBSample-ELBSecurityPolicy-2014-01",
        policy_type_name: SSL_NEGOTIATION_POLICY_TYPE,
        protocols: SSLV3_AND_TLS_PROTOCOLS,
        server_defined_cipher_order: true,
        cipher_groups: SUITES_2014,
    },
    // The oldest published configuration: no forward-secret suites, no server cipher order.
    SamplePolicy {
        name: "ELBSample-ELBSecurityPolicy-2011-08",
        policy_type_name: SSL_NEGOTIATION_POLICY_TYPE,
        protocols: SSLV3_AND_TLSV1_PROTOCOLS,
        server_defined_cipher_order: false,
        cipher_groups: &[
            LEGACY_AES_SUITES,
            DHE_SUITES,
            CAMELLIA_SUITES,
            EDH_DES_CBC3_SUITES,
            DES_CBC3_SUITES,
            RC4_SHA_SUITES,
            RC4_MD5_SUITES,
        ],
    },
    // The load balancer's own default cipher configuration: the client's cipher order, TLS
    // only, and the suites the default negotiates.
    SamplePolicy {
        name: "ELBSample-ELBDefaultCipherPolicy",
        policy_type_name: SSL_NEGOTIATION_POLICY_TYPE,
        protocols: TLS_PROTOCOLS,
        server_defined_cipher_order: false,
        cipher_groups: SUITES_2014,
    },
    // OpenSSL's default cipher configuration: every suite the policy type publishes, and
    // SSLv3 as OpenSSL's default negotiates it.
    SamplePolicy {
        name: "ELBSample-OpenSSLDefaultCipherPolicy",
        policy_type_name: SSL_NEGOTIATION_POLICY_TYPE,
        protocols: SSLV3_AND_TLS_PROTOCOLS,
        server_defined_cipher_order: false,
        cipher_groups: &[SSL_CIPHER_ATTRIBUTES],
    },
];

/// The sample policy named `policy_name`, or `None` when no sample policy carries that
/// name.
pub fn sample_policy(policy_name: &str) -> Option<&'static SamplePolicy> {
    SAMPLE_POLICIES
        .iter()
        .find(|sample| sample.name == policy_name)
}
