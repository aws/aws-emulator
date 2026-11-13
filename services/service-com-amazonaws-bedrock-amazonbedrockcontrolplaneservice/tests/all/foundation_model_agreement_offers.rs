// SPDX-License-Identifier: CC0-1.0

//! `ListFoundationModelAgreementOffers` end to end over HTTP, through the same
//! `MultiServiceRouter` the emulator boots.
//!
//! # The rule
//!
//! An offer is an offer of the AGREEMENT a model is published under, so the `modelId` has
//! to name a model this account's published catalog offers under one: such a model answers
//! 200 with its own offers — an offer carrying the `offerToken` that feeds
//! `CreateFoundationModelAgreement` and the term structures the agreement is made on.
//!
//! Every other `modelId` names no model there are offers OF, and is refused rather than
//! answered with an empty list: a model AWS publishes itself is reached without accepting
//! another party's terms, so no agreement of it exists to be offered, and an id naming no
//! model of the catalog at all names nothing either — neither is a value this operation can
//! read as the identifier of an agreement-bearing model, which is the 400
//! `ValidationException` carrying this service's fixed invalid-identifier sentence. A model
//! the service recognises and offers no longer is a resource that is GONE instead, which
//! every read of this catalog reports as 404 `ResourceNotFoundException` with the
//! end-of-life reason.
//!
//! # What the tables below generalise over
//!
//! The rows walk the WHOLE published catalog rather than a chosen row of it, so every
//! provider in it is covered on both sides of the rule, and they differ along the
//! `offerType` the request names (absent, `ALL`, `PUBLIC` — the two values `OfferType`
//! declares) and along the form of the `modelId` (the bare id, and the qualified form
//! `BedrockModelId`'s pattern admits). The refusal rows differ along WHY an id names no
//! agreement-bearing model: a provider the catalog carries no model of, a model of a
//! provider it does carry, a deployment-suffixed id, a model past its end of life, and a
//! model AWS publishes itself.

use serde_json::Value;
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{emulator, error_code, label, Emulator, Reply};

/// The published catalog this service serves.
const CATALOG: &[bedrock::state::FoundationModel] = bedrock::state::FOUNDATION_MODELS;

/// Every `offerType` a request can carry: absent, and the two values `OfferType` declares
/// (`smithy_query(query="shape",shape="OfferType")`).
const OFFER_TYPES: [Option<&str>; 3] = [
    None,
    Some(bedrock::state::OFFER_TYPE_ALL),
    Some(bedrock::state::OFFER_TYPE_PUBLIC),
];

/// `GET /list-foundation-model-agreement-offers/{modelId}`, with the `offerType` the row
/// names.
async fn offers(emulator: &Emulator, model_id: &str, offer_type: Option<&str>) -> Reply {
    let path = match offer_type {
        Some(offer_type) => format!(
            "/list-foundation-model-agreement-offers/{}?offerType={offer_type}",
            label(model_id)
        ),
        None => format!(
            "/list-foundation-model-agreement-offers/{}",
            label(model_id)
        ),
    };
    emulator.call("GET", &path, None).await
}

/// The offers a reply reports.
fn reported(reply: &Reply) -> Vec<Value> {
    reply.get("offers").as_array().cloned().unwrap_or_default()
}

/// The wire error code a refusal carries.
fn code(reply: &Reply) -> &str {
    reply.error.as_deref().map(error_code).unwrap_or("none")
}

/// The `message` a refusal carries.
fn message(case: &str, reply: &Reply) -> String {
    reply
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case}: a refusal must carry a message, got {:?}",
                reply.body
            )
        })
        .to_string()
}

/// Assert `reply` is the refusal this service answers an id that names no agreement-bearing
/// model with: 400 `ValidationException`, the error class in the `x-amzn-errortype` header,
/// carrying the fixed invalid-identifier sentence and no offers.
fn assert_invalid_identifier(case: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, code(reply)),
        (400, "ValidationException"),
        "case {case}: must be refused 400 ValidationException, got {} {} {:?}",
        reply.status,
        code(reply),
        reply.body
    );
    assert_eq!(
        message(case, reply),
        bedrock::state::INVALID_MODEL_IDENTIFIER_MESSAGE,
        "case {case}: the refusal must be the service's fixed sentence, got {:?}",
        reply.body
    );
    assert!(
        reported(reply).is_empty(),
        "case {case}: a refusal reports no offers, got {:?}",
        reply.body
    );
}

/// Every model the account's own `ListFoundationModels` catalog reports.
async fn catalogued(emulator: &Emulator) -> Vec<Value> {
    let listed = emulator.call("GET", "/foundation-models", None).await;
    assert_eq!(
        listed.status, 200,
        "the catalog must be served, got {} {:?}",
        listed.status, listed.body
    );
    let summaries = listed
        .get("modelSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !summaries.is_empty(),
        "the published catalog must report models, got {:?}",
        listed.body
    );
    summaries
}

