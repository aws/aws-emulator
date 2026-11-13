// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the custom endpoint a `DomainEndpointOptions` container asks the
//! service to serve — the container `CreateDomain` and `UpdateDomainConfig` both take.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The certificates the rows name are brought into Certificate Manager through that
//! service's OWN `ImportCertificate`, mounted on the same shared engine
//! ([`Emulator::with_certificates`]), and every certificate is minted by `rcgen` — something
//! other than the code under test — so the names a row's certificate covers are the names
//! that service parsed out of a real X.509 body, and every ARN asserted on is the one the
//! import answered rather than a value written into this file.
//!
//! The rule under test, asserted over a table rather than one call: a container that ENABLES
//! a custom endpoint is accepted only when it names BOTH the endpoint and the certificate
//! that endpoint's TLS is terminated with, that certificate is one the account holds, and
//! that certificate COVERS the requested host; anything else is refused with
//! `ValidationException` (HTTP 400) and neither creates a domain nor changes the endpoint of
//! one. A container that enables no custom endpoint asks for no name of the caller's own to
//! be served and so has nothing to resolve.
//!
//! The rows differ along what the rule generalises over: which half of the pair the request
//! leaves out (both, the endpoint, the certificate), whether the certificate ARN resolves (a
//! well-formed ARN nobody imported, one of another Region, one the account holds), how the
//! certificate's names relate to the requested host (the subject name, a later subject
//! alternative name, a wildcard one label up, a wildcard two labels up, a host differing
//! only in case, a host the certificate names nothing near), and whether the container
//! enables a custom endpoint at all — each row driven through BOTH operations that take the
//! container.

use std::time::Duration;

use serde_json::{json, Value};

use super::domain_configuration_rules::{assert_no_domain, assert_refusal};
use super::domain_lifecycle::{baseline_create, create_baseline, Emulator, REGION};
use super::service_software_update::message;

/// The error every refusal below carries: the operation's modelled 400 for a configuration
/// the service will not provision.
const REFUSAL: &str = "ValidationException";

/// How many times a row looks for a change the clock owes a domain, and how long it waits
/// between looks. Nothing in the emulator sleeps: the span is materialised by whichever
/// later read reads the record, so the loop looks again rather than waiting a span out.
const POLL_LIMIT: u32 = 240;
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// A Region other than the one the requests are signed for, where a row imports the
/// certificate a domain of this Region therefore resolves nothing for.
const OTHER_REGION: &str = "us-west-2";

/// The TLS security policy a domain is created with when the request names none, as the
/// service publishes it — asserted on the accepted row, since a custom endpoint is the
/// HTTPS endpoint that policy governs.
const DEFAULT_TLS_POLICY: &str = "Policy-Min-TLS-1-2-2019-07";

/// The certificate each row's host is asked to be served under.
#[derive(Clone, Copy)]
enum Certificate {
    /// The certificate whose subject name IS the host the rows ask for.
    Exact,
    /// A certificate naming several hosts, the row's among them but not first.
    SeveralNames,
    /// A certificate whose one name is a wildcard over one label.
    Wildcard,
    /// A certificate naming an internal load balancer's host and nothing else — the shape
    /// a client imports and then points a dashboards hostname at by mistake.
    Internal,
    /// The certificate imported in [`OTHER_REGION`], which this Region holds none of.
    AnotherRegion,
    /// The certificate this Region holds, named by an ARN of ANOTHER account — which this
    /// caller holds no certificate under.
    AnotherAccount,
    /// A well-formed certificate ARN nobody imported.
    Unknown,
    /// No certificate member at all.
    Absent,
}

