// SPDX-License-Identifier: CC0-1.0

//! DNS validation end to end: what a PUBLISHED validation record does to a public
//! certificate, driven over both services' real wires (ACM's `POST /` awsJson1_1 and
//! Route 53's restXml `/2013-04-01/...`) on one shared engine.
//!
//! RULE under test: a public certificate whose domains are validated by DNS leaves
//! `PENDING_VALIDATION` for `ISSUED`, with every `DomainValidation.ValidationStatus`
//! `SUCCESS`, once the CNAME ACM handed out for EVERY covered name resolves in the
//! account's public DNS to the value ACM handed out; while any one of them does not, the
//! request keeps waiting the documented 72-hour window out and carries no
//! `FailureReason`.
//!
//! The tables span what the rule generalises over: how many names the certificate
//! covers and how deep they sit, a wildcard name as against a plain one, the spelling
//! the record is published with (trailing dot, letter case), and — on the negative side
//! — each way a published record can fail to prove the name (absent, only some names,
//! the wrong value, the wrong name, the wrong type, a private zone, another
//! certificate's challenge).
//!
//! Nothing here sleeps for the emulator's owed span except the one test that pins that
//! a client which publishes the record and polls reaches `ISSUED` on the wall clock.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state::VALIDATION_SETTLE_DELAY;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{describe, request_certificate};
use crate::certificate_validation_state_machine::{
    arns_in_statuses, make_validation_due_now, pending_due_in, validations,
};

/// The host a Route 53 request addresses, which is also the routing signal the router
/// resolves that service from.
const DNS_ENDPOINT: &str = "route53.amazonaws.com";

/// The `VPCRegion` the private-zone row associates its VPC in (a value of the modelled
/// `VPCRegion` enum).
const VPC_REGION: &str = "us-east-1";

/// The VPC id the private-zone row names, in the documented `vpc-<8 hex>` form.
const PRIVATE_ZONE_VPC: &str = "vpc-4d3c2b1a";

/// A router serving ACM and Route 53 over ONE engine, beside that engine — so the
/// certificate's challenge and the zone it is published in are the same account's, and
/// a test can read the instant the emulator stored for an owed transition.
pub(crate) fn router_with_dns() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    {
        // Which VPCs the account holds is Amazon EC2's fact, and Route 53 resolves the
        // VPC a private-zone create names against it. Written straight into that store
        // because the id has to be the one the request names and `CreateVpc` mints its
        // own.
        let state = ec2::state::service_state(&*engine)
            .expect("the shared engine must carry the Amazon EC2 store");
        let mut store = state.lock();
        store.vpcs.insert(
            PRIVATE_ZONE_VPC.to_string(),
            ec2::ops::types::Vpc {
                vpc_id: Some(PRIVATE_ZONE_VPC.to_string()),
                cidr_block: Some("10.0.0.0/16".to_string()),
                state: Some(ec2::state::VPC_STATE_AVAILABLE.to_string()),
                ..Default::default()
            },
        );
        store
            .vpc_regions
            .insert(PRIVATE_ZONE_VPC.to_string(), VPC_REGION.to_string());
    }
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_acm_certificatemanager::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// Send one Route 53 request and return `(status, body)`.
///
/// `pub(crate)` so a sibling suite in this test target reaches the DNS service over the
/// SAME wire rather than growing a second copy of it.
pub(crate) async fn dns_call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &str,
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header(http::header::HOST, DNS_ENDPOINT)
        .header(http::header::CONTENT_TYPE, "application/xml")
        .body(Bytes::from(body.to_owned()))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    (status, body)
}

/// The bare id following `prefix` in a Route 53 reply — the form a hosted zone
/// (`/hostedzone/Z…`) and a change (`/change/C…`) report their id in.
fn bare_id_after(body: &str, prefix: &str) -> String {
    let start = body
        .find(prefix)
        .unwrap_or_else(|| panic!("the reply must carry a {prefix} id, got {body}"))
        + prefix.len();
    let rest = &body[start..];
    let end = rest.find('<').unwrap_or(rest.len());
    rest[..end].to_string()
}

/// The bare (`Z…`) zone id a `CreateHostedZone` reply carries.
fn zone_id_from(body: &str) -> String {
    bare_id_after(body, "/hostedzone/")
}

