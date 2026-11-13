// SPDX-License-Identifier: CC0-1.0

//! Which requested certificates ACM refuses UP FRONT, driven over both services' real
//! wires (ACM's `POST /` awsJson1_1 and Route 53's restXml `/2013-04-01/...`) on one
//! shared engine.
//!
//! RULE under test: a public certificate request is refused up front — terminal `FAILED`
//! with `FailureReason` `ADDITIONAL_VERIFICATION_REQUIRED` and nothing issued — only
//! where NOBODY could prove control of a name it covers: the registries reserve that
//! name AND no public hosted zone of the account is authoritative for it. A name the
//! account is itself delegated in the public DNS has a holder, so its request waits in
//! `PENDING_VALIDATION` for the documented 72-hour window and reaches `ISSUED` once the
//! challenge ACM handed out is published — whatever registry suffix the name sits under.
//!
//! The tables span what the rule generalises over: EVERY entry of the two reserved
//! catalogs the service publishes (`RESERVED_TOP_LEVEL_LABELS` and
//! `RESERVED_SECOND_LEVEL_NAMES`, whose source is RFC 2606/6761/6762/7686 and IANA's
//! Example Domains), how the covered name sits against the zone apex (the apex itself, a
//! label under it, several labels deep, a wildcard, two names at once), and each way a
//! zone can fail to delegate the name (no zone at all, a private zone, a zone for a
//! sibling name, a zone BELOW the name).
//!
//! Nothing here sleeps for the emulator's owed span: a row that needs the outcome
//! materialised moves the instant the service itself stored into the past, exactly as
//! the wall clock would have.

use serde_json::{json, Value};
use service_com_amazonaws_acm_certificatemanager::state::{
    FAILURE_REASON_ADDITIONAL_VERIFICATION_REQUIRED, RESERVED_SECOND_LEVEL_NAMES,
    RESERVED_TOP_LEVEL_LABELS, VALIDATION_SETTLE_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{describe, request_certificate};
use crate::certificate_validation_state_machine::{
    arns_in_statuses, make_validation_due_now, pending_due_in,
};
use crate::dns_validation_published_record::{
    assert_issuance_owed, assert_issued, assert_still_waiting, create_private_zone,
    create_public_zone, request_dns_validated, router_with_dns, submit_change,
};

/// The label a test zone's apex is spelled with, under the reserved registry name the
/// row is about. Immaterial to the rule — any label the account holds a zone for would
/// do — but every row needs one.
const ZONE_LABEL: &str = "acct-zone";

/// Every reserved registry name the service publishes: the special-use top-level labels
/// beside the reserved documentation second-level names. Read from the production
/// catalogs rather than written out here, so a catalog that grows is covered with it.
fn reserved_names() -> Vec<&'static str> {
    RESERVED_TOP_LEVEL_LABELS
        .iter()
        .chain(RESERVED_SECOND_LEVEL_NAMES)
        .copied()
        .collect()
}

/// How a row's certificate names sit against the zone apex.
#[derive(Debug, Clone, Copy)]
enum Covered {
    /// The apex itself.
    Apex,
    /// One label under the apex.
    Label,
    /// Several labels under the apex.
    Deep,
    /// A wildcard over the apex, proven through the domain it covers.
    Wildcard,
    /// Two names: the apex and a label under it.
    ApexAndLabel,
}

impl Covered {
    /// The primary `DomainName` and the subject alternative names, for `apex`.
    fn names_of(self, apex: &str) -> (String, Vec<String>) {
        match self {
            Self::Apex => (apex.to_string(), Vec::new()),
            Self::Label => (format!("www.{apex}"), Vec::new()),
            Self::Deep => (format!("a.b.c.{apex}"), Vec::new()),
            Self::Wildcard => (format!("*.{apex}"), Vec::new()),
            Self::ApexAndLabel => (apex.to_string(), vec![format!("api.{apex}")]),
        }
    }
}

/// The name shapes the positive table cycles through, one per reserved catalog entry.
const COVERED_SHAPES: &[Covered] = &[
    Covered::Label,
    Covered::Apex,
    Covered::Wildcard,
    Covered::Deep,
    Covered::ApexAndLabel,
];

