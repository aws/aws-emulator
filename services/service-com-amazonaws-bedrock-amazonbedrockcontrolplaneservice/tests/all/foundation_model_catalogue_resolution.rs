// SPDX-License-Identifier: CC0-1.0

//! What a `modelId` naming a BASE model resolves to, across every surface of this service
//! that takes one, end to end over the same `MultiServiceRouter` the emulator boots.
//!
//! # The rules
//!
//! 1. **An identifier is resolved against the models the service KNOWS, and a model that
//!    cannot back a request is refused for the reason it cannot** — never reported as
//!    naming no model. Three classes:
//!    * a model the published catalog OFFERS backs the request (or is refused by that
//!      operation's own eligibility rule);
//!    * a model the service recognises and offers no longer (past its end of life) names a
//!      model all the same: a read of the catalog reports it gone (404 with the end-of-life
//!      reason), a Provisioned Throughput purchase is refused as not supported (400), and a
//!      batch submission goes on to be judged on what it asks for;
//!    * an identifier naming no model at all is a missing ARN (404) when it is a
//!      `foundation-model` ARN and an unreadable identifier (400 `ValidationException`
//!      carrying the service's fixed sentence) when it is a bare model id.
//! 2. **A no-commitment Provisioned Throughput purchase is offered only over the base
//!    models AWS publishes one for** (`commitmentDuration`'s own documentation sends a
//!    caller to "Supported Regions and models for Provisioned Throughput" for "which base
//!    models support no commitment"), so over any other model that purchase is refused with
//!    the not-supported sentence rather than drawn against the account's no-commitment
//!    model-unit quota.
//! 3. **A catalogued model's lifecycle is reported as the instants AWS publishes for it**:
//!    its launch instant always, and both ends of the legacy window for a deprecated model
//!    and for no other — each in the RFC 3339 wire form its `@timestampFormat("date-time")`
//!    declares, ordered launch → legacy → end of life.
//! 4. **`GetFoundationModelAvailability` reports the account's standing with a published
//!    model**, read off the same agreement offers `ListFoundationModelAgreementOffers`
//!    reports: while the terms a third-party model is offered under are unaccepted there is
//!    no agreement, no entitlement and no authorization, and a model AWS publishes itself
//!    needs none — there being no agreement of such a model, that listing has no offer of
//!    it to report and refuses its id instead; the region a catalogued model is offered in
//!    is always available, and the read writes nothing.
//!
//! # What the tables generalise over
//!
//! The rows walk the resolution CLASSES above rather than one recorded model id: offered
//! models taken from the published catalog itself (by bare id, by `foundation-model` ARN and
//! in the qualified form `BedrockModelId` admits), models past their end of life from three
//! different providers in both spellings, and identifiers that name nothing in both
//! spellings. The expectations come from the model
//! (`smithy_query(query="operation",operation="GetFoundationModel,GetFoundationModelAvailability,CreateProvisionedModelThroughput,CreateModelInvocationJob")`,
//! `query="shape"` on `FoundationModelLifecycle` and `AgreementAvailability`).

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{
    date_time_seconds, emulator, error_code, foundation_model_arn, label, Emulator, Reply, REGION,
};

/// The published catalog this service serves through `ListFoundationModels`.
const CATALOG: &[bedrock::state::FoundationModel] = bedrock::state::FOUNDATION_MODELS;

/// `CommitmentDuration` — the term a purchase that buys one names
/// (`smithy_query(query="shape",shape="CommitmentDuration")`).
const COMMITMENT: &str = "OneMonth";

/// What a `modelId` names, which is what decides how a request carrying it is answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Named {
    /// A model the published catalog offers.
    Offered,
    /// A model the service recognises and no longer offers.
    Retired,
    /// A `foundation-model` ARN naming no model.
    MissingArn,
    /// A bare model id naming no model.
    UnreadableId,
}

