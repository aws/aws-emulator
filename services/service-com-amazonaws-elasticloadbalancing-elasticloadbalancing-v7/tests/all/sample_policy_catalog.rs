// SPDX-License-Identifier: CC0-1.0

//! `DescribeLoadBalancerPolicies` with NO load balancer: the whole published catalog of
//! sample SSL negotiation configurations, each described by the whole attribute set of its
//! policy type.
//!
//! Every request goes through the awsQuery protocol on the multi-service router (see
//! `super::wire`), so what is asserted is the emulator's HTTP behaviour.
//!
//! The rules under test, each over a TABLE of inputs rather than one recorded request:
//!
//! * a describe that names no load balancer reports EVERY sample policy the service
//!   predefines — the nine published predefined SSL security configurations and the two
//!   published default-cipher samples — each under the documented `ELBSample-` prefix and
//!   of the SSL negotiation policy type.
//! * each one is described by the WHOLE attribute set of that policy type: every protocol
//!   attribute, the server cipher-order attribute and one attribute per published cipher
//!   suite, in that order, each valued `"true"` or `"false"` according to whether that
//!   configuration enables it — no attribute of the type is left out of a description.
//! * `PolicyNames` selects from the catalog, whatever it names and however often, and a
//!   name the catalog does not carry is refused `PolicyNotFound`.
//! * each configuration reports the protocols and cipher suites ITS publication enables,
//!   which is what the per-row table below checks on policies and ciphers the issue that
//!   filed this did not cite.

use super::wire::{call, emulator, error_message, param, Wire};
use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::state::{
    ssl_negotiation_attribute_names, ATTRIBUTE_VALUE_DISABLED, ATTRIBUTE_VALUE_ENABLED,
    SAMPLE_POLICIES, SAMPLE_POLICY_NAME_PREFIX, SERVER_DEFINED_CIPHER_ORDER_ATTRIBUTE,
    SSL_CIPHER_ATTRIBUTES, SSL_NEGOTIATION_POLICY_TYPE, SSL_PROTOCOL_ATTRIBUTES,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for. A sample policy is no load balancer's and
/// no region's, so the region only has to be one the request can be signed for.
const REGION: &str = "us-east-1";

/// The sample SSL negotiation configurations the Classic Load Balancer documentation
/// publishes, each named with the prefix `DescribeLoadBalancerPolicies`' own documentation
/// gives them ("The names of the sample policies have the `ELBSample-` prefix"): the nine
/// predefined SSL security configurations, newest first, and the two published
/// default-cipher samples — the load balancer's own and OpenSSL's.
///
/// Source: the predefined-SSL-security-policy table of the Classic Load Balancer
/// documentation and the two sample cipher configurations published beside it.
const PUBLISHED_SAMPLE_POLICY_NAMES: &[&str] = &[
    "ELBSample-ELBDefaultCipherPolicy",
    "ELBSample-ELBSecurityPolicy-2011-08",
    "ELBSample-ELBSecurityPolicy-2014-01",
    "ELBSample-ELBSecurityPolicy-2014-10",
    "ELBSample-ELBSecurityPolicy-2015-02",
    "ELBSample-ELBSecurityPolicy-2015-03",
    "ELBSample-ELBSecurityPolicy-2015-05",
    "ELBSample-ELBSecurityPolicy-2016-08",
    "ELBSample-ELBSecurityPolicy-TLS-1-1-2017-01",
    "ELBSample-ELBSecurityPolicy-TLS-1-2-2017-01",
    "ELBSample-OpenSSLDefaultCipherPolicy",
];

/// The attribute set of the SSL negotiation policy type, in the order a description reports
/// it: the protocol attributes, the server cipher-order attribute, then one attribute per
/// published cipher suite.
///
/// Written out here, independently of the catalog's own constants, so a change that shortens
/// or reorders the attribute set cannot silently pass this suite. Source: the
/// predefined-SSL-security-policy table of the Classic Load Balancer documentation (its
/// protocol rows, its server cipher-order row and its cipher rows).
const PUBLISHED_ATTRIBUTE_NAMES: &[&str] = &[
    "Protocol-TLSv1",
    "Protocol-SSLv3",
    "Protocol-TLSv1.1",
    "Protocol-TLSv1.2",
    "Server-Defined-Cipher-Order",
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

/// One sample policy as a describe reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    policy_name: String,
    policy_type_name: String,
    /// The attribute names, in the order the description carried them.
    attribute_names: Vec<String>,
    /// Each attribute's value, by name.
    values: Vec<(String, Option<String>)>,
}

impl Reported {
    /// The value this description gave the attribute named `attribute`, failing the test
    /// when the description left the attribute out.
    fn value(&self, attribute: &str) -> &str {
        self.values
            .iter()
            .find(|(name, _)| name == attribute)
            .map(|(_, value)| value.as_deref().unwrap_or_default())
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the description carries no {attribute:?} attribute, got {:?}",
                    self.policy_name, self.attribute_names
                )
            })
    }
}