/// The names each imported certificate covers. The hosts below are drawn from them, so a
/// row's expectation is about the certificate's own name set rather than about a string.
const EXACT_HOST: &str = "dashboards.probe-endpoint.example.com";
const SEVERAL_FIRST: &str = "primary.probe-several.example.com";
const SEVERAL_LATER: &str = "dashboards.probe-several.example.com";
const WILDCARD_NAME: &str = "*.probe-wildcard.example.com";
const WILDCARD_HOST: &str = "dashboards.probe-wildcard.example.com";
const WILDCARD_DEEPER_HOST: &str = "a.dashboards.probe-wildcard.example.com";
const INTERNAL_NAME: &str = "internal-probe-1234567890.us-east-1.elb.example.com";

/// The certificate id of the ARN no import mints: a nil UUID, which is the form a
/// certificate id takes and a value Certificate Manager's own minting never answers.
const NIL_CERTIFICATE_ID: &str = "00000000-0000-0000-0000-000000000000";

/// An account other than this emulator's, for the row that moves a held certificate's ARN
/// into one — twelve digits, the form an account id takes.
const OTHER_ACCOUNT: &str = "210987654321";

/// What a row expects of the answer.
enum Expected {
    /// Accepted: the domain reports the container back.
    Accepted,
    /// Refused because the container enabled an endpoint without both of its members.
    Incomplete,
    /// Refused because the certificate it named resolves to nothing in the account.
    NoSuchCertificate,
    /// Refused because the certificate it named does not cover the requested host.
    NotCovered,
}

/// One row: the container to send and what the service must answer it with.
struct Row {
    case: &'static str,
    /// Whether the container turns a custom endpoint on.
    enabled: Option<bool>,
    /// The host it asks to be served, if any.
    endpoint: Option<&'static str>,
    certificate: Certificate,
    expected: Expected,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an enabled endpoint naming neither member",
            enabled: Some(true),
            endpoint: None,
            certificate: Certificate::Absent,
            expected: Expected::Incomplete,
        },
        Row {
            case: "an enabled endpoint naming the host but no certificate",
            enabled: Some(true),
            endpoint: Some(EXACT_HOST),
            certificate: Certificate::Absent,
            expected: Expected::Incomplete,
        },
        Row {
            case: "an enabled endpoint naming the certificate but no host",
            enabled: Some(true),
            endpoint: None,
            certificate: Certificate::Exact,
            expected: Expected::Incomplete,
        },
        Row {
            case: "a well-formed certificate ARN nobody imported",
            enabled: Some(true),
            endpoint: Some(EXACT_HOST),
            certificate: Certificate::Unknown,
            expected: Expected::NoSuchCertificate,
        },
        Row {
            case: "a certificate imported in another Region",
            enabled: Some(true),
            endpoint: Some(EXACT_HOST),
            certificate: Certificate::AnotherRegion,
            expected: Expected::NoSuchCertificate,
        },
        Row {
            case: "a certificate of this Region named by another account's ARN",
            enabled: Some(true),
            endpoint: Some(EXACT_HOST),
            certificate: Certificate::AnotherAccount,
            expected: Expected::NoSuchCertificate,
        },
        Row {
            case: "a certificate covering only an internal load balancer's host",
            enabled: Some(true),
            endpoint: Some(EXACT_HOST),
            certificate: Certificate::Internal,
            expected: Expected::NotCovered,
        },
        Row {
            case: "a wildcard certificate asked for a host two labels below it",
            enabled: Some(true),
            endpoint: Some(WILDCARD_DEEPER_HOST),
            certificate: Certificate::Wildcard,
            expected: Expected::NotCovered,
        },
        Row {
            case: "a certificate whose subject name is the host",
            enabled: Some(true),
            endpoint: Some(EXACT_HOST),
            certificate: Certificate::Exact,
            expected: Expected::Accepted,
        },
        Row {
            case: "a certificate whose LATER subject alternative name is the host",
            enabled: Some(true),
            endpoint: Some(SEVERAL_LATER),
            certificate: Certificate::SeveralNames,
            expected: Expected::Accepted,
        },
        Row {
            case: "a wildcard certificate asked for a host one label below it",
            enabled: Some(true),
            endpoint: Some(WILDCARD_HOST),
            certificate: Certificate::Wildcard,
            expected: Expected::Accepted,
        },
        Row {
            case: "a host differing from the certificate's name only in case",
            enabled: Some(true),
            endpoint: Some("Dashboards.Probe-Endpoint.Example.COM"),
            certificate: Certificate::Exact,
            expected: Expected::Accepted,
        },
        Row {
            case: "a container that enables no custom endpoint and names neither member",
            enabled: Some(false),
            endpoint: None,
            certificate: Certificate::Absent,
            expected: Expected::Accepted,
        },
        Row {
            case: "a container that enables nothing and leaves the flag out",
            enabled: None,
            endpoint: None,
            certificate: Certificate::Absent,
            expected: Expected::Accepted,
        },
    ]
}