/// The rows every table below walks: one identifier per resolution class, spanning both
/// spellings `ModelIdentifier` admits and several providers.
fn rows() -> Vec<(String, String, Named)> {
    let offered = CATALOG
        .iter()
        .find(|model| bedrock::state::supports_provisioned_throughput(model))
        .expect("the catalog must offer a model with Provisioned Throughput");
    let mut rows = vec![
        (
            format!("the offered model {} by its bare id", offered.model_id),
            offered.model_id.to_string(),
            Named::Offered,
        ),
        (
            format!("the offered model {} by its ARN", offered.model_id),
            foundation_model_arn(REGION, offered.model_id),
            Named::Offered,
        ),
        (
            format!("the offered model {} with a version qualifier", offered.model_id),
            format!("{}:24k", offered.model_id),
            Named::Offered,
        ),
        (
            "a bare id no model of this service answers to".to_string(),
            "nonesuch.model-v1".to_string(),
            Named::UnreadableId,
        ),
        (
            "a bare id of a provider the catalog carries, naming no model".to_string(),
            "amazon.nova-sonic-v9:0".to_string(),
            Named::UnreadableId,
        ),
        (
            "the ARN of a model no catalog row carries".to_string(),
            foundation_model_arn(REGION, "nonesuch.model-v1"),
            Named::MissingArn,
        ),
        (
            "an ARN of another region, of a model no catalog row carries".to_string(),
            foundation_model_arn("eu-central-1", "nonesuch.model-v1"),
            Named::MissingArn,
        ),
    ];
    // Models past their end of life: the service knows them, so they name a model. Three
    // providers, in both spellings, and none of them the one the issue's check named.
    for model_id in ["anthropic.claude-v2", "ai21.j2-ultra-v1", "meta.llama2-70b-chat-v1"] {
        assert!(
            bedrock::state::RETIRED_FOUNDATION_MODELS.contains(&model_id),
            "case {model_id}: this row must name a model the service recognises as retired"
        );
        rows.push((
            format!("the retired model {model_id} by its bare id"),
            model_id.to_string(),
            Named::Retired,
        ));
        rows.push((
            format!("the retired model {model_id} by its ARN"),
            foundation_model_arn(REGION, model_id),
            Named::Retired,
        ));
    }
    rows
}

/// The `message` a refusal carries.
fn message(case: &str, reply: &Reply) -> String {
    reply
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: a refusal must carry a message, got {:?}", reply.body))
        .to_string()
}

/// The wire error code a refusal carries.
fn code(reply: &Reply) -> &str {
    reply.error.as_deref().map(error_code).unwrap_or("none")
}