/// RULE: a catalogued model is answered by whether it is offered under an agreement at all
/// — one offered under a provider's terms answers 200 with its own usable offer, and one AWS
/// publishes itself has no agreement to be offered and is refused as an identifier naming no
/// agreement-bearing model.
#[tokio::test]
async fn every_catalogued_model_is_answered_by_the_agreement_it_is_offered_under() {
    let emulator = emulator();
    let mut with_offers = 0usize;
    let mut refused = 0usize;
    for summary in catalogued(&emulator).await {
        let model_id = summary["modelId"].as_str().expect("a modelId").to_string();
        let provider = summary["providerName"]
            .as_str()
            .expect("a providerName")
            .to_string();
        let case = format!("{model_id} by {provider}");

        let reply = offers(&emulator, &model_id, None).await;

        // A model AWS publishes itself carries no agreement to accept; every other
        // provider's model is offered under that provider's own terms.
        if provider == bedrock::state::AWS_MODEL_PROVIDER {
            assert_invalid_identifier(&case, &reply);
            refused += 1;
            continue;
        }

        assert_eq!(
            reply.status, 200,
            "case {case}: a model offered under an agreement must be answered 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.text("modelId"),
            model_id,
            "case {case}: the response must name the model it is about, got {:?}",
            reply.get("modelId")
        );

        let listed = reported(&reply);
        assert!(
            !listed.is_empty(),
            "case {case}: a model offered under a provider's own terms must expose an offer, \
             got {:?}",
            reply.body
        );
        with_offers += 1;

        for offer in &listed {
            let token = offer["offerToken"].as_str().unwrap_or_else(|| {
                panic!("case {case}: an offer must carry an offerToken, got {offer:?}")
            });
            assert!(
                !token.is_empty(),
                "case {case}: an offerToken must be usable, got {token:?}"
            );
            let offer_id = offer["offerId"].as_str().unwrap_or_default();
            assert_eq!(
                offer_id,
                bedrock::state::model_offer_id(&model_id),
                "case {case}: the offerId must be the one this model's offer carries, got \
                 {offer_id:?}"
            );
            assert_eq!(
                token,
                bedrock::state::model_offer_token(&model_id, offer_id),
                "case {case}: the offerToken must be the token of this model's offer, got \
                 {token:?}"
            );
            // The terms an offer is made on: the three structures `TermDetails` requires
            // are reported, and the price list, license document and support policy they
            // would name are not invented here.
            for term in ["usageBasedPricingTerm", "legalTerm", "supportTerm"] {
                assert!(
                    offer["termDetails"].get(term).is_some(),
                    "case {case}: termDetails must carry {term}, got {:?}",
                    offer["termDetails"]
                );
            }
            assert_eq!(
                offer["termDetails"]["usageBasedPricingTerm"]
                    .get("rateCard")
                    .and_then(Value::as_array)
                    .map(Vec::len),
                Some(0),
                "case {case}: the usage-based pricing term must report its rate card, got {:?}",
                offer["termDetails"]
            );
        }

        // Reading twice changes nothing: this operation is @readonly, and an offer is
        // published data rather than a resource a read brings into being.
        let again = offers(&emulator, &model_id, None).await;
        assert_eq!(
            reported(&again),
            listed,
            "case {case}: a second listing must report the same offers, got {:?}",
            again.body
        );
    }
    assert!(
        with_offers > 0 && refused > 0,
        "the catalog must carry both a model offered under an agreement and one offered under \
         none, got {with_offers} and {refused}"
    );
}

/// RULE: `offerType` narrows the listing to the offers of that type. Every offer this
/// emulator publishes is public, so `ALL` and `PUBLIC` admit the same offers and a request
/// naming neither gets all of them.
#[tokio::test]
async fn the_offer_type_narrows_the_listing_to_offers_of_that_type() {
    let emulator = emulator();
    // A third-party model: the one that has an offer to narrow at all. Its id is a
    // published row of this service's own catalog.
    const MODEL_ID: &str = "anthropic.claude-3-haiku-20240307-v1:0";
    let all = reported(&offers(&emulator, MODEL_ID, None).await);
    assert!(
        !all.is_empty(),
        "the fixture model must expose an offer, got {all:?}"
    );
    for offer_type in [
        bedrock::state::OFFER_TYPE_ALL,
        bedrock::state::OFFER_TYPE_PUBLIC,
    ] {
        let reply = offers(&emulator, MODEL_ID, Some(offer_type)).await;
        assert_eq!(
            reply.status, 200,
            "case {offer_type}: the listing must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reported(&reply),
            all,
            "case {offer_type}: every offer here is public, so this type admits them all, got \
             {:?}",
            reply.body
        );
    }

    // A value `OfferType` does not declare is refused by the member's own constraint,
    // ahead of the handler.
    let unknown = offers(&emulator, MODEL_ID, Some("PRIVATE")).await;
    assert_eq!(
        unknown.status, 400,
        "case an undeclared offerType: must be refused 400, got {} {:?}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error.as_deref(),
        Some("ValidationException"),
        "case an undeclared offerType: a constraint violation is a ValidationException, got {:?}",
        unknown.error
    );
}