/// Create a hosted zone for `apex` and return its bare id. `extra` carries the
/// `HostedZoneConfig` and `VPC` blocks, so a row can ask for a private zone.
async fn create_zone(
    router: &MultiServiceRouter,
    apex: &str,
    caller_reference: &str,
    extra: &str,
) -> String {
    let body = format!(
        "<CreateHostedZoneRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
         <Name>{apex}</Name><CallerReference>{caller_reference}</CallerReference>{extra}\
         </CreateHostedZoneRequest>"
    );
    let (status, reply) = dns_call(router, "POST", "/2013-04-01/hostedzone", &body).await;
    assert_eq!(
        status, 201,
        "creating the hosted zone {apex}: expected 201, got {status} {reply}"
    );
    zone_id_from(&reply)
}

/// Create a PUBLIC hosted zone for `apex` and return its bare id.
pub(crate) async fn create_public_zone(
    router: &MultiServiceRouter,
    apex: &str,
    caller_reference: &str,
) -> String {
    create_zone(router, apex, caller_reference, "").await
}

/// Create a PRIVATE hosted zone for `apex`, associated with [`PRIVATE_ZONE_VPC`], and
/// return its bare id.
pub(crate) async fn create_private_zone(
    router: &MultiServiceRouter,
    apex: &str,
    caller_reference: &str,
) -> String {
    let extra = format!(
        "<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>\
         <VPC><VPCRegion>{VPC_REGION}</VPCRegion><VPCId>{PRIVATE_ZONE_VPC}</VPCId></VPC>"
    );
    create_zone(router, apex, caller_reference, &extra).await
}

/// One DNS record to write: the name, the type and the single value.
#[derive(Debug, Clone)]
pub(crate) struct Record {
    pub(crate) name: String,
    pub(crate) rr_type: String,
    pub(crate) value: String,
}

/// The TTL every record this suite writes carries. Immaterial to the rule — nothing
/// reads it — but a record set has one.
const RECORD_TTL: i64 = 60;

/// Submit one change batch applying `action` to `record`, assert it was accepted, and
/// return the bare (`C…`) id of the change it created — the id `GetChange` takes, which
/// a client polls for the propagation the submission owes.
pub(crate) async fn submit_change(
    case: &str,
    router: &MultiServiceRouter,
    zone_id: &str,
    action: &str,
    record: &Record,
) -> String {
    let body = format!(
        "<ChangeResourceRecordSetsRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
         <ChangeBatch><Changes><Change><Action>{action}</Action><ResourceRecordSet>\
         <Name>{}</Name><Type>{}</Type><TTL>{RECORD_TTL}</TTL>\
         <ResourceRecords><ResourceRecord><Value>{}</Value></ResourceRecord></ResourceRecords>\
         </ResourceRecordSet></Change></Changes></ChangeBatch>\
         </ChangeResourceRecordSetsRequest>",
        record.name, record.rr_type, record.value
    );
    let (status, reply) = dns_call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
        &body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: the {action} of {record:?} must be accepted, got {status} {reply}"
    );
    bare_id_after(&reply, "/change/")
}

/// The validation records ACM handed out for a described certificate, one per covered
/// name, in the order the response lists them.
fn handed_out_records(case: &str, detail: &Value) -> Vec<Record> {
    validations(case, detail)
        .iter()
        .map(|validation| {
            let record = &validation["ResourceRecord"];
            Record {
                name: record["Name"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: a DNS validation must carry a record name, got {validation}")
                    })
                    .to_string(),
                rr_type: record["Type"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: a DNS validation must carry a record type, got {validation}")
                    })
                    .to_string(),
                value: record["Value"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: a DNS validation must carry a record value, got {validation}")
                    })
                    .to_string(),
            }
        })
        .collect()
}