/// The certificate ARNs the rows name, as the imports answered them.
struct Certificates {
    exact: String,
    several_names: String,
    wildcard: String,
    internal: String,
    another_region: String,
    another_account: String,
    unknown: String,
}

impl Certificates {
    /// The ARN `certificate` names, and `None` for a row that names no certificate member.
    fn arn(&self, certificate: Certificate) -> Option<&str> {
        match certificate {
            Certificate::Exact => Some(&self.exact),
            Certificate::SeveralNames => Some(&self.several_names),
            Certificate::Wildcard => Some(&self.wildcard),
            Certificate::Internal => Some(&self.internal),
            Certificate::AnotherRegion => Some(&self.another_region),
            Certificate::AnotherAccount => Some(&self.another_account),
            Certificate::Unknown => Some(&self.unknown),
            Certificate::Absent => None,
        }
    }
}

/// A real self-signed certificate covering `names`, as the PEM pair `ImportCertificate`
/// takes: the leaf and the private key that matches it. The first name is also the subject
/// common name, which is the name Certificate Manager reports as the certificate's own.
fn minted(names: &[&str]) -> (String, String) {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("a key pair");
    let mut params = rcgen::CertificateParams::new(
        names.iter().map(|name| (*name).to_string()).collect::<Vec<String>>(),
    )
    .expect("certificate parameters");
    let mut subject = rcgen::DistinguishedName::new();
    subject.push(
        rcgen::DnType::CommonName,
        *names.first().expect("a certificate covers at least one name"),
    );
    params.distinguished_name = subject;
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = rcgen::date_time_ymd(2100, 1, 1);
    let certificate = params
        .self_signed(&key)
        .expect("a self-signed certificate")
        .pem();
    (certificate, key.serialize_pem())
}

/// Import a certificate covering `names` into Certificate Manager at `region` through that
/// service's own `ImportCertificate`, and answer the ARN it was stored under.
///
/// The stored certificate is read back with that service's own `DescribeCertificate` and its
/// `SubjectAlternativeNames` asserted to be the names handed in, so a row's expectation
/// rests on the name set Certificate Manager holds rather than on what this file assumed.
async fn imported(emulator: &Emulator, region: &str, names: &[&str]) -> String {
    let (certificate, private_key) = minted(names);
    let answer = emulator
        .acm_call(
            region,
            "ImportCertificate",
            json!({
                "Certificate": ::aws_smithy_types::base64::encode(certificate.as_bytes()),
                "PrivateKey": ::aws_smithy_types::base64::encode(private_key.as_bytes()),
            }),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case ImportCertificate({names:?}) in {region}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let arn = answer.body["CertificateArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case ImportCertificate({names:?}): expected a CertificateArn, got {:?}", answer.body)
        })
        .to_string();

    let read = emulator
        .acm_call(region, "DescribeCertificate", json!({ "CertificateArn": arn }))
        .await;
    assert_eq!(
        read.status, 200,
        "case DescribeCertificate({arn}): expected 200, got {} with {:?}",
        read.status, read.body
    );
    let held: Vec<String> = read.body["Certificate"]["SubjectAlternativeNames"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case DescribeCertificate({arn}): expected SubjectAlternativeNames, got {:?}", read.body)
        })
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect();
    let expected: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
    assert_eq!(
        held, expected,
        "case DescribeCertificate({arn}): expected the imported name set {expected:?}, got \
         {held:?}"
    );
    arn
}

