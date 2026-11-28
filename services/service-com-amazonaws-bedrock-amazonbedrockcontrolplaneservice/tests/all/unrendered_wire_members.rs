// SPDX-License-Identifier: CC0-1.0

//! The wire form of the members this service reports that the RENDERED declaration of a
//! shape cannot express, end-to-end over the same `MultiServiceRouter` the emulator boots.
//!
//! Two kinds of member sit outside what `#[smithy_macros::smithy(shape = ...)]` emits from
//! the model, and both are part of this service's contract:
//!
//! 1. **A `Timestamp` member the model marks `@timestampFormat("date-time")`** — every one
//!    of this service's, since the trait sits on the `Timestamp` shape they all target
//!    (`smithy_query(query="shape",shape="Timestamp")`). On the restJson1 wire that makes
//!    the RFC 3339 STRING form part of the contract, and the vendored codegen puts no format
//!    on a member schema, so such a member would otherwise go out as the protocol's default
//!    epoch NUMBER — a type no generated client deserializes into the modelled `date-time`.
//!    The rule: the member schema reports `date-time`, and the value on the wire is RFC 3339
//!    text.
//! 2. **A member Amazon Bedrock reports that the on-disk shape does not declare** — the
//!    capability records a foundation model listing and a foundation model read carry
//!    (`types/catalog_reporting.rs`), the account-level access default a listing reports
//!    beside its summaries, and the toolbox an inference profile is bound to, which is
//!    STATED whether or not there is one. The rule: each is reported, with the presence the
//!    member's own question has.
//!
//! Asserted over a SET of inputs rather than over one request: the shapes of this service
//! that carry such a member, each with a control member of its own that carries no format,
//! and catalogue rows differing along what the rules generalise over — whether the row
//! publishes a legacy window, what it answers with, whether it streams, and who publishes
//! it.

use ::aws_smithy_schema::traits::TimestampFormat;
use serde_json::Value;

use bedrock::ops::types::{
    AutomatedReasoningPolicyTestCase, AutomatedReasoningPolicyTestResult, DataProcessingDetails,
    FoundationModelLifecycle, GetInferenceProfileOutput, InferenceProfileSummary,
    MarketplaceModelEndpoint, TrainingDetails, TrustedAccessProgramProviderHandoff,
    ValidationDetails,
};
use bedrock::state::{FoundationModel, FOUNDATION_MODELS};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;

use crate::support::{date_time_seconds, emulator, label, Emulator, Reply, REGION};

/// The path the catalogue listing is bound to, and the prefix a read of one model sits
/// under (`smithy_query(query="operation",operation="ListFoundationModels")`).
const CATALOGUE: &str = "/foundation-models";

/// The path the inference profile listing is bound to, and the prefix a read of one profile
/// sits under (`smithy_query(query="operation",operation="ListInferenceProfiles")`).
const PROFILES: &str = "/inference-profiles";

/// The capability members a summary and a read carry for EVERY offered model: what answers
/// about an invocation of it, which every offered model can be given. The on-disk shape
/// declares none of them.
const EVERY_MODEL_CAPABILITY_MEMBERS: &[&str] = &[
    "inferenceAPIsSupported",
    "featuresSupported",
    "explicitPromptCaching",
    "intelligentPromptRouting",
];

/// The members carried only for a model that answers a TURN of a conversation, spoken or
/// written: what describes the conversation, or a job built over one.
const TURN_ONLY_MEMBERS: &[&str] = &[
    "modelFamily",
    "latencyOptimizationSupported",
    "batchSupported",
];

