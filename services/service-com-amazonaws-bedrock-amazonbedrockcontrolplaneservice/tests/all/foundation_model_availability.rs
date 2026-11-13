// SPDX-License-Identifier: CC0-1.0

//! What `GetFoundationModelAvailability` answers for a `modelId`, driven end to end over the
//! same `MultiServiceRouter` the emulator boots.
//!
//! # The rule
//!
//! **A read of a foundation model's availability reports the account's standing with a
//! PUBLISHED model** — the catalog row the id resolves to and whether the terms that model is
//! offered under are still outstanding — **and nothing about the request beyond the model it
//! names**: the same identifier is answered alike whatever region the request addresses, a
//! `:`-qualified id names the published model it is a variant of, and every member the
//! response carries is a value its own modelled enum declares.
//!
//! `tests/all/foundation_model_catalogue_resolution.rs` pins the standing itself against the
//! agreement offers it is read off, in one region and by bare model id. These rows pin what
//! makes that standing a fact about the MODEL rather than about one request:
//!
//! * the region the request addresses (five, spanning four partitions' worth of prefixes),
//!   for an offered model and for both classes of identifier that name no offered model;
//! * how the id is spelled: the bare catalog id and the `:`-qualified form `BedrockModelId`
//!   admits, which names a variant of the same published model;
//! * both agreement classes the catalog carries — a model offered under its provider's own
//!   terms and one offered under none.
//!
//! The expected value sets are cited from the model
//! (`smithy_query(query="operation",operation="GetFoundationModelAvailability")`,
//! `query="shape"` on `AgreementStatus`, `AuthorizationStatus`, `EntitlementAvailability` and
//! `RegionAvailability`).

use serde_json::Value;
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{emulator, error_code, label, Emulator, Reply, REGION};

/// The published catalog this service serves.
const CATALOG: &[bedrock::state::FoundationModel] = bedrock::state::FOUNDATION_MODELS;

/// The regions the rows are driven against. The region a request addresses travels in its
/// SigV4 credential scope, so these are five different requests about one model; the
/// published catalog is the same in all of them.
const REGIONS: &[&str] = &[
    REGION,
    "us-east-1",
    "eu-central-1",
    "ap-northeast-1",
    "sa-east-1",
];

/// `AgreementStatus` (`smithy_query(query="shape",shape="AgreementStatus")`), four values.
const AGREEMENT_STATUS: &[&str] = &["AVAILABLE", "PENDING", "NOT_AVAILABLE", "ERROR"];

/// `AuthorizationStatus` (`query="shape"`, shape `AuthorizationStatus`), two values.
const AUTHORIZATION_STATUS: &[&str] = &["AUTHORIZED", "NOT_AUTHORIZED"];

/// `EntitlementAvailability` and `RegionAvailability` (`query="shape"` on each) declare the
/// same two values.
const AVAILABILITY: &[&str] = &["AVAILABLE", "NOT_AVAILABLE"];