/// Ask `DescribeLoadBalancerPolicies` about `policy_names` (empty leaving the list out of
/// the request) and NO load balancer, which is the sample-policy form of the call.
async fn describe_samples(
    router: &MultiServiceRouter,
    region: &str,
    policy_names: &[&str],
) -> Wire {
    let params: Vec<(String, String)> = policy_names
        .iter()
        .enumerate()
        .map(|(index, policy_name)| {
            param(&format!("PolicyNames.member.{}", index + 1), policy_name)
        })
        .collect();
    call(router, region, "DescribeLoadBalancerPolicies", &params).await
}

/// The `PolicyDescriptions` of a success envelope, in the order it reported them.
fn reported(answer: &Wire) -> Vec<Reported> {
    let doc = super::wire::parse(&answer.body);
    let result = answer.result(&doc);
    super::wire::members(result, "PolicyDescriptions")
        .into_iter()
        .map(|description| {
            let values: Vec<(String, Option<String>)> =
                super::wire::members(description, "PolicyAttributeDescriptions")
                    .into_iter()
                    .map(|attribute| {
                        (
                            super::wire::kid_text(attribute, "AttributeName").unwrap_or_default(),
                            super::wire::kid_text(attribute, "AttributeValue"),
                        )
                    })
                    .collect();
            Reported {
                policy_name: super::wire::kid_text(description, "PolicyName").unwrap_or_default(),
                policy_type_name: super::wire::kid_text(description, "PolicyTypeName")
                    .unwrap_or_default(),
                attribute_names: values.iter().map(|(name, _)| name.clone()).collect(),
                values,
            }
        })
        .collect()
}

/// The whole catalog as one describe naming nothing reports it, failing the test if the
/// call does not succeed.
async fn whole_catalog(router: &MultiServiceRouter) -> Vec<Reported> {
    let all = describe_samples(router, REGION, &[]).await;
    assert_eq!(
        all.status, 200,
        "a describe naming no load balancer must succeed, got {}",
        all.body
    );
    reported(&all)
}

#[tokio::test]
async fn the_sample_describe_reports_every_published_sample_policy() {
    let router = emulator();
    let catalog = whole_catalog(&router).await;

    let mut reported_names: Vec<&str> = catalog
        .iter()
        .map(|policy| policy.policy_name.as_str())
        .collect();
    reported_names.sort_unstable();
    assert_eq!(
        reported_names, PUBLISHED_SAMPLE_POLICY_NAMES,
        "the sample describe must report the whole published catalog, got {reported_names:?}"
    );

    // The catalog the handler projects and the published set are the same set, so no
    // predefined policy is selectable that the unfiltered describe does not report.
    let mut catalogued: Vec<&str> = SAMPLE_POLICIES.iter().map(|policy| policy.name).collect();
    catalogued.sort_unstable();
    assert_eq!(
        catalogued, PUBLISHED_SAMPLE_POLICY_NAMES,
        "the predefined catalog must be the published set, got {catalogued:?}"
    );

    for policy in &catalog {
        assert!(
            policy.policy_name.starts_with(SAMPLE_POLICY_NAME_PREFIX),
            "case {:?}: a sample policy's name must carry the documented prefix \
             {SAMPLE_POLICY_NAME_PREFIX:?}",
            policy.policy_name
        );
        assert_eq!(
            policy.policy_type_name, SSL_NEGOTIATION_POLICY_TYPE,
            "case {:?}: a sample policy configures SSL negotiation, got {:?}",
            policy.policy_name, policy.policy_type_name
        );
    }
}