/// RULE: a member the model marks `@timestampFormat("date-time")` reports that format on its
/// member schema, and a member the model gives no `@timestampFormat` reports none.
///
/// The rows are every structure of this service whose instants are written out beside the
/// rendered declaration, each with a CONTROL member of its own — so the assertion is about
/// the trait the model states rather than about every member schema, and a shape whose
/// instants lost the format fails here rather than silently writing an epoch number.
#[test]
fn every_modelled_date_time_member_reports_the_format() {
    // (case, the member schema, whether the model marks that member `date-time`)
    let rows: &[(&str, &::aws_smithy_schema::Schema, bool)] = &[
        (
            "FoundationModelLifecycle$startOfLifeTime",
            &FoundationModelLifecycle::M_START_OF_LIFE_TIME,
            true,
        ),
        (
            "FoundationModelLifecycle$endOfLifeTime",
            &FoundationModelLifecycle::M_END_OF_LIFE_TIME,
            true,
        ),
        (
            "FoundationModelLifecycle$legacyTime",
            &FoundationModelLifecycle::M_LEGACY_TIME,
            true,
        ),
        (
            "FoundationModelLifecycle$publicExtendedAccessTime",
            &FoundationModelLifecycle::M_PUBLIC_EXTENDED_ACCESS_TIME,
            true,
        ),
        (
            "FoundationModelLifecycle$status",
            &FoundationModelLifecycle::M_STATUS,
            false,
        ),
        (
            "InferenceProfileSummary$createdAt",
            &InferenceProfileSummary::M_CREATED_AT,
            true,
        ),
        (
            "InferenceProfileSummary$updatedAt",
            &InferenceProfileSummary::M_UPDATED_AT,
            true,
        ),
        (
            "InferenceProfileSummary$toolboxArn",
            &InferenceProfileSummary::M_TOOLBOX_ARN,
            false,
        ),
        (
            "GetInferenceProfileResponse$createdAt",
            &GetInferenceProfileOutput::M_CREATED_AT,
            true,
        ),
        (
            "GetInferenceProfileResponse$updatedAt",
            &GetInferenceProfileOutput::M_UPDATED_AT,
            true,
        ),
        (
            "GetInferenceProfileResponse$inferenceProfileArn",
            &GetInferenceProfileOutput::M_INFERENCE_PROFILE_ARN,
            false,
        ),
        (
            "AutomatedReasoningPolicyTestCase$createdAt",
            &AutomatedReasoningPolicyTestCase::M_CREATED_AT,
            true,
        ),
        (
            "AutomatedReasoningPolicyTestCase$updatedAt",
            &AutomatedReasoningPolicyTestCase::M_UPDATED_AT,
            true,
        ),
        (
            "AutomatedReasoningPolicyTestCase$testCaseId",
            &AutomatedReasoningPolicyTestCase::M_TEST_CASE_ID,
            false,
        ),
        (
            "AutomatedReasoningPolicyTestResult$updatedAt",
            &AutomatedReasoningPolicyTestResult::M_UPDATED_AT,
            true,
        ),
        (
            "AutomatedReasoningPolicyTestResult$policyArn",
            &AutomatedReasoningPolicyTestResult::M_POLICY_ARN,
            false,
        ),
        (
            "MarketplaceModelEndpoint$createdAt",
            &MarketplaceModelEndpoint::M_CREATED_AT,
            true,
        ),
        (
            "MarketplaceModelEndpoint$updatedAt",
            &MarketplaceModelEndpoint::M_UPDATED_AT,
            true,
        ),
        (
            "MarketplaceModelEndpoint$endpointArn",
            &MarketplaceModelEndpoint::M_ENDPOINT_ARN,
            false,
        ),
        (
            "ValidationDetails$creationTime",
            &ValidationDetails::M_CREATION_TIME,
            true,
        ),
        (
            "ValidationDetails$lastModifiedTime",
            &ValidationDetails::M_LAST_MODIFIED_TIME,
            true,
        ),
        ("ValidationDetails$status", &ValidationDetails::M_STATUS, false),
        (
            "DataProcessingDetails$creationTime",
            &DataProcessingDetails::M_CREATION_TIME,
            true,
        ),
        (
            "DataProcessingDetails$lastModifiedTime",
            &DataProcessingDetails::M_LAST_MODIFIED_TIME,
            true,
        ),
        (
            "TrainingDetails$creationTime",
            &TrainingDetails::M_CREATION_TIME,
            true,
        ),
        (
            "TrainingDetails$lastModifiedTime",
            &TrainingDetails::M_LAST_MODIFIED_TIME,
            true,
        ),
        (
            "TrustedAccessProgramProviderHandoff$applicationTokenExpiresAt",
            &TrustedAccessProgramProviderHandoff::M_APPLICATION_TOKEN_EXPIRES_AT,
            true,
        ),
        (
            "TrustedAccessProgramProviderHandoff$applicationToken",
            &TrustedAccessProgramProviderHandoff::M_APPLICATION_TOKEN,
            false,
        ),
    ];
    for (case, schema, modelled) in rows {
        let format = schema.timestamp_format().map(|trait_| trait_.format());
        let carries = matches!(format, Some(TimestampFormat::DateTime));
        assert_eq!(
            carries, *modelled,
            "case {case}: expected the member schema to carry @timestampFormat(date-time) \
             {modelled}, got {format:?}"
        );
    }
}