/// Request a DNS-validated certificate for `domain_name` (plus `sans`) and return its
/// ARN beside the records ACM handed out for it, after asserting the request is
/// `PENDING_VALIDATION` with every covered name still unproven.
pub(crate) async fn request_dns_validated(
    case: &str,
    router: &MultiServiceRouter,
    domain_name: &str,
    sans: &[&str],
) -> (String, Vec<Record>) {
    let mut body = json!({ "DomainName": domain_name, "ValidationMethod": "DNS" });
    if !sans.is_empty() {
        body["SubjectAlternativeNames"] = json!(sans);
    }
    let arn = request_certificate(router, &body).await;
    let pending = describe(router, &arn).await;
    assert_eq!(
        pending["Status"].as_str(),
        Some("PENDING_VALIDATION"),
        "case {case}: a requested certificate waits in PENDING_VALIDATION, got {pending}"
    );
    for validation in validations(case, &pending) {
        assert_eq!(
            validation["ValidationStatus"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case}: an unproven domain is PENDING_VALIDATION, got {validation}"
        );
        assert_eq!(
            validation["ResourceRecord"]["Type"].as_str(),
            Some("CNAME"),
            "case {case}: DNS validation publishes a CNAME, got {validation}"
        );
    }
    let records = handed_out_records(case, &pending);
    assert_eq!(
        records.len(),
        1 + sans.len(),
        "case {case}: expected one validation record per covered name, got {records:?}"
    );
    (arn, records)
}

/// Assert that `arn` is still waiting on proof: `PENDING_VALIDATION`, no
/// `FailureReason`, every domain unproven, and the outcome the store owes it still the
/// documented 72-hour timeout rather than an issuance.
pub(crate) async fn assert_still_waiting(
    case: &str,
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
) {
    let detail = describe(router, arn).await;
    assert_eq!(
        detail["Status"].as_str(),
        Some("PENDING_VALIDATION"),
        "case {case}: an unproven name leaves the request pending, got {detail}"
    );
    assert!(
        detail["FailureReason"].is_null(),
        "case {case}: a challenge nobody published is no request failure, got {detail}"
    );
    for validation in validations(case, &detail) {
        assert_eq!(
            validation["ValidationStatus"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case}: an unproven domain stays PENDING_VALIDATION, got {validation}"
        );
    }
    let due_in = pending_due_in(engine, arn)
        .unwrap_or_else(|| panic!("case {case}: the pending request owes no outcome at all"));
    assert!(
        due_in > VALIDATION_SETTLE_DELAY,
        "case {case}: expected the request to still owe the 72-hour timeout, \
         got an outcome due in {due_in:?}"
    );
}

/// Assert that `arn` has proof recorded: the store now owes it an ISSUANCE, due within
/// the settle span and not instantly, while the read still reports the pending state.
pub(crate) async fn assert_issuance_owed(
    case: &str,
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
) {
    let detail = describe(router, arn).await;
    assert_eq!(
        detail["Status"].as_str(),
        Some("PENDING_VALIDATION"),
        "case {case}: the issuance span is never zero, so the record is still pending \
         straight after publication, got {detail}"
    );
    let due_in = pending_due_in(engine, arn)
        .unwrap_or_else(|| panic!("case {case}: the proven request owes no outcome at all"));
    assert!(
        due_in > Duration::ZERO && due_in <= VALIDATION_SETTLE_DELAY,
        "case {case}: expected an issuance due within {VALIDATION_SETTLE_DELAY:?} and not \
         instantly, got {due_in:?}"
    );
}

/// Assert everything issuance implies for a described certificate.
pub(crate) fn assert_issued(case: &str, detail: &Value) {
    assert_eq!(
        detail["Status"].as_str(),
        Some("ISSUED"),
        "case {case}: a certificate whose validation records are all published is ISSUED, \
         got {detail}"
    );
    assert!(
        detail["FailureReason"].is_null(),
        "case {case}: an issued certificate carries no FailureReason, got {detail}"
    );
    assert_eq!(
        detail["Type"].as_str(),
        Some("AMAZON_ISSUED"),
        "case {case}: a requested certificate is AMAZON_ISSUED, got {detail}"
    );
    for validation in validations(case, detail) {
        assert_eq!(
            validation["ValidationStatus"].as_str(),
            Some("SUCCESS"),
            "case {case}: a proven domain is SUCCESS, got {validation}"
        );
    }
    let not_before = detail["NotBefore"].as_f64();
    let not_after = detail["NotAfter"].as_f64();
    match (not_before, not_after) {
        (Some(before), Some(after)) => assert!(
            before < after,
            "case {case}: expected NotBefore before NotAfter, got {before} and {after}"
        ),
        _ => panic!("case {case}: issuance opens a validity window, got {detail}"),
    }
}

/// One row of the positive table: what it is, the zone apex, the certificate's primary
/// domain, its subject alternative names, and how the publisher spells the record.
struct ProvenRow<'a> {
    case: &'a str,
    apex: &'a str,
    domain_name: &'a str,
    sans: &'a [&'a str],
    /// Rewrite the handed-out record into the spelling it is published with.
    spelled: fn(&Record) -> Record,
}