#[tokio::test]
async fn every_sample_policy_is_described_by_the_whole_attribute_set_of_its_policy_type() {
    let router = emulator();
    let catalog = whole_catalog(&router).await;
    // The protocol attributes, the server cipher-order attribute and one per published
    // cipher suite: a description short of any of them is the defect under test. The
    // expectation is the list written out in this suite, not the catalog's own, so the
    // assertion below fails if the catalog is shortened.
    let catalogued: Vec<&str> = ssl_negotiation_attribute_names().collect();
    assert_eq!(
        catalogued, PUBLISHED_ATTRIBUTE_NAMES,
        "the policy type's attribute set must be the published one, got {catalogued:?}"
    );
    assert_eq!(
        PUBLISHED_ATTRIBUTE_NAMES.len(),
        SSL_PROTOCOL_ATTRIBUTES.len() + 1 + SSL_CIPHER_ATTRIBUTES.len(),
        "the attribute set must be the protocols, the cipher order and the ciphers"
    );
    let expected_attributes: Vec<String> = PUBLISHED_ATTRIBUTE_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();

    for policy in &catalog {
        assert_eq!(
            policy.attribute_names, expected_attributes,
            "case {:?}: every attribute of the policy type must be described, in the \
             published order, got {:?}",
            policy.policy_name, policy.attribute_names
        );
        for (name, value) in &policy.values {
            assert!(
                value.as_deref() == Some(ATTRIBUTE_VALUE_ENABLED)
                    || value.as_deref() == Some(ATTRIBUTE_VALUE_DISABLED),
                "case {:?}: attribute {name:?} must be reported enabled or disabled, got \
                 {value:?}",
                policy.policy_name
            );
        }
        assert!(
            SSL_CIPHER_ATTRIBUTES
                .iter()
                .any(|cipher| policy.value(cipher) == ATTRIBUTE_VALUE_ENABLED),
            "case {:?}: a cipher configuration must enable at least one cipher suite, got \
             {:?}",
            policy.policy_name,
            policy.values
        );
        // SSLv3 excepted, a configuration's protocol set is a contiguous range of versions,
        // and no Classic load balancer negotiates SSLv2 at all: the policy type carries no
        // attribute for it, so no description reports one.
        assert!(
            !policy
                .attribute_names
                .iter()
                .any(|name| name == "Protocol-SSLv2"),
            "case {:?}: the policy type carries no SSLv2 attribute, got {:?}",
            policy.policy_name,
            policy.attribute_names
        );
    }
}