/// RULE: a `modelId` that names no model offered under an agreement is refused 400
/// `ValidationException` with the fixed invalid-identifier sentence — never an empty success
/// — whichever `offerType` the request carries, and the qualified form of an
/// agreement-bearing id names that same published model.
#[tokio::test]
async fn an_id_that_names_no_agreement_bearing_model_is_refused_as_invalid() {
    let emulator = emulator();

    // The qualified form `BedrockModelId`'s pattern admits — the published id with the
    // `:`-separated qualifiers AWS appends to it — names the same model, so it answers the
    // offers of that model.
    let qualified = offers(&emulator, "anthropic.claude-3-haiku-20240307-v1:0:200k", None).await;
    assert_eq!(
        qualified.status, 200,
        "case a qualified model id: must name the same published model, got {} {:?}",
        qualified.status, qualified.body
    );
    assert!(
        !reported(&qualified).is_empty(),
        "case a qualified model id: must report that model's offers, got {:?}",
        qualified.body
    );

    // One axis per row: a provider the catalog has no model of, a model id of a provider it
    // does carry, a well-formed id whose model was never published, and — the other half of
    // the rule — models AWS publishes itself, which the catalog carries and offers under no
    // agreement. The AWS-published rows are read off the catalog rather than written out, so
    // they are not one chosen id.
    let mut rows: Vec<(String, String)> = vec![
        (
            "a provider the catalog carries no model of".to_string(),
            "acme.model-v1".to_string(),
        ),
        (
            "a model the catalog does not carry from a provider it does".to_string(),
            "anthropic.claude-9-opus-20990101-v9:0".to_string(),
        ),
        (
            "a model id with a deployment suffix".to_string(),
            "meta.llama3-1-8b-instruct-v1:0/abcdef123456".to_string(),
        ),
    ];
    let aws_published: Vec<&bedrock::state::FoundationModel> = CATALOG
        .iter()
        .filter(|model| !bedrock::state::model_offered_under_agreement(model))
        .collect();
    assert!(
        aws_published.len() > 1,
        "the catalog must carry models AWS publishes itself, got {}",
        aws_published.len()
    );
    for model in &aws_published {
        rows.push((
            format!("the AWS-published model {}", model.model_id),
            model.model_id.to_string(),
        ));
    }

    for (case, model_id) in rows {
        for offer_type in OFFER_TYPES {
            let case = format!("{case} with offerType {offer_type:?}");
            let reply = offers(&emulator, &model_id, offer_type).await;
            assert_invalid_identifier(&case, &reply);
        }
    }
}

/// RULE: a model this service recognises and offers no longer is a resource that is GONE, so
/// the offers of it are refused 404 `ResourceNotFoundException` with the end-of-life reason
/// rather than reported as an unreadable identifier.
#[tokio::test]
async fn a_model_past_its_end_of_life_has_no_offers_to_report() {
    let emulator = emulator();
    // Three providers, read off the ids this service publishes as past their end of life
    // (`crate::state::RETIRED_FOUNDATION_MODELS`), none of them the id the recorded step
    // named.
    for model_id in [
        "anthropic.claude-v2",
        "ai21.j2-ultra-v1",
        "meta.llama2-70b-chat-v1",
    ] {
        assert!(
            bedrock::state::RETIRED_FOUNDATION_MODELS.contains(&model_id),
            "case {model_id}: this row must name a model the service recognises as retired"
        );
        let reply = offers(&emulator, model_id, None).await;
        assert_eq!(
            (reply.status, code(&reply)),
            (404, "ResourceNotFoundException"),
            "case {model_id}: a model past its end of life is gone rather than unreadable, got \
             {} {} {:?}",
            reply.status,
            code(&reply),
            reply.body
        );
        assert_eq!(
            message(model_id, &reply),
            bedrock::state::FOUNDATION_MODEL_END_OF_LIFE_MESSAGE,
            "case {model_id}: the refusal must state the end-of-life reason, got {:?}",
            reply.body
        );
    }
}