/// Every certificate the rows name, imported through Certificate Manager's own operations.
async fn certificates(emulator: &Emulator) -> Certificates {
    let exact = imported(emulator, REGION, &[EXACT_HOST]).await;
    Certificates {
        several_names: imported(emulator, REGION, &[SEVERAL_FIRST, SEVERAL_LATER]).await,
        wildcard: imported(emulator, REGION, &[WILDCARD_NAME]).await,
        internal: imported(emulator, REGION, &[INTERNAL_NAME]).await,
        another_region: imported(emulator, OTHER_REGION, &[EXACT_HOST]).await,
        // The held certificate's own ARN with its account segment moved into another
        // account's: an ARN of the form `CustomEndpointCertificateArn` admits, naming a
        // certificate this caller holds none of.
        another_account: exact.replace(
            &format!(":{}:", ::engine::emulator_account_id()),
            &format!(":{OTHER_ACCOUNT}:"),
        ),
        // A well-formed certificate ARN of this account and Region whose id is a nil UUID:
        // the form `CertificateArn` admits, and an id no import mints (Certificate Manager
        // mints a random one), so nothing in the account resolves for it.
        unknown: format!(
            "arn:aws:acm:{REGION}:{}:certificate/{NIL_CERTIFICATE_ID}",
            ::engine::emulator_account_id()
        ),
        exact,
    }
}

/// The `DomainEndpointOptions` container a row sends.
fn container(row: &Row, held: &Certificates) -> Value {
    let mut options = json!({ "EnforceHTTPS": true });
    if let Some(enabled) = row.enabled {
        options["CustomEndpointEnabled"] = json!(enabled);
    }
    if let Some(endpoint) = row.endpoint {
        options["CustomEndpoint"] = json!(endpoint);
    }
    if let Some(arn) = held.arn(row.certificate) {
        options["CustomEndpointCertificateArn"] = json!(arn);
    }
    options
}

/// The sentence a row's refusal must carry, stated here independently of the constants the
/// implementation renders it from.
fn refusal_message(row: &Row, held: &Certificates) -> Option<String> {
    let arn = held.arn(row.certificate).unwrap_or_default();
    match row.expected {
        Expected::Accepted => None,
        Expected::Incomplete => Some(
            "Please provide both CustomEndpoint and CustomEndpointCertificateArn fields to \
             create a custom endpoint."
                .to_string(),
        ),
        Expected::NoSuchCertificate => {
            Some(format!("ACM Certificate {arn} not found in your account."))
        }
        Expected::NotCovered => Some(format!(
            "Certificate {arn} does not contain the custom domain name {}.",
            row.endpoint.unwrap_or_default()
        )),
    }
}

/// The `DomainEndpointOptions` `DescribeDomain` reports for `name`.
async fn reported_options(emulator: &Emulator, name: &str) -> Option<Value> {
    let answer = emulator.describe(name).await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"]
        .get("DomainEndpointOptions")
        .cloned()
}