/// Publish the record exactly as ACM handed it out.
fn as_handed_out(record: &Record) -> Record {
    record.clone()
}

/// Publish the record without its trailing dot and in upper case — one more spelling of
/// the same DNS name, which is why it must prove the name just as the handed-out
/// spelling does.
fn shouted_without_the_root_dot(record: &Record) -> Record {
    Record {
        name: record.name.trim_end_matches('.').to_ascii_uppercase(),
        rr_type: record.rr_type.clone(),
        value: record.value.trim_end_matches('.').to_ascii_uppercase(),
    }
}

/// RULE: once the CNAME ACM handed out for every covered name is published in one of
/// the account's public hosted zones, with the value ACM handed out, the certificate is
/// `ISSUED` and every domain's `ValidationStatus` is `SUCCESS`.
#[tokio::test]
async fn a_published_validation_record_issues_the_certificate() {
    let rows = [
        ProvenRow {
            case: "one name a label under the apex",
            apex: "alpha.dns-proof.net",
            domain_name: "www.alpha.dns-proof.net",
            sans: &[],
            spelled: as_handed_out,
        },
        ProvenRow {
            case: "two names: the apex and a subdomain of it",
            apex: "beta.dns-proof.org",
            domain_name: "beta.dns-proof.org",
            sans: &["api.beta.dns-proof.org"],
            spelled: as_handed_out,
        },
        ProvenRow {
            case: "a wildcard name, proven through the domain it covers",
            apex: "gamma.dns-proof.com",
            domain_name: "*.gamma.dns-proof.com",
            sans: &[],
            spelled: as_handed_out,
        },
        ProvenRow {
            case: "three names, one of them several labels deep",
            apex: "delta.dns-proof.io",
            domain_name: "delta.dns-proof.io",
            sans: &["a.b.c.delta.dns-proof.io", "*.delta.dns-proof.io"],
            spelled: as_handed_out,
        },
        ProvenRow {
            case: "the record published in another spelling of the same name",
            apex: "epsilon.dns-proof.net",
            domain_name: "shop.epsilon.dns-proof.net",
            sans: &[],
            spelled: shouted_without_the_root_dot,
        },
    ];

    for row in rows {
        // A fresh store per row, so one row's zone or certificate cannot explain
        // another's.
        let (router, engine) = router_with_dns();
        let zone = create_public_zone(&router, row.apex, row.case).await;
        let (arn, records) =
            request_dns_validated(row.case, &router, row.domain_name, row.sans).await;

        // Publishing all but the last record proves nothing: every covered name must be
        // validated.
        for record in &records[..records.len() - 1] {
            submit_change(row.case, &router, &zone, "CREATE", &(row.spelled)(record)).await;
            assert_still_waiting(row.case, &router, &engine, &arn).await;
        }
        let last = (row.spelled)(&records[records.len() - 1]);
        submit_change(row.case, &router, &zone, "CREATE", &last).await;

        // The proof is recorded, and the issuance it owes is materialised by the next
        // call once its stored instant has passed — nothing sleeps for it.
        assert_issuance_owed(row.case, &router, &engine, &arn).await;
        make_validation_due_now(&engine);
        let issued = describe(&router, &arn).await;
        assert_issued(row.case, &issued);

        // A second read reports the same record, and the other reader agrees.
        let again = describe(&router, &arn).await;
        assert_eq!(
            again, issued,
            "case {}: an issued certificate must read the same every time, got {again}",
            row.case
        );
        let listed = arns_in_statuses(&router, &["ISSUED"]).await;
        assert_eq!(
            listed,
            vec![arn.clone()],
            "case {}: expected ListCertificates to report the certificate as ISSUED, got {listed:?}",
            row.case
        );
    }
}

/// How a row of the negative table publishes (or declines to publish) the handed-out
/// records.
enum Publication {
    /// Nothing is published at all.
    Nothing,
    /// Only the FIRST covered name's record is published.
    OnlyTheFirstName,
    /// Every record is published, rewritten by this function first.
    EveryRecord(fn(&Record) -> Record),
    /// Every record is published, unchanged, but in a PRIVATE hosted zone.
    InAPrivateZone,
}