/// What a read of one identifier must answer.
#[derive(Clone, Copy, Debug)]
enum Expect {
    /// The published row the identifier names: the standing is read off it.
    Offered(&'static bedrock::state::FoundationModel),
    /// A model this service recognises and offers no longer: a resource that is gone.
    PastEndOfLife,
    /// A bare id naming no model at all: a value this service cannot read as a model.
    Unreadable,
}

/// One read of `model_id`'s availability, signed for `region`.
async fn availability(emu: &Emulator, region: &str, model_id: &str) -> Reply {
    emu.call_in_region(
        region,
        "GET",
        &format!("/foundation-model-availability/{}", label(model_id)),
        None,
    )
    .await
}

/// The wire error code a refusal carries.
fn code(reply: &Reply) -> &str {
    reply.error.as_deref().map(error_code).unwrap_or("none")
}

/// The `message` a refusal carries.
fn message(reply: &Reply) -> String {
    reply
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A catalogued model whose terms are still outstanding (`outstanding`) or settled
/// (`!outstanding`), whose id carries room for one more `:` qualifier — `BedrockModelId`
/// admits at most two.
fn model_with_agreement(outstanding: bool) -> &'static bedrock::state::FoundationModel {
    CATALOG
        .iter()
        .find(|model| {
            bedrock::state::model_agreement_outstanding(model) == outstanding
                && model.model_id.matches(':').count() < 2
        })
        .expect("the catalog must carry a qualifiable model of each agreement class")
}

/// The rows every table below walks: both agreement classes, in both spellings
/// `BedrockModelId` admits, beside the two classes of identifier that name no offered model.
fn rows() -> Vec<(String, String, Expect)> {
    let mut rows = Vec::new();
    for outstanding in [true, false] {
        let model = model_with_agreement(outstanding);
        let standing = if outstanding { "outstanding" } else { "settled" };
        rows.push((
            format!("{} ({standing}) by its bare id", model.model_id),
            model.model_id.to_string(),
            Expect::Offered(model),
        ));
        // A `:`-separated qualifier names a variant of the same published model, so the
        // standing is the model's.
        rows.push((
            format!("{} ({standing}) with a version qualifier", model.model_id),
            format!("{}:24k", model.model_id),
            Expect::Offered(model),
        ));
    }
    // Models past their end of life, in both spellings the retired ids come in: one with no
    // qualifier and one carrying one.
    for retired in bedrock::state::RETIRED_FOUNDATION_MODELS {
        if retired.contains(':') || *retired == bedrock::state::RETIRED_FOUNDATION_MODELS[0] {
            rows.push((
                format!("the retired model {retired}"),
                (*retired).to_string(),
                Expect::PastEndOfLife,
            ));
        }
    }
    for unreadable in ["nonesuch.model-v1", "vendor.absent-model-v9:0"] {
        rows.push((
            format!("the unresolvable id {unreadable}"),
            unreadable.to_string(),
            Expect::Unreadable,
        ));
    }
    rows
}

/// The three members that describe ONE standing, as the published row decides them.
fn expected_standing(
    model: &bedrock::state::FoundationModel,
) -> (&'static str, &'static str, &'static str) {
    if bedrock::state::model_agreement_outstanding(model) {
        (
            bedrock::state::AGREEMENT_STATUS_NOT_AVAILABLE,
            bedrock::state::AVAILABILITY_NOT_AVAILABLE,
            bedrock::state::AUTHORIZATION_STATUS_NOT_AUTHORIZED,
        )
    } else {
        (
            bedrock::state::AGREEMENT_STATUS_AVAILABLE,
            bedrock::state::AVAILABILITY_AVAILABLE,
            bedrock::state::AUTHORIZATION_STATUS_AUTHORIZED,
        )
    }
}

/// The standing a reply reports, as the triple `expected_standing` states.
fn reported_standing(reply: &Reply) -> (String, String, String) {
    (
        reply
            .get("agreementAvailability")
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("none")
            .to_string(),
        reply
            .get("entitlementAvailability")
            .as_str()
            .unwrap_or("none")
            .to_string(),
        reply
            .get("authorizationStatus")
            .as_str()
            .unwrap_or("none")
            .to_string(),
    )
}

#[tokio::test]
async fn availability_answers_one_identifier_alike_in_every_region() {
    let emu = emulator();
    for (case, identifier, expect) in rows() {
        let mut first: Option<(u16, Value)> = None;
        for region in REGIONS {
            let reply = availability(&emu, region, &identifier).await;
            match expect {
                Expect::Offered(model) => {
                    assert_eq!(
                        reply.status, 200,
                        "case {case:?} in {region}: an offered model must be answered 200, got \
                         {} {:?}",
                        reply.status, reply.body
                    );
                    assert_eq!(
                        reply.text("modelId"),
                        identifier,
                        "case {case:?} in {region}: the response must name the model the request \
                         asked about, got {:?}",
                        reply.body
                    );
                    let expected = expected_standing(model);
                    let actual = reported_standing(&reply);
                    assert_eq!(
                        (actual.0.as_str(), actual.1.as_str(), actual.2.as_str()),
                        expected,
                        "case {case:?} in {region}: the agreement, the entitlement and the \
                         authorization follow the model's own standing, got {:?}",
                        reply.body
                    );
                    // The published catalog does not vary by region, so a model it carries is
                    // offered in the region the request addressed.
                    assert_eq!(
                        reply.text("regionAvailability"),
                        bedrock::state::AVAILABILITY_AVAILABLE,
                        "case {case:?} in {region}: a catalogued model is offered in the region \
                         the request addressed, got {:?}",
                        reply.body
                    );
                }
                Expect::PastEndOfLife => {
                    assert_eq!(
                        (reply.status, code(&reply)),
                        (404, "ResourceNotFoundException"),
                        "case {case:?} in {region}: a model past its end of life has no standing \
                         to report, got {} {} {:?}",
                        reply.status,
                        code(&reply),
                        reply.body
                    );
                    assert_eq!(
                        message(&reply),
                        bedrock::state::FOUNDATION_MODEL_END_OF_LIFE_MESSAGE,
                        "case {case:?} in {region}: the refusal must state the end-of-life \
                         reason, got {:?}",
                        reply.body
                    );
                }
                Expect::Unreadable => {
                    assert_eq!(
                        (reply.status, code(&reply)),
                        (400, "ValidationException"),
                        "case {case:?} in {region}: a bare id naming no model is an input fault, \
                         got {} {} {:?}",
                        reply.status,
                        code(&reply),
                        reply.body
                    );
                    assert_eq!(
                        message(&reply),
                        bedrock::state::INVALID_MODEL_IDENTIFIER_MESSAGE,
                        "case {case:?} in {region}: the refusal must be the service's fixed \
                         sentence, got {:?}",
                        reply.body
                    );
                }
            }
            // The whole answer — not only the members asserted above — is a fact about the
            // model rather than about the region the request addressed.
            match first {
                None => first = Some((reply.status, reply.body.clone())),
                Some((status, ref body)) => assert_eq!(
                    (reply.status, &reply.body),
                    (status, body),
                    "case {case:?}: {region} must answer exactly as {} did, got {:?}",
                    REGIONS[0],
                    reply.body
                ),
            }
        }
    }
}

#[tokio::test]
async fn a_qualified_model_id_reports_the_standing_of_the_model_it_qualifies() {
    let emu = emulator();
    // Qualifiers that name a variant of the same published model: one and two of them, the
    // second spelling the `…:0:8k` form AWS writes into a resource ARN.
    for outstanding in [true, false] {
        let model = model_with_agreement(outstanding);
        let bare = availability(&emu, REGION, model.model_id).await;
        assert_eq!(
            bare.status, 200,
            "case {}: the bare id must be answered 200, got {} {:?}",
            model.model_id, bare.status, bare.body
        );
        let qualifiers: &[&str] = if model.model_id.contains(':') {
            &["8k"]
        } else {
            &["0", "0:8k"]
        };
        for qualifier in qualifiers {
            let identifier = format!("{}:{qualifier}", model.model_id);
            let reply = availability(&emu, REGION, &identifier).await;
            assert_eq!(
                reply.status, 200,
                "case {identifier}: a qualified variant names the published model, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reported_standing(&reply),
                reported_standing(&bare),
                "case {identifier}: a variant's standing is the model's, got {:?} against {:?}",
                reply.body,
                bare.body
            );
            assert_eq!(
                reply.text("modelId"),
                identifier,
                "case {identifier}: the response must name the id the request carried, got {:?}",
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn every_availability_member_carries_a_value_its_enum_declares() {
    // The constants this service answers with are values the model declares, so a read can
    // never report a value outside its member's enum.
    for (name, value, declared) in [
        (
            "AgreementStatus",
            bedrock::state::AGREEMENT_STATUS_AVAILABLE,
            AGREEMENT_STATUS,
        ),
        (
            "AgreementStatus",
            bedrock::state::AGREEMENT_STATUS_NOT_AVAILABLE,
            AGREEMENT_STATUS,
        ),
        (
            "AuthorizationStatus",
            bedrock::state::AUTHORIZATION_STATUS_AUTHORIZED,
            AUTHORIZATION_STATUS,
        ),
        (
            "AuthorizationStatus",
            bedrock::state::AUTHORIZATION_STATUS_NOT_AUTHORIZED,
            AUTHORIZATION_STATUS,
        ),
        (
            "EntitlementAvailability",
            bedrock::state::AVAILABILITY_AVAILABLE,
            AVAILABILITY,
        ),
        (
            "EntitlementAvailability",
            bedrock::state::AVAILABILITY_NOT_AVAILABLE,
            AVAILABILITY,
        ),
    ] {
        assert!(
            declared.contains(&value),
            "case {name}: {value:?} must be one of {declared:?}"
        );
    }

    let emu = emulator();
    for model in CATALOG {
        let case = model.model_id;
        let reply = availability(&emu, REGION, case).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: a catalogued model must be answered 200, got {} {:?}",
            reply.status, reply.body
        );
        let standing = reported_standing(&reply);
        for (member, value, declared) in [
            ("agreementAvailability.status", standing.0, AGREEMENT_STATUS),
            ("entitlementAvailability", standing.1, AVAILABILITY),
            ("authorizationStatus", standing.2, AUTHORIZATION_STATUS),
            (
                "regionAvailability",
                reply.text("regionAvailability"),
                AVAILABILITY,
            ),
        ] {
            assert!(
                declared.contains(&value.as_str()),
                "case {case}: {member} must be one of {declared:?}, got {value:?}"
            );
        }
    }
}