/// RULE: every instant a catalogue row reports is RFC 3339 TEXT, and the instants it
/// reports are the ones the row publishes — a launch instant for every offered model, and
/// both ends of the legacy window for a deprecated one and for no other.
///
/// The rows differ along what the rule generalises over: whether the published row carries a
/// legacy window, what the model answers with, whether it streams, and who publishes it.
#[tokio::test]
async fn a_catalogue_row_reports_its_instants_as_rfc_3339_text() {
    let emu = emulator();
    let listing = list(&emu).await;
    for (case, model) in selected_rows() {
        let summary = summary_of(&case, &listing, model.model_id);
        let lifecycle = summary.get("modelLifecycle").unwrap_or_else(|| {
            panic!("case {case:?}: a summary reports the row's lifecycle, got {summary:?}")
        });
        let status = lifecycle.get("status").and_then(Value::as_str);
        assert_eq!(
            status,
            Some(model.lifecycle_status),
            "case {case:?}: expected the row's status {:?}, got {status:?}",
            model.lifecycle_status
        );
        // Every offered model has launched, so its launch instant is reported — as text.
        let launched = instant_text(&case, lifecycle, "startOfLifeTime");
        assert_eq!(
            launched, model.start_of_life_time,
            "case {case:?}: expected the published launch instant {:?}, got {launched:?}",
            model.start_of_life_time
        );
        match model.legacy_window {
            Some(window) => {
                let entered = instant_text(&case, lifecycle, "legacyTime");
                let ends = instant_text(&case, lifecycle, "endOfLifeTime");
                assert_eq!(
                    (entered.as_str(), ends.as_str()),
                    (window.entered_at, window.end_of_life_at),
                    "case {case:?}: expected the published legacy window {:?}, got {:?}",
                    (window.entered_at, window.end_of_life_at),
                    (&entered, &ends)
                );
            }
            None => {
                for member in ["legacyTime", "endOfLifeTime"] {
                    assert!(
                        lifecycle.get(member).is_none(),
                        "case {case:?}: a model with no published legacy window reports no \
                         {member}, got {lifecycle:?}"
                    );
                }
            }
        }
        // Nothing publishes this instant per model, so no row reports one.
        assert!(
            lifecycle.get("publicExtendedAccessTime").is_none(),
            "case {case:?}: no row publishes a public extended access instant, got \
             {lifecycle:?}"
        );
    }
}

/// RULE: the capability records and the account-level access default a listing reports
/// beside the declared members are reported — the ones answering about an INVOCATION of the
/// model for every offered model, the ones describing a conversation with it or a job built
/// over one for a model that answers a turn — and a read of one model reports exactly what
/// the listing carries for it.
#[tokio::test]
async fn a_listing_reports_the_capability_records_and_a_read_agrees_with_it() {
    let emu = emulator();
    let listing = list(&emu).await;
    let default = listing.get("enableAccessToAllModelsByDefault");
    assert!(
        default.is_boolean(),
        "case \"the access default\": a listing states whether this account reaches every \
         offered model by default, got {default:?}"
    );
    for (case, model) in selected_rows() {
        let summary = summary_of(&case, &listing, model.model_id);
        for member in EVERY_MODEL_CAPABILITY_MEMBERS {
            assert!(
                summary.get(member).is_some(),
                "case {case:?}: every offered model can be invoked, so its summary answers \
                 for {member}, got {summary:?}"
            );
        }
        // A spoken turn is carried by the stream the model is driven through rather than by
        // the written-conversation surfaces, so a model that SPEAKS answers a turn without
        // taking part in a written conversation.
        let speaks = model.output_modalities.contains(&"SPEECH");
        let written = model.output_modalities.contains(&"TEXT") && !speaks;
        let answers_a_turn = written || speaks;
        for member in TURN_ONLY_MEMBERS {
            assert_eq!(
                summary.get(member).is_some(),
                answers_a_turn,
                "case {case:?}: {member} is reported exactly for a model that answers a turn \
                 of a conversation ({answers_a_turn}), got {summary:?}"
            );
        }
        assert_eq!(
            summary.get("converse").is_some(),
            written,
            "case {case:?}: the Converse record describes a WRITTEN conversation, so it is \
             reported exactly for a model whose turn is text ({written}), got {summary:?}"
        );
        // The read of one model is the same projection the listing carries for it.
        let read = emu
            .call_in_region(
                REGION,
                "GET",
                &format!("{CATALOGUE}/{}", label(model.model_id)),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a read of an offered model must answer 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.get("modelDetails"),
            summary,
            "case {case:?}: a read must report what the listing reports for the model, got \
             {:?}",
            read.get("modelDetails")
        );
    }
}