/// RULE: a name the account holds a PUBLIC hosted zone for is delegated to it, so its
/// certificate request waits for proof instead of being refused — and the published
/// challenge issues it — however the registries classify the name's suffix.
#[tokio::test]
async fn a_delegated_reserved_name_is_validated_rather_than_refused() {
    for (index, reserved) in reserved_names().into_iter().enumerate() {
        let shape = COVERED_SHAPES[index % COVERED_SHAPES.len()];
        let apex = format!("{ZONE_LABEL}.{reserved}");
        let (domain, sans) = shape.names_of(&apex);
        let san_refs: Vec<&str> = sans.iter().map(String::as_str).collect();
        let case = format!("a zone for {apex} covering {domain} ({shape:?})");

        // A fresh store per row, so one row's zone or certificate cannot explain
        // another's.
        let (router, engine) = router_with_dns();
        let zone = create_public_zone(&router, &apex, &case).await;
        let (arn, records) = request_dns_validated(&case, &router, &domain, &san_refs).await;

        // The request is WAITING on proof, not refused: the outcome the store owes it is
        // the documented 72-hour timeout, which is far beyond the settle span an
        // up-front refusal falls due within.
        assert_still_waiting(&case, &router, &engine, &arn).await;
        let failed_now = arns_in_statuses(&router, &["FAILED"]).await;
        assert!(
            failed_now.is_empty(),
            "case {case}: a delegated name is no up-front failure, got {failed_now:?}"
        );

        for record in &records {
            submit_change(&case, &router, &zone, "CREATE", record).await;
        }

        // The published challenge is the proof, and the issuance it owes is materialised
        // by the next call once the instant the service stored has passed.
        assert_issuance_owed(&case, &router, &engine, &arn).await;
        make_validation_due_now(&engine);

        let issued = describe(&router, &arn).await;
        assert_issued(&case, &issued);
        assert_eq!(
            issued["DomainName"].as_str(),
            Some(domain.as_str()),
            "case {case}: the requested DomainName is echoed verbatim, got {issued}"
        );
        assert_eq!(
            issued["InUseBy"],
            json!([]),
            "case {case}: nothing references a freshly issued certificate, got {issued}"
        );
        let listed = arns_in_statuses(&router, &["ISSUED"]).await;
        assert_eq!(
            listed,
            vec![arn.clone()],
            "case {case}: expected ListCertificates to report the certificate as ISSUED, \
             got {listed:?}"
        );
    }
}

/// How a row of the refusal table leaves the covered name UNDELEGATED.
#[derive(Debug, Clone, Copy)]
enum Delegation {
    /// The account holds no hosted zone at all.
    NoZone,
    /// The account holds a PRIVATE zone for the name's apex, which resolves nowhere the
    /// public DNS can see it.
    PrivateZone,
    /// The account holds a public zone for a SIBLING name, which is authoritative for
    /// nothing the certificate covers.
    PublicZoneForASibling,
    /// The account holds a public zone BELOW the covered name, which the name is a
    /// parent of rather than a child.
    PublicZoneBelowTheName,
}

/// The delegation shapes the refusal table cycles through, one per reserved catalog
/// entry.
const UNDELEGATED_SHAPES: &[Delegation] = &[
    Delegation::NoZone,
    Delegation::PrivateZone,
    Delegation::PublicZoneForASibling,
    Delegation::PublicZoneBelowTheName,
];

/// Assert everything the up-front refusal of `arn` implies, and that it is terminal.
async fn assert_refused_up_front(
    case: &str,
    router: &MultiServiceRouter,
    arn: &str,
    domain: &str,
) -> Value {
    let detail = describe(router, arn).await;
    assert_eq!(
        detail["Status"].as_str(),
        Some("FAILED"),
        "case {case}: a name nobody could prove control of is refused up front, got {detail}"
    );
    assert_eq!(
        detail["FailureReason"].as_str(),
        Some(FAILURE_REASON_ADDITIONAL_VERIFICATION_REQUIRED),
        "case {case}: expected the FailureReason of a request needing manual \
         verification, got {detail}"
    );
    assert_eq!(
        detail["Type"].as_str(),
        Some("AMAZON_ISSUED"),
        "case {case}: a requested certificate stays AMAZON_ISSUED, got {detail}"
    );
    assert_eq!(
        detail["DomainName"].as_str(),
        Some(domain),
        "case {case}: the requested DomainName is echoed verbatim, got {detail}"
    );
    assert!(
        detail["Serial"].is_null(),
        "case {case}: no material was issued, so there is no Serial, got {detail}"
    );
    assert_eq!(
        detail["InUseBy"],
        json!([]),
        "case {case}: nothing references a certificate that never issued, got {detail}"
    );
    for absent in ["IssuedAt", "NotBefore", "NotAfter"] {
        assert!(
            detail[absent].is_null(),
            "case {case}: {absent} cannot exist without issuance, got {detail}"
        );
    }

    // Terminal: reading again neither moves it on nor re-opens the validation.
    let again = describe(router, arn).await;
    assert_eq!(
        again, detail,
        "case {case}: a refused request must read the same every time, got {again}"
    );
    detail
}