#[tokio::test]
async fn each_published_configuration_reports_what_its_publication_enables() {
    let router = emulator();
    let catalog = whole_catalog(&router).await;

    // One row per published configuration, naming attributes of each KIND — a protocol, the
    // server cipher-order attribute and cipher suites a publication added or removed. Source:
    // the predefined-SSL-security-policy table of the Classic Load Balancer documentation,
    // read on policies and ciphers the recorded request did not cite.
    let rows: &[(&str, &str, &str)] = &[
        // The minimum-protocol variants turn the lower protocols off and keep the suites.
        (
            "ELBSample-ELBSecurityPolicy-TLS-1-2-2017-01",
            "Protocol-TLSv1",
            ATTRIBUTE_VALUE_DISABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-TLS-1-2-2017-01",
            "Protocol-TLSv1.1",
            ATTRIBUTE_VALUE_DISABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-TLS-1-2-2017-01",
            "Protocol-TLSv1.2",
            ATTRIBUTE_VALUE_ENABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-TLS-1-1-2017-01",
            "Protocol-TLSv1",
            ATTRIBUTE_VALUE_DISABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-TLS-1-1-2017-01",
            "Protocol-TLSv1.1",
            ATTRIBUTE_VALUE_ENABLED,
        ),
        // The current configuration: the server's cipher order, no triple-DES.
        (
            "ELBSample-ELBSecurityPolicy-2016-08",
            SERVER_DEFINED_CIPHER_ORDER_ATTRIBUTE,
            ATTRIBUTE_VALUE_ENABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-2016-08",
            "DES-CBC3-SHA",
            ATTRIBUTE_VALUE_DISABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-2016-08",
            "ECDHE-ECDSA-AES128-GCM-SHA256",
            ATTRIBUTE_VALUE_ENABLED,
        ),
        // Triple-DES still on, the ephemeral-DH suites dropped.
        (
            "ELBSample-ELBSecurityPolicy-2015-05",
            "DES-CBC3-SHA",
            ATTRIBUTE_VALUE_ENABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-2015-05",
            "DHE-RSA-AES128-SHA",
            ATTRIBUTE_VALUE_DISABLED,
        ),
        // The ephemeral-DH suites still on, triple-DES re-enabled for older clients.
        (
            "ELBSample-ELBSecurityPolicy-2015-03",
            "DHE-RSA-AES128-SHA",
            ATTRIBUTE_VALUE_ENABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-2015-03",
            "DES-CBC3-SHA",
            ATTRIBUTE_VALUE_ENABLED,
        ),
        // The configuration that dropped RC4 and triple-DES.
        (
            "ELBSample-ELBSecurityPolicy-2015-02",
            "DES-CBC3-SHA",
            ATTRIBUTE_VALUE_DISABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-2015-02",
            "RC4-SHA",
            ATTRIBUTE_VALUE_DISABLED,
        ),
        // The 2014 suites, with and without SSLv3.
        (
            "ELBSample-ELBSecurityPolicy-2014-10",
            "Protocol-SSLv3",
            ATTRIBUTE_VALUE_DISABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-2014-10",
            "RC4-SHA",
            ATTRIBUTE_VALUE_ENABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-2014-01",
            "Protocol-SSLv3",
            ATTRIBUTE_VALUE_ENABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-2014-01",
            "CAMELLIA128-SHA",
            ATTRIBUTE_VALUE_ENABLED,
        ),
        // The oldest configuration: no TLS 1.1, no forward-secret suites, no server order.
        (
            "ELBSample-ELBSecurityPolicy-2011-08",
            "Protocol-TLSv1.1",
            ATTRIBUTE_VALUE_DISABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-2011-08",
            SERVER_DEFINED_CIPHER_ORDER_ATTRIBUTE,
            ATTRIBUTE_VALUE_DISABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-2011-08",
            "ECDHE-RSA-AES128-SHA",
            ATTRIBUTE_VALUE_DISABLED,
        ),
        (
            "ELBSample-ELBSecurityPolicy-2011-08",
            "AES256-SHA",
            ATTRIBUTE_VALUE_ENABLED,
        ),
        // The two default-cipher samples: the CLIENT's cipher order, and OpenSSL's default
        // negotiating SSLv3 and every suite the policy type publishes.
        (
            "ELBSample-ELBDefaultCipherPolicy",
            SERVER_DEFINED_CIPHER_ORDER_ATTRIBUTE,
            ATTRIBUTE_VALUE_DISABLED,
        ),
        (
            "ELBSample-ELBDefaultCipherPolicy",
            "Protocol-SSLv3",
            ATTRIBUTE_VALUE_DISABLED,
        ),
        (
            "ELBSample-ELBDefaultCipherPolicy",
            "Protocol-TLSv1.2",
            ATTRIBUTE_VALUE_ENABLED,
        ),
        (
            "ELBSample-OpenSSLDefaultCipherPolicy",
            "Protocol-SSLv3",
            ATTRIBUTE_VALUE_ENABLED,
        ),
        (
            "ELBSample-OpenSSLDefaultCipherPolicy",
            "RC4-MD5",
            ATTRIBUTE_VALUE_ENABLED,
        ),
    ];

    for (policy_name, attribute, expected) in rows {
        let policy = catalog
            .iter()
            .find(|policy| policy.policy_name == *policy_name)
            .unwrap_or_else(|| panic!("case {policy_name:?}: the catalog must report it"));
        assert_eq!(
            policy.value(attribute),
            *expected,
            "case ({policy_name:?}, {attribute:?}): expected {expected:?}, got {:?}",
            policy.value(attribute)
        );
    }

    // OpenSSL's default covers the whole published roster, which is what makes it the
    // widest sample: every cipher attribute of the policy type is enabled on it.
    let openssl = catalog
        .iter()
        .find(|policy| policy.policy_name == "ELBSample-OpenSSLDefaultCipherPolicy")
        .expect("the catalog must report OpenSSL's default cipher sample");
    for cipher in SSL_CIPHER_ATTRIBUTES {
        assert_eq!(
            openssl.value(cipher),
            ATTRIBUTE_VALUE_ENABLED,
            "case {cipher:?}: OpenSSL's default covers the published cipher roster, got {:?}",
            openssl.value(cipher)
        );
    }
}