/// Look at `name` through `DescribeDomain` until it reports no work in flight — the
/// condition a further change waits for, since the service applies one configuration change
/// of a domain at a time. The wait is for the CONDITION: nothing here asserts on how long it
/// took, and the budget ([`POLL_LIMIT`] × [`POLL_INTERVAL`]) is many times the span the clock
/// owes a change, so a loaded host still gets there.
async fn settled(emulator: &Emulator, name: &str) {
    for _ in 0..POLL_LIMIT {
        let answer = emulator.describe(name).await;
        assert_eq!(
            answer.status, 200,
            "case DescribeDomain({name:?}): expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        if answer.body["DomainStatus"]["Processing"].as_bool() == Some(false) {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {name:?}: expected the domain's change to land within the poll budget");
}

/// Every member of `sent` is reported back by `held`, which is what a round trip of the
/// container asserts: the service completes the container with the endpoint settings it
/// defaults, so the reported container is a superset rather than an equal.
fn reports_back(sent: &Value, held: &Value, case: &str) {
    for (member, value) in sent.as_object().expect("the row sends an object") {
        assert_eq!(
            held.get(member),
            Some(value),
            "case {case}: expected the domain to report {member} as {value:?}, got {:?}",
            held.get(member)
        );
    }
}

#[tokio::test]
async fn an_enabled_custom_endpoint_must_name_a_certificate_of_the_account_that_covers_it() {
    let emulator = Emulator::with_certificates();
    let held = certificates(&emulator).await;

    for (index, row) in rows().into_iter().enumerate() {
        let sent = container(&row, &held);
        let case = format!("CreateDomain({})", row.case);
        let name = format!("ce-create-{index}");
        let mut body = baseline_create(&name);
        body["DomainEndpointOptions"] = sent.clone();
        let answer = emulator.create(body).await;

        match refusal_message(&row, &held) {
            Some(expected) => {
                assert_refusal(&answer, REFUSAL, &case);
                assert_eq!(
                    message(&answer, &case),
                    expected,
                    "case {case}: expected the refusal to state why the endpoint cannot be \
                     served"
                );
                // A refused create leaves no domain behind, so the name it asked for is
                // still free.
                assert_no_domain(&emulator, &name).await;
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected 200, got {} with {:?}",
                    answer.status, answer.body
                );
                reports_back(&sent, &answer.body["DomainStatus"]["DomainEndpointOptions"], &case);
                let reported = reported_options(&emulator, &name)
                    .await
                    .unwrap_or_else(|| panic!("case {case}: expected the stored container"));
                reports_back(&sent, &reported, &case);
                // The HTTPS endpoint a custom endpoint is served on runs the policy the
                // service picks when the request names none.
                assert_eq!(
                    reported["TLSSecurityPolicy"],
                    json!(DEFAULT_TLS_POLICY),
                    "case {case}: expected the defaulted TLS policy, got {:?}",
                    reported["TLSSecurityPolicy"]
                );
            }
        }
    }
}

#[tokio::test]
async fn the_same_rule_governs_a_change_to_a_domains_custom_endpoint() {
    let emulator = Emulator::with_certificates();
    let held = certificates(&emulator).await;

    // One settled domain for the rows that CHANGE a domain's endpoint: a domain with work
    // in flight refuses a further change, so the create's own change lands first.
    let name = "ce-update";
    create_baseline(&emulator, name).await;
    settled(&emulator, name).await;
    let mut before = reported_options(&emulator, name).await;

    for row in rows() {
        let sent = container(&row, &held);
        let case = format!("UpdateDomainConfig({})", row.case);
        let answer = emulator
            .update_config(name, json!({ "DomainEndpointOptions": sent.clone() }))
            .await;

        match refusal_message(&row, &held) {
            Some(expected) => {
                assert_refusal(&answer, REFUSAL, &case);
                assert_eq!(
                    message(&answer, &case),
                    expected,
                    "case {case}: expected the refusal to state why the endpoint cannot be \
                     served"
                );
                // A refused change stores nothing: the domain keeps the endpoint it had.
                let reported = reported_options(&emulator, name).await;
                assert_eq!(
                    reported, before,
                    "case {case}: a refused change must leave the domain answering on \
                     {before:?}, got {reported:?}"
                );
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected 200, got {} with {:?}",
                    answer.status, answer.body
                );
                // An accepted change lands before the next row starts another.
                settled(&emulator, name).await;
                let reported = reported_options(&emulator, name)
                    .await
                    .unwrap_or_else(|| panic!("case {case}: expected the stored container"));
                reports_back(&sent, &reported, &case);
                before = Some(reported);
            }
        }
    }
}