/// One row of the negative table: what it is, the zone apex, the certificate's names
/// and how its records are published.
struct UnprovenRow<'a> {
    case: &'a str,
    apex: &'a str,
    domain_name: &'a str,
    sans: &'a [&'a str],
    publication: Publication,
}

/// Publish the handed-out name with a value that is not the one ACM handed out.
fn with_another_value(record: &Record) -> Record {
    Record {
        name: record.name.clone(),
        rr_type: record.rr_type.clone(),
        value: format!("_not{}", record.value.trim_start_matches('_')),
    }
}

/// Publish the handed-out value under a sibling name ACM asked for nothing at.
fn under_a_sibling_name(record: &Record) -> Record {
    Record {
        name: format!("_sibling.{}", record.name.trim_start_matches('_')),
        rr_type: record.rr_type.clone(),
        value: record.value.clone(),
    }
}

/// Publish the handed-out name and value as a record of another type whose data is
/// also a domain name (`PTR`), so only the TYPE differs.
fn as_another_record_type(record: &Record) -> Record {
    Record {
        name: record.name.clone(),
        rr_type: "PTR".to_string(),
        value: record.value.clone(),
    }
}

/// RULE: while any covered name's handed-out CNAME does not resolve, in the account's
/// public DNS, to the value ACM handed out, the request keeps waiting the documented
/// 72-hour window out — and carries no `FailureReason`, because no modelled reason
/// names an unpublished challenge.
#[tokio::test]
async fn an_unmatched_record_leaves_the_certificate_waiting() {
    let rows = [
        UnprovenRow {
            case: "nothing published",
            apex: "none.dns-proof.net",
            domain_name: "www.none.dns-proof.net",
            sans: &[],
            publication: Publication::Nothing,
        },
        UnprovenRow {
            case: "only one of two covered names published",
            apex: "partial.dns-proof.org",
            domain_name: "partial.dns-proof.org",
            sans: &["api.partial.dns-proof.org"],
            publication: Publication::OnlyTheFirstName,
        },
        UnprovenRow {
            case: "the handed-out name published with another value",
            apex: "value.dns-proof.com",
            domain_name: "www.value.dns-proof.com",
            sans: &[],
            publication: Publication::EveryRecord(with_another_value),
        },
        UnprovenRow {
            case: "the handed-out value published under a sibling name",
            apex: "name.dns-proof.io",
            domain_name: "www.name.dns-proof.io",
            sans: &[],
            publication: Publication::EveryRecord(under_a_sibling_name),
        },
        UnprovenRow {
            case: "the handed-out name and value published as another record type",
            apex: "type.dns-proof.net",
            domain_name: "www.type.dns-proof.net",
            sans: &[],
            publication: Publication::EveryRecord(as_another_record_type),
        },
        UnprovenRow {
            case: "the record published only in a private hosted zone",
            apex: "private.dns-proof.org",
            domain_name: "www.private.dns-proof.org",
            sans: &[],
            publication: Publication::InAPrivateZone,
        },
    ];

    for row in rows {
        let (router, engine) = router_with_dns();
        let zone = match row.publication {
            Publication::InAPrivateZone => create_private_zone(&router, row.apex, row.case).await,
            _ => create_public_zone(&router, row.apex, row.case).await,
        };
        let (arn, records) =
            request_dns_validated(row.case, &router, row.domain_name, row.sans).await;

        match row.publication {
            Publication::Nothing => {}
            Publication::OnlyTheFirstName => {
                submit_change(row.case, &router, &zone, "CREATE", &records[0]).await;
            }
            Publication::EveryRecord(spelled) => {
                for record in &records {
                    submit_change(row.case, &router, &zone, "CREATE", &spelled(record)).await;
                }
            }
            Publication::InAPrivateZone => {
                for record in &records {
                    submit_change(row.case, &router, &zone, "CREATE", record).await;
                }
            }
        }

        assert_still_waiting(row.case, &router, &engine, &arn).await;
        // The other reader agrees, and nothing has been issued.
        let issued = arns_in_statuses(&router, &["ISSUED"]).await;
        assert!(
            issued.is_empty(),
            "case {}: nothing is issued while a name is unproven, got {issued:?}",
            row.case
        );
    }
}