#[tokio::test]
async fn policy_names_selects_from_the_catalog_and_a_name_outside_it_is_refused() {
    let router = emulator();
    let catalog = whole_catalog(&router).await;

    // The rows differ in how many names the request carries, which names, and whether it
    // repeats one — a repeated name is not an error, so the policy is reported once.
    let selections: &[(&str, &[&str])] = &[
        ("no name at all", &[]),
        ("one name", &["ELBSample-ELBSecurityPolicy-2011-08"]),
        (
            "two names",
            &[
                "ELBSample-OpenSSLDefaultCipherPolicy",
                "ELBSample-ELBSecurityPolicy-TLS-1-2-2017-01",
            ],
        ),
        (
            "one name, twice",
            &[
                "ELBSample-ELBSecurityPolicy-2015-03",
                "ELBSample-ELBSecurityPolicy-2015-03",
            ],
        ),
        ("every name", PUBLISHED_SAMPLE_POLICY_NAMES),
    ];

    for (case, names) in selections {
        let answer = describe_samples(&router, REGION, names).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the sample describe must succeed, got {}",
            answer.body
        );
        let selected = reported(&answer);
        let expected: Vec<Reported> = catalog
            .iter()
            .filter(|policy| names.is_empty() || names.contains(&policy.policy_name.as_str()))
            .cloned()
            .collect();
        assert_eq!(
            selected, expected,
            "case {case:?}: the selection must report exactly the named policies, as the \
             whole catalog reports them, got {:?}",
            selected
                .iter()
                .map(|policy| policy.policy_name.as_str())
                .collect::<Vec<&str>>()
        );
    }

    // A name the catalog does not carry names no policy: the unprefixed form of a
    // published name is not a sample policy either.
    for case in [
        "ELBSample-NotAPolicy",
        "ELBSecurityPolicy-2016-08",
        "ELBSample-ELBSecurityPolicy-2024-01",
    ] {
        let refused = describe_samples(&router, REGION, &[case]).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a name outside the catalog must be refused 400, got {}",
            refused.body
        );
        assert_eq!(
            refused.error_code(),
            "PolicyNotFound",
            "case {case:?}: the refusal must be PolicyNotFound, got {}",
            refused.body
        );
        assert_eq!(
            error_message(&refused),
            format!("There is no policy with name {case}."),
            "case {case:?}: the refusal must name the policy, got {}",
            refused.body
        );
    }

    // A sample describe is a report: the catalog answers the same after every call above.
    assert_eq!(
        whole_catalog(&router).await,
        catalog,
        "the catalog must answer the same on every describe"
    );
}