/// RULE: a request for a name the registries reserve AND no public hosted zone of the
/// account is authoritative for is refused up front — terminal `FAILED` with
/// `ADDITIONAL_VERIFICATION_REQUIRED`, no issuance and no serial.
#[tokio::test]
async fn an_undelegated_reserved_name_settles_into_failed() {
    for (index, reserved) in reserved_names().into_iter().enumerate() {
        let delegation = UNDELEGATED_SHAPES[index % UNDELEGATED_SHAPES.len()];
        let apex = format!("{ZONE_LABEL}.{reserved}");
        // The zone below the name can only sit below a name the row keeps short, so that
        // row covers the apex itself; every other row covers a label under it.
        let domain = match delegation {
            Delegation::PublicZoneBelowTheName => apex.clone(),
            _ => format!("www.{apex}"),
        };
        let case = format!("{domain} with {delegation:?}");

        let (router, engine) = router_with_dns();
        match delegation {
            Delegation::NoZone => {}
            Delegation::PrivateZone => {
                create_private_zone(&router, &apex, &case).await;
            }
            Delegation::PublicZoneForASibling => {
                create_public_zone(&router, &format!("other-zone.{reserved}"), &case).await;
            }
            Delegation::PublicZoneBelowTheName => {
                create_public_zone(&router, &format!("deeper.{domain}"), &case).await;
            }
        }

        let arn = request_certificate(
            &router,
            &json!({ "DomainName": domain, "ValidationMethod": "DNS" }),
        )
        .await;

        // The refusal is owed by the clock, not applied at once: the span is never zero,
        // so a caller reading straight after the request still sees the pending state.
        let pending = describe(&router, &arn).await;
        assert_eq!(
            pending["Status"].as_str(),
            Some("PENDING_VALIDATION"),
            "case {case}: the refusal span is never zero, got {pending}"
        );
        let due_in = pending_due_in(&engine, &arn)
            .unwrap_or_else(|| panic!("case {case}: the request owed no validation outcome"));
        assert!(
            due_in <= VALIDATION_SETTLE_DELAY,
            "case {case}: expected the up-front refusal to fall due within \
             {VALIDATION_SETTLE_DELAY:?}, got {due_in:?}"
        );

        make_validation_due_now(&engine);
        assert_refused_up_front(&case, &router, &arn, &domain).await;

        let listed = arns_in_statuses(&router, &["FAILED"]).await;
        assert_eq!(
            listed,
            vec![arn.clone()],
            "case {case}: expected ListCertificates to report the certificate as FAILED, \
             got {listed:?}"
        );
        let issued = arns_in_statuses(&router, &["ISSUED"]).await;
        assert!(
            issued.is_empty(),
            "case {case}: a refused request issues nothing, got {issued:?}"
        );
    }
}

/// RULE: every name a certificate covers must be one whose control could be proven, so
/// one undelegated name refuses the whole request however many of its siblings the
/// account is delegated.
#[tokio::test]
async fn one_undelegated_name_refuses_the_whole_request() {
    let delegated_apex = format!("{ZONE_LABEL}.example.org");
    let rows = [
        (
            "the undelegated name as a subject alternative name",
            format!("www.{delegated_apex}"),
            vec![format!("www.{ZONE_LABEL}.invalid")],
        ),
        (
            "the undelegated name as the primary domain",
            format!("www.{ZONE_LABEL}.invalid"),
            vec![format!("www.{delegated_apex}")],
        ),
    ];

    for (case, domain, sans) in rows {
        let (router, engine) = router_with_dns();
        create_public_zone(&router, &delegated_apex, case).await;

        let arn = request_certificate(
            &router,
            &json!({
                "DomainName": domain,
                "SubjectAlternativeNames": sans,
                "ValidationMethod": "DNS",
            }),
        )
        .await;
        make_validation_due_now(&engine);
        let detail = assert_refused_up_front(case, &router, &arn, &domain).await;
        assert_eq!(
            detail["DomainValidationOptions"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1 + sans.len(),
            "case {case}: every covered name reports its validation, got {detail}"
        );
    }
}