/// RULE: a published inference profile reports its two instants as RFC 3339 text and STATES
/// the toolbox it is bound to — as `null` for a profile bound to none, which every profile
/// Amazon Bedrock publishes itself is — and a read of the profile reports what the listing
/// carries for it.
#[tokio::test]
async fn a_published_profile_reports_text_instants_and_states_its_toolbox() {
    let emu = emulator();
    let listing = emu.call_in_region(REGION, "GET", PROFILES, None).await;
    assert_eq!(
        listing.status, 200,
        "case \"the profile listing\": it must answer 200, got {} {:?}",
        listing.status, listing.body
    );
    let summaries = listing
        .get("inferenceProfileSummaries")
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case \"the profile listing\": inferenceProfileSummaries must be a present \
                 array, got {:?}",
                listing.body
            )
        });
    assert!(
        !summaries.is_empty(),
        "case \"the profile listing\": the catalogue publishes profiles in {REGION}, got {:?}",
        listing.body
    );
    for summary in &summaries {
        let case = summary
            .get("inferenceProfileId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("a summary carries its id, got {summary:?}"))
            .to_string();
        for member in ["createdAt", "updatedAt"] {
            let value = summary.get(member).cloned().unwrap_or(Value::Null);
            let text = value.as_str().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {member} is modelled @timestampFormat(\"date-time\"), so it \
                     must be RFC 3339 text, got {value}"
                )
            });
            let _ = date_time_seconds(&case, text);
        }
        assert_eq!(
            summary.get("toolboxArn"),
            Some(&Value::Null),
            "case {case:?}: a profile bound to no toolbox states that it is bound to none, \
             got {summary:?}"
        );
        let read = emu
            .call_in_region(
                REGION,
                "GET",
                &format!("{PROFILES}/{}", label(&case)),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a read of a published profile must answer 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            &read.body, summary,
            "case {case:?}: a read must report what the listing reports for the profile, got \
             {:?}",
            read.body
        );
    }
}

/// `GET /foundation-models`, the whole catalogue.
async fn list(emu: &Emulator) -> Reply {
    let reply = emu.call_in_region(REGION, "GET", CATALOGUE, None).await;
    assert_eq!(
        reply.status, 200,
        "case \"the catalogue listing\": it must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
}

/// The summary the listing reported for `model_id`.
fn summary_of(case: &str, listing: &Reply, model_id: &str) -> Value {
    listing
        .get("modelSummaries")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: modelSummaries must be a present array, got {:?}",
                listing.body
            )
        })
        .iter()
        .find(|summary| summary.get("modelId").and_then(Value::as_str) == Some(model_id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the listing must report {model_id:?}"))
}

/// The instant `member` of a reported lifecycle, as the RFC 3339 TEXT a `date-time` member
/// is on the wire — which is what this reading refuses anything else for: an epoch number
/// parses as no string.
fn instant_text(case: &str, lifecycle: &Value, member: &str) -> String {
    let value = lifecycle.get(member).cloned().unwrap_or(Value::Null);
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {member} is modelled @timestampFormat(\"date-time\"), so it must be \
             RFC 3339 text, got {value}"
        )
    });
    let _ = date_time_seconds(case, text);
    text.to_string()
}

/// Whether a published catalogue row belongs to one class the end-to-end rules are read
/// over. A named type rather than the signature inline, so the table below stays legible.
type RowClass = fn(&FoundationModel) -> bool;

/// The catalogue rows the two end-to-end rules are asserted over: one per class the rules
/// generalise over — a row that publishes a legacy window and one that does not, a model
/// that answers a written turn and one that answers an artifact instead, a model that
/// streams and one that does not, and the models of two different providers — picked by
/// those PROPERTIES rather than by id, so a refreshed catalogue keeps the classes covered.
fn selected_rows() -> Vec<(String, &'static FoundationModel)> {
    let classes: &[(&str, RowClass)] = &[
        ("a deprecated row", |model| model.legacy_window.is_some()),
        ("a row still in life", |model| model.legacy_window.is_none()),
        ("a model that answers a written turn", |model| {
            model.output_modalities.contains(&"TEXT")
        }),
        ("a model whose answer is a vector", |model| {
            model.output_modalities.contains(&"EMBEDDING")
        }),
        ("a model that streams its answer", |model| {
            model.response_streaming_supported
        }),
        ("a model that does not stream", |model| {
            !model.response_streaming_supported
        }),
        ("a model AWS publishes itself", |model| {
            model.provider_name == "Amazon"
        }),
        ("a model of another provider", |model| {
            model.provider_name != "Amazon"
        }),
    ];
    let mut rows = Vec::new();
    for (class, holds) in classes {
        if let Some(model) = FOUNDATION_MODELS.iter().find(|model| holds(model)) {
            rows.push((format!("{class} ({})", model.model_id), model));
        }
    }
    assert!(
        !rows.is_empty(),
        "case \"the catalogue\": it publishes rows to read these rules over"
    );
    rows
}