// ---------------------------------------------------------------------------------------
// 1. The read of the catalog.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_read_of_the_catalog_answers_each_resolution_class_on_its_own_terms() {
    let emu = emulator();
    for (case, identifier, named) in rows() {
        let reply = emu
            .call(
                "GET",
                &format!("/foundation-models/{}", label(&identifier)),
                None,
            )
            .await;
        match named {
            Named::Offered => {
                assert_eq!(
                    reply.status, 200,
                    "case {case}: an offered model must read back 200, got {} {:?}",
                    reply.status, reply.body
                );
                let reported = reply.get("modelDetails");
                let model_id = reported
                    .get("modelId")
                    .and_then(Value::as_str)
                    .unwrap_or("none");
                assert!(
                    CATALOG.iter().any(|model| model.model_id == model_id),
                    "case {case}: the details must be a published catalog row, got \
                     {model_id:?}"
                );
            }
            Named::Retired => {
                assert_eq!(
                    (reply.status, code(&reply)),
                    (404, "ResourceNotFoundException"),
                    "case {case}: a model past its end of life is gone rather than unreadable, \
                     got {} {} {:?}",
                    reply.status,
                    code(&reply),
                    reply.body
                );
                assert_eq!(
                    message(&case, &reply),
                    bedrock::state::FOUNDATION_MODEL_END_OF_LIFE_MESSAGE,
                    "case {case}: the refusal must state the end-of-life reason, got {:?}",
                    reply.body
                );
            }
            Named::MissingArn => {
                assert_eq!(
                    (reply.status, code(&reply)),
                    (404, "ResourceNotFoundException"),
                    "case {case}: an ARN naming no model is a missing resource, got {} {} {:?}",
                    reply.status,
                    code(&reply),
                    reply.body
                );
                assert_eq!(
                    message(&case, &reply),
                    bedrock::state::foundation_model_not_found(&identifier),
                    "case {case}: the refusal must name the identifier it could not resolve, \
                     got {:?}",
                    reply.body
                );
            }
            Named::UnreadableId => {
                assert_eq!(
                    (reply.status, code(&reply)),
                    (400, "ValidationException"),
                    "case {case}: a bare id naming no model is an input fault, got {} {} {:?}",
                    reply.status,
                    code(&reply),
                    reply.body
                );
                assert_eq!(
                    message(&case, &reply),
                    bedrock::state::INVALID_MODEL_IDENTIFIER_MESSAGE,
                    "case {case}: the refusal must be the service's fixed sentence, got {:?}",
                    reply.body
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// 1b. The Provisioned Throughput purchase.
// ---------------------------------------------------------------------------------------

async fn buy(emu: &Emulator, name: &str, model_id: &str, commitment: Option<&str>) -> Reply {
    let mut body = json!({
        "provisionedModelName": name,
        "modelId": model_id,
        "modelUnits": 1,
    });
    if let Some(commitment) = commitment {
        body["commitmentDuration"] = json!(commitment);
    }
    emu.call("POST", "/provisioned-model-throughput", Some(body))
        .await
}

#[tokio::test]
async fn a_provisioned_throughput_purchase_refuses_a_model_that_exists_as_not_supported() {
    let emu = emulator();
    for (index, (case, identifier, named)) in rows().into_iter().enumerate() {
        let reply = buy(
            &emu,
            &format!("pt-resolution-{index}"),
            &identifier,
            Some(COMMITMENT),
        )
        .await;
        match named {
            Named::Offered => assert_eq!(
                reply.status, 201,
                "case {case}: a model offered with Provisioned Throughput must be bought, got \
                 {} {:?}",
                reply.status, reply.body
            ),
            // A model that EXISTS and is offered under no Provisioned Throughput: the
            // purchase is refused, the model is not reported missing.
            Named::Retired => {
                assert_eq!(
                    (reply.status, code(&reply)),
                    (400, "ValidationException"),
                    "case {case}: a model the service offers no longer is not a missing \
                     resource, got {} {} {:?}",
                    reply.status,
                    code(&reply),
                    reply.body
                );
                assert_eq!(
                    message(&case, &reply),
                    bedrock::state::MODEL_NOT_SUPPORTED_FOR_PROVISIONED_THROUGHPUT,
                    "case {case}: the refusal must be the not-supported sentence, got {:?}",
                    reply.body
                );
            }
            // An identifier that names no model at all is refused as the spelling it IS,
            // exactly as the read of the catalogue refuses it: the ARN spelling reports a
            // missing resource, the bare id is an input fault.
            Named::MissingArn => {
                assert_eq!(
                    (reply.status, code(&reply)),
                    (404, "ResourceNotFoundException"),
                    "case {case}: an ARN naming no model has no resource to provision, got {} \
                     {} {:?}",
                    reply.status,
                    code(&reply),
                    reply.body
                );
                assert_eq!(
                    message(&case, &reply),
                    bedrock::state::foundation_model_not_found(&identifier),
                    "case {case}: the refusal must name the identifier it could not resolve, \
                     got {:?}",
                    reply.body
                );
            }
            Named::UnreadableId => {
                assert_eq!(
                    (reply.status, code(&reply)),
                    (400, "ValidationException"),
                    "case {case}: a bare id naming no model is an input fault, got {} {} {:?}",
                    reply.status,
                    code(&reply),
                    reply.body
                );
                assert_eq!(
                    message(&case, &reply),
                    bedrock::state::INVALID_MODEL_IDENTIFIER_MESSAGE,
                    "case {case}: the refusal must be the service's fixed sentence, got {:?}",
                    reply.body
                );
            }
        }
    }

    // And a model the catalog carries but offers ON DEMAND ONLY is refused the same way a
    // retired one is: it exists, and no Provisioned Throughput is offered over it.
    let on_demand_only: Vec<&str> = CATALOG
        .iter()
        .filter(|model| !bedrock::state::supports_provisioned_throughput(model))
        .map(|model| model.model_id)
        .collect();
    assert!(
        !on_demand_only.is_empty(),
        "the catalog must carry a model offered on demand only, or this table asserts nothing"
    );
    for model_id in on_demand_only {
        let reply = buy(&emu, "pt-on-demand-only", model_id, Some(COMMITMENT)).await;
        assert_eq!(
            (reply.status, code(&reply)),
            (400, "ValidationException"),
            "case {model_id}: a model offered on demand only cannot back a purchase, got {} {} \
             {:?}",
            reply.status,
            code(&reply),
            reply.body
        );
        assert_eq!(
            message(model_id, &reply),
            bedrock::state::MODEL_NOT_SUPPORTED_FOR_PROVISIONED_THROUGHPUT,
            "case {model_id}: the refusal must be the not-supported sentence, got {:?}",
            reply.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 2. The no-commitment purchase.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_no_commitment_purchase_is_offered_only_over_the_models_published_with_one() {
    let emu = emulator();

    // Every model the catalog offers with Provisioned Throughput, split by whether a
    // no-commitment purchase is offered over it at all.
    let provisionable: Vec<&bedrock::state::FoundationModel> = CATALOG
        .iter()
        .filter(|model| bedrock::state::supports_provisioned_throughput(model))
        .collect();
    let (with_no_commitment, term_only): (
        Vec<&&bedrock::state::FoundationModel>,
        Vec<&&bedrock::state::FoundationModel>,
    ) = provisionable
        .iter()
        .partition(|model| bedrock::state::supports_no_commitment_provisioned_throughput(model));
    assert!(
        !with_no_commitment.is_empty() && !term_only.is_empty(),
        "both sides of the rule must have a model in the catalog, got {} with and {} without",
        with_no_commitment.len(),
        term_only.len()
    );

    // A model AWS publishes a no-commitment purchase for reaches the account's
    // no-commitment model-unit quota, which is granted at zero.
    for model in &with_no_commitment {
        let case = model.model_id;
        let reply = buy(&emu, "pt-no-commitment", case, None).await;
        assert_eq!(
            (reply.status, code(&reply)),
            (400, "ServiceQuotaExceededException"),
            "case {case}: the purchase must be refused for want of granted model units, got {} \
             {} {:?}",
            reply.status,
            code(&reply),
            reply.body
        );
        assert_eq!(
            message(case, &reply),
            bedrock::state::no_commitment_model_units_message(model.model_id, REGION),
            "case {case}: the refusal must name the model units the account lacks, got {:?}",
            reply.body
        );
    }

    // A model offered only UNDER a term has no such purchase to make.
    for model in &term_only {
        let case = model.model_id;
        let reply = buy(&emu, "pt-term-only", case, None).await;
        assert_eq!(
            (reply.status, code(&reply)),
            (400, "ValidationException"),
            "case {case}: a model offered only with a commitment cannot back a no-commitment \
             purchase, got {} {} {:?}",
            reply.status,
            code(&reply),
            reply.body
        );
        assert_eq!(
            message(case, &reply),
            bedrock::state::MODEL_NOT_SUPPORTED_FOR_PROVISIONED_THROUGHPUT,
            "case {case}: the refusal must be the not-supported sentence, got {:?}",
            reply.body
        );
        // The same model, with a term, is bought: the refusal is about the purchase asked
        // for and not about the model.
        let committed = buy(
            &emu,
            &format!("pt-term-{}", case.replace(['.', ':'], "-")),
            case,
            Some(COMMITMENT),
        )
        .await;
        assert_eq!(
            committed.status, 201,
            "case {case}: the same model under a term must be bought, got {} {:?}",
            committed.status, committed.body
        );
    }
}

// ---------------------------------------------------------------------------------------
// 1c. The batch inference submission.
// ---------------------------------------------------------------------------------------

/// A `CreateModelInvocationJob` body carrying only the members the model marks required.
fn batch_body(name: &str, model_id: &str) -> Value {
    json!({
        "jobName": name,
        "roleArn": "arn:aws:iam::000000000000:role/bedrock-batch-inference",
        "modelId": model_id,
        "inputDataConfig": {"s3InputDataConfig": {"s3Uri": "s3://batch-resolution-in/prompts.jsonl"}},
        "outputDataConfig": {"s3OutputDataConfig": {"s3Uri": "s3://batch-resolution-out/"}},
    })
}

#[tokio::test]
async fn a_batch_submission_naming_a_model_the_service_knows_is_not_reported_absent() {
    let emu = emulator();
    for (index, (case, identifier, named)) in rows().into_iter().enumerate() {
        let name = format!("batch-resolution-{index}");
        let reply = emu
            .call(
                "POST",
                "/model-invocation-job",
                Some(batch_body(&name, &identifier)),
            )
            .await;
        match named {
            // A model the service knows — offered, or recognised and offered no longer —
            // names a model, so identification is not where the submission is refused.
            Named::Offered | Named::Retired => {
                assert_eq!(
                    reply.status, 200,
                    "case {case}: a modelId naming a model this service knows must get past \
                     identification, got {} {:?}",
                    reply.status, reply.body
                );
                // Round trip: the submission the create stored reads back under the ARN it
                // minted, carrying the identifier the request named.
                let arn = reply.text("jobArn");
                let read = emu
                    .call(
                        "GET",
                        &format!("/model-invocation-job/{}", label(&arn)),
                        None,
                    )
                    .await;
                assert_eq!(
                    read.status, 200,
                    "case {case}: the stored job must read back, got {} {:?}",
                    read.status, read.body
                );
                assert_eq!(
                    read.text("modelId"),
                    identifier,
                    "case {case}: the read must report the modelId the submission carried, got \
                     {:?}",
                    read.body
                );
            }
            // An identifier that names no model at all is refused as the spelling it IS,
            // exactly as the read of the catalogue refuses it.
            Named::MissingArn => {
                assert_eq!(
                    (reply.status, code(&reply)),
                    (404, "ResourceNotFoundException"),
                    "case {case}: an ARN naming no model has nothing to invoke, got {} {} \
                     {:?}",
                    reply.status,
                    code(&reply),
                    reply.body
                );
                assert_eq!(
                    message(&case, &reply),
                    bedrock::state::foundation_model_not_found(&identifier),
                    "case {case}: the refusal must name the identifier it could not resolve, \
                     got {:?}",
                    reply.body
                );
            }
            Named::UnreadableId => {
                assert_eq!(
                    (reply.status, code(&reply)),
                    (400, "ValidationException"),
                    "case {case}: a bare id naming no model is an input fault, got {} {} {:?}",
                    reply.status,
                    code(&reply),
                    reply.body
                );
                assert_eq!(
                    message(&case, &reply),
                    bedrock::state::INVALID_MODEL_IDENTIFIER_MESSAGE,
                    "case {case}: the refusal must be the service's fixed sentence, got {:?}",
                    reply.body
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// 3. The lifecycle instants.
// ---------------------------------------------------------------------------------------

/// `FoundationModelLifecycleStatus` — the status of a deprecated model
/// (`crate::ops::types::FOUNDATION_MODEL_LIFECYCLE_STATUS_VALUES`).
const LEGACY: &str = "LEGACY";

#[tokio::test]
async fn every_catalogued_model_reports_the_lifecycle_instants_published_for_it() {
    let emu = emulator();
    for model in CATALOG {
        let case = model.model_id;
        let reply = emu
            .call("GET", &format!("/foundation-models/{}", label(case)), None)
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case}: an offered model must read back 200, got {} {:?}",
            reply.status, reply.body
        );
        let lifecycle = reply.get("modelDetails");
        let lifecycle = lifecycle.get("modelLifecycle").unwrap_or_else(|| {
            panic!("case {case}: the details must carry modelLifecycle, got {:?}", reply.body)
        });
        assert_eq!(
            lifecycle.get("status").and_then(Value::as_str),
            Some(model.lifecycle_status),
            "case {case}: the status must be the row's, got {lifecycle:?}"
        );

        // Every instant is the RFC 3339 wire form its `@timestampFormat("date-time")`
        // declares — never the epoch number the protocol default would have written.
        let instant = |member: &str| -> Option<f64> {
            lifecycle.get(member).map(|value| {
                let text = value.as_str().unwrap_or_else(|| {
                    panic!(
                        "case {case}: {member} is modelled date-time, so it must be an RFC 3339 \
                         string, got {value}"
                    )
                });
                date_time_seconds(case, text)
            })
        };

        let launched = instant("startOfLifeTime").unwrap_or_else(|| {
            panic!("case {case}: an offered model has launched, so it must report startOfLifeTime, got {lifecycle:?}")
        });
        match model.legacy_window {
            // A deprecated model reports both ends of its legacy window, after its launch.
            Some(window) => {
                assert_eq!(
                    model.lifecycle_status, LEGACY,
                    "case {case}: only a deprecated model has a legacy window"
                );
                let entered = instant("legacyTime").unwrap_or_else(|| {
                    panic!("case {case}: a deprecated model must report legacyTime, got {lifecycle:?}")
                });
                let ends = instant("endOfLifeTime").unwrap_or_else(|| {
                    panic!("case {case}: a deprecated model must report endOfLifeTime, got {lifecycle:?}")
                });
                assert!(
                    launched < entered && entered < ends,
                    "case {case}: the window must run launch -> legacy -> end of life, got \
                     {launched} {entered} {ends}"
                );
                assert_eq!(
                    entered,
                    date_time_seconds(case, window.entered_at),
                    "case {case}: legacyTime must be the instant published for the row"
                );
                assert_eq!(
                    ends,
                    date_time_seconds(case, window.end_of_life_at),
                    "case {case}: endOfLifeTime must be the instant published for the row"
                );
            }
            // A model that is still available has entered no legacy state and is scheduled
            // for no end of life, so neither instant is invented for it.
            None => {
                assert_ne!(
                    model.lifecycle_status, LEGACY,
                    "case {case}: a deprecated model must carry a legacy window"
                );
                for member in ["legacyTime", "endOfLifeTime"] {
                    assert!(
                        lifecycle.get(member).is_none(),
                        "case {case}: an available model publishes no {member}, got {lifecycle:?}"
                    );
                }
            }
        }
        // Nothing publishes a public extended access instant per model.
        assert!(
            lifecycle.get("publicExtendedAccessTime").is_none(),
            "case {case}: no public extended access instant is published, got {lifecycle:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 4. The account's standing with a published model.
// ---------------------------------------------------------------------------------------

async fn availability(emu: &Emulator, model_id: &str) -> Reply {
    emu.call(
        "GET",
        &format!("/foundation-model-availability/{}", label(model_id)),
        None,
    )
    .await
}

/// The offers `ListFoundationModelAgreementOffers` reports for a catalogued model.
///
/// EMPTY for a model AWS publishes itself: there is no agreement of such a model for an
/// offer to be an offer of, so that operation refuses its id as one naming no
/// agreement-bearing model rather than answering with an empty list. Which answer each
/// model gets is decided here from the catalog itself, so neither branch is a status this
/// helper merely tolerates.
async fn offers(emu: &Emulator, model_id: &str) -> Vec<Value> {
    let reply = emu
        .call(
            "GET",
            &format!("/list-foundation-model-agreement-offers/{}", label(model_id)),
            None,
        )
        .await;
    let offered_under_agreement = bedrock::state::catalogued_model(model_id)
        .is_some_and(bedrock::state::model_offered_under_agreement);
    if !offered_under_agreement {
        assert_eq!(
            (reply.status, code(&reply), message(model_id, &reply).as_str()),
            (
                400,
                "ValidationException",
                bedrock::state::INVALID_MODEL_IDENTIFIER_MESSAGE
            ),
            "case {model_id}: a model offered under no agreement must be refused as an \
             identifier naming no agreement-bearing model, got {} {} {:?}",
            reply.status,
            code(&reply),
            reply.body
        );
        return Vec::new();
    }
    assert_eq!(
        reply.status, 200,
        "case {model_id}: the offers of a model offered under an agreement must be served, got \
         {} {:?}",
        reply.status, reply.body
    );
    reply.get("offers").as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn availability_reports_the_standing_the_agreement_offers_describe() {
    let emu = emulator();
    let mut outstanding = 0usize;
    let mut settled = 0usize;
    for model in CATALOG {
        let case = model.model_id;
        let reply = availability(&emu, case).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: a catalogued model must be answered 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.text("modelId"),
            case,
            "case {case}: the response must name the model it is about, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.text("regionAvailability"),
            bedrock::state::AVAILABILITY_AVAILABLE,
            "case {case}: a catalogued model is offered in the region the request addressed, \
             got {:?}",
            reply.body
        );

        // An OUTSTANDING offer is what says the terms the model is offered under have not
        // been accepted, so the three members that describe that standing follow it.
        let has_offer = !offers(&emu, case).await.is_empty();
        assert_eq!(
            has_offer,
            bedrock::state::model_agreement_outstanding(model),
            "case {case}: the offers listing and the service must agree about the agreement, \
             got {has_offer}"
        );
        let expected = if has_offer {
            outstanding += 1;
            (
                bedrock::state::AGREEMENT_STATUS_NOT_AVAILABLE,
                bedrock::state::AVAILABILITY_NOT_AVAILABLE,
                bedrock::state::AUTHORIZATION_STATUS_NOT_AUTHORIZED,
            )
        } else {
            settled += 1;
            (
                bedrock::state::AGREEMENT_STATUS_AVAILABLE,
                bedrock::state::AVAILABILITY_AVAILABLE,
                bedrock::state::AUTHORIZATION_STATUS_AUTHORIZED,
            )
        };
        let agreement = reply.get("agreementAvailability");
        let actual = (
            agreement.get("status").and_then(Value::as_str).unwrap_or("none"),
            reply.get("entitlementAvailability").as_str().unwrap_or("none").to_string(),
            reply.get("authorizationStatus").as_str().unwrap_or("none").to_string(),
        );
        assert_eq!(
            (actual.0, actual.1.as_str(), actual.2.as_str()),
            expected,
            "case {case}: an outstanding offer ({has_offer}) must decide the agreement, the \
             entitlement and the authorization together, got {:?}",
            reply.body
        );
        assert!(
            agreement.get("errorMessage").is_none(),
            "case {case}: nothing about this read failed, so it carries no errorMessage, got \
             {agreement:?}"
        );
    }
    assert!(
        outstanding > 0 && settled > 0,
        "the catalog must carry both a model offered under an agreement and one offered under \
         none, got {outstanding} and {settled}"
    );
}

#[tokio::test]
async fn availability_refuses_an_identifier_that_names_no_offered_model() {
    let emu = emulator();
    for (case, identifier, named) in rows() {
        // The member is a `BedrockModelId`, whose pattern admits no ARN, so only the bare
        // spellings are requests this operation can carry at all.
        if matches!(named, Named::MissingArn) || identifier.starts_with("arn:") {
            continue;
        }
        let reply = availability(&emu, &identifier).await;
        match named {
            Named::Offered => assert_eq!(
                reply.status, 200,
                "case {case}: an offered model must be answered 200, got {} {:?}",
                reply.status, reply.body
            ),
            // The same reading a read of the catalog refuses an identifier by: a model that
            // is gone, against a value that names no model at all.
            Named::Retired => {
                assert_eq!(
                    (reply.status, code(&reply)),
                    (404, "ResourceNotFoundException"),
                    "case {case}: a model past its end of life has no standing to report, got \
                     {} {} {:?}",
                    reply.status,
                    code(&reply),
                    reply.body
                );
                assert_eq!(
                    message(&case, &reply),
                    bedrock::state::FOUNDATION_MODEL_END_OF_LIFE_MESSAGE,
                    "case {case}: the refusal must state the end-of-life reason, got {:?}",
                    reply.body
                );
            }
            Named::UnreadableId => {
                assert_eq!(
                    (reply.status, code(&reply)),
                    (400, "ValidationException"),
                    "case {case}: a bare id naming no model is an input fault, got {} {} {:?}",
                    reply.status,
                    code(&reply),
                    reply.body
                );
                assert_eq!(
                    message(&case, &reply),
                    bedrock::state::INVALID_MODEL_IDENTIFIER_MESSAGE,
                    "case {case}: the refusal must be the service's fixed sentence, got {:?}",
                    reply.body
                );
            }
            Named::MissingArn => unreachable!("ARN rows are skipped above"),
        }
    }
}

/// Asking about a model is not accepting its terms: the read writes nothing a later caller
/// sees — not the offers it is read off, and not the model's own standing.
#[tokio::test]
async fn reading_the_availability_of_a_model_changes_nothing() {
    let emu = emulator();
    let model_id = CATALOG
        .iter()
        .find(|model| bedrock::state::model_agreement_outstanding(model))
        .map(|model| model.model_id)
        .expect("the catalog must offer a model under an agreement");

    let before = offers(&emu, model_id).await;
    let first = availability(&emu, model_id).await;
    let second = availability(&emu, model_id).await;
    assert_eq!(
        first.body, second.body,
        "two reads of one model must answer identically, got {:?} then {:?}",
        first.body, second.body
    );
    assert_eq!(
        offers(&emu, model_id).await,
        before,
        "the offers the standing is read off must be left as they were"
    );
}