/// RULE: only the certificate whose own challenge is published is proven — the records
/// are per certificate and per name, so one certificate's published CNAME says nothing
/// about another's, nor about a name validated by a method the DNS does not answer.
#[tokio::test]
async fn one_certificate_s_published_record_proves_only_that_certificate() {
    let case = "two certificates for one domain, one record published";
    let apex = "shared.dns-proof.net";
    let domain = "www.shared.dns-proof.net";
    let (router, engine) = router_with_dns();
    let zone = create_public_zone(&router, apex, case).await;

    let (proven, records) = request_dns_validated(case, &router, domain, &[]).await;
    let (other, other_records) = request_dns_validated(case, &router, domain, &[]).await;
    assert_ne!(
        records[0].name, other_records[0].name,
        "case {case}: two requests for one domain get their own challenges, \
         got {records:?} and {other_records:?}"
    );

    // A certificate ACM validates by EMAIL is not proven by a DNS record at all: its
    // evidence is a mailbox, and it is handed no record to publish.
    let mailed = request_certificate(
        &router,
        &json!({ "DomainName": domain, "ValidationMethod": "EMAIL" }),
    )
    .await;

    submit_change(case, &router, &zone, "CREATE", &records[0]).await;
    assert_issuance_owed(case, &router, &engine, &proven).await;
    assert_still_waiting(case, &router, &engine, &other).await;
    assert_still_waiting(case, &router, &engine, &mailed).await;

    make_validation_due_now(&engine);
    assert_issued(case, &describe(&router, &proven).await);
    let listed = arns_in_statuses(&router, &["ISSUED"]).await;
    assert_eq!(
        listed,
        vec![proven.clone()],
        "case {case}: only the certificate whose record was published is ISSUED, got {listed:?}"
    );
}

/// RULE: the proof is READ from the DNS on every pass, never remembered — a record
/// withdrawn before the issuance it owes falls due puts the request back to waiting the
/// documented window out, and publishing it again proves the name again.
#[tokio::test]
async fn a_withdrawn_record_stops_proving_the_name() {
    let case = "the validation record created, deleted and created again";
    let apex = "withdrawn.dns-proof.com";
    let (router, engine) = router_with_dns();
    let zone = create_public_zone(&router, apex, case).await;
    let (arn, records) =
        request_dns_validated(case, &router, "www.withdrawn.dns-proof.com", &[]).await;

    submit_change(case, &router, &zone, "CREATE", &records[0]).await;
    assert_issuance_owed(case, &router, &engine, &arn).await;

    submit_change(case, &router, &zone, "DELETE", &records[0]).await;
    assert_still_waiting(case, &router, &engine, &arn).await;

    submit_change(case, &router, &zone, "CREATE", &records[0]).await;
    assert_issuance_owed(case, &router, &engine, &arn).await;
    make_validation_due_now(&engine);
    assert_issued(case, &describe(&router, &arn).await);
}

/// RULE: a client that publishes the handed-out record and polls `DescribeCertificate`
/// reaches `ISSUED` on the wall clock — the issuance the proof owes falls due a bounded
/// span later, so the poll terminates without the test pushing any stored instant.
#[tokio::test]
async fn a_poll_after_publishing_the_record_reaches_issued() {
    let case = "polled to ISSUED after publishing the record";
    let apex = "polled.dns-proof.org";
    let (router, _engine) = router_with_dns();
    let zone = create_public_zone(&router, apex, case).await;
    let (arn, records) = request_dns_validated(case, &router, "www.polled.dns-proof.org", &[]).await;

    let published_at = Instant::now();
    submit_change(case, &router, &zone, "CREATE", &records[0]).await;

    // A cap far above the emulator's own span, so an expired budget is a real failure
    // rather than a tight-timing artefact.
    let budget = VALIDATION_SETTLE_DELAY * 20;
    let mut polls = 0_u32;
    loop {
        let detail = describe(&router, &arn).await;
        if detail["Status"].as_str() != Some("PENDING_VALIDATION") {
            assert_issued(case, &detail);
            break;
        }
        polls += 1;
        assert!(
            published_at.elapsed() < budget,
            "case {case}: polling for ISSUED gave up after {polls} polls in {:?}: still {detail}",
            published_at.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        polls > 0,
        "case {case}: the pending state must still be observable right after publication"
    );
    assert!(
        published_at.elapsed() >= VALIDATION_SETTLE_DELAY,
        "case {case}: the issuance must not fall due before its span has passed, got {:?}",
        published_at.elapsed()
    );
}
