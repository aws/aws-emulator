// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a `SourceFormat` outside the modelled enum, from every
//! operation whose input declares that member.
//!
//! The rule under test: a refusal of a value outside a modelled enum's value set is
//! this service's ONE enum clause — the member and the value set the service REPORTS,
//! with no echo of what the caller sent — whichever operation refuses it, and whether
//! the boundary validator or a handler re-stating that same constraint is the one
//! answering. A value the enum DOES declare is served.
//!
//! The rows differ along what the rule generalises over: the FORM of the rejected
//! value (another real format name, wrong case, a padded member, the empty string,
//! unicode look-alikes, a numeric string, a member of a DIFFERENT modelled enum of
//! this service, and an over-long string) and the OPERATION refusing it — the
//! create, the listing and the publish, which are every operation whose input
//! carries `SourceFormat`. The reported set itself is read from
//! `state::REPORTED_SOURCE_FORMATS` and the accepted one from the generated
//! `SOURCE_FORMAT_VALUES`, so neither expectation is written out here.
//!
//! Each refusal is asserted twice over: once at the handler, called directly with a
//! typed input, which is the only caller that reaches the handler's own re-statement
//! of the enum, and once over the awsJson1_0 wire through the shared [`Profiles`]
//! harness, where the boundary validator answers first. The two must read
//! identically — that is the rule.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, Service,
};

use service_com_amazonaws_healthlake_healthlake as healthlake;

use healthlake::ops::create_data_transformation_profile::{
    CreateDataTransformationProfile, CreateDataTransformationProfileError,
    CreateDataTransformationProfileErrorModeled,
};
use healthlake::ops::list_data_transformation_profiles::{
    ListDataTransformationProfiles, ListDataTransformationProfilesError,
    ListDataTransformationProfilesErrorModeled,
};
use healthlake::ops::publish_data_transformation_profile::{
    PublishDataTransformationProfile, PublishDataTransformationProfileError,
    PublishDataTransformationProfileErrorModeled,
};
use healthlake::ops::types::{
    CreateDataTransformationProfileInput, CreateDataTransformationProfileSource,
    ListDataTransformationProfilesInput, ProfileMappingSource,
    PublishDataTransformationProfileInput, SOURCE_FORMAT_VALUES,
};
use healthlake::scaffolded::Scaffolded;
use healthlake::state::REPORTED_SOURCE_FORMATS;

use crate::control_plane_error_semantics::enum_envelope;
use crate::data_transformation_profile::{
    mapping, modelled_message, profile_mapping_source, Profiles,
};
use crate::fhir_datastore_lifecycle::{error_code, string_at};

/// The violation path of the `SourceFormat` member — its model name in the
/// lowerCamel spelling a violation path takes (the member is the same in all three
/// inputs; see `smithy_query(query="operation",
/// operation="CreateDataTransformationProfile,ListDataTransformationProfiles,PublishDataTransformationProfile")`).
const SOURCE_FORMAT_PATH: &str = "sourceFormat";

/// The success status this service answers `CreateDataTransformationProfile` with:
/// awsJson1_0 binds nothing to the transport, so the operation's `@http(code: 201)`
/// is inert and a success is the protocol's 200.
const CREATE_SUCCESS_STATUS: u16 = 200;

/// The forms a value outside the modelled `SourceFormat` set can take. The rule
/// generalises over the value, so the rows differ along its form alone — including
/// a value that IS a member of another modelled enum of this service
/// (`TargetFormat`'s `FHIR_R4`), which is what makes "outside THIS member's set"
/// the thing under test.
///
/// `HL7V2` is deliberately among them: it is a format the service's own refusal
/// REPORTS, so this row pins what the emulator does with it — the constraint the
/// boundary walk enforces is the model snapshot's enum, which omits it, so the value
/// is refused and the refusal names it among the reported set. The snapshot is what
/// would have to grow for it to be accepted; nothing here decides that quietly.
fn rejected_source_formats() -> Vec<String> {
    let mut cases: Vec<String> = [
        "HL7V2", "HL7", "ccda", "csv", " CSV", "CSV ", "", "ⅭⅭⅮⅯ", "4", "FHIR_R4", "CCDA,CSV",
    ]
    .iter()
    .map(|case| (*case).to_string())
    .collect();
    // Longer than any value the set declares, so length is not what tells it apart.
    cases.push("X".repeat(300));
    cases
}

/// A `ProfileMapping` source carrying one entry, for the creates below.
///
/// The one `mapping-config.yaml` document, so the publishes below are of a mapping
/// EITHER modelled format admits: what a publish checks is the shape of the content
/// against the format it names, and this suite is about the format VALUE.
fn one_entry_source() -> CreateDataTransformationProfileSource {
    CreateDataTransformationProfileSource::ProfileMapping(ProfileMappingSource {
        profile_mapping: HashMap::from([(
            "mapping-config.yaml".to_string(),
            "tableName: patients".to_string(),
        )]),
    })
}

/// A `ProfileId` that satisfies the modelled `^[a-f0-9]{32}$` pattern, so the only
/// rule a publish row breaks is the one under test.
const WELL_FORMED_PROFILE_ID: &str = "deadbeefdeadbeefdeadbeefdeadbeef";

/// One emulator: the shared engine, the service mounted on a router for the
/// callback a handler is given, and the request metadata a handler reads.
///
/// The operations are called DIRECTLY here because the boundary validator refuses an
/// out-of-set enum value before dispatch, so a handler's own re-statement of the
/// enum constraint has no other caller.
struct Handlers {
    engine: Arc<engine::InMemoryEngine>,
    service: healthlake::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
}

impl Handlers {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(healthlake::EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: healthlake::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts: signed_request_parts(),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    async fn create(
        &self,
        source_format: &str,
        profile_name: &str,
    ) -> Result<healthlake::ops::types::CreatedProfile, CreateDataTransformationProfileError> {
        CreateDataTransformationProfile::new(Arc::clone(&self.engine))
            .handle(
                CreateDataTransformationProfileInput {
                    source_format: source_format.to_string(),
                    source: one_entry_source(),
                    kms_key_id: None,
                    profile_description: None,
                    profile_name: profile_name.to_string(),
                    tags: None,
                    client_token: None,
                },
                &self.ctx(),
            )
            .await
    }

    async fn list(
        &self,
        source_format: &str,
    ) -> Result<
        healthlake::ops::types::ListDataTransformationProfilesOutput,
        ListDataTransformationProfilesError,
    > {
        ListDataTransformationProfiles::new(Arc::clone(&self.engine))
            .handle(
                ListDataTransformationProfilesInput {
                    source_format: source_format.to_string(),
                    max_results: None,
                    next_token: None,
                },
                &self.ctx(),
            )
            .await
    }

    async fn publish(
        &self,
        profile_id: &str,
        source_format: &str,
    ) -> Result<
        healthlake::ops::types::PublishDataTransformationProfileOutput,
        PublishDataTransformationProfileError,
    > {
        PublishDataTransformationProfile::new(Arc::clone(&self.engine))
            .handle(
                PublishDataTransformationProfileInput {
                    profile_id: profile_id.to_string(),
                    source_format: source_format.to_string(),
                    from_existing_version: None,
                    change_description: None,
                },
                &self.ctx(),
            )
            .await
    }
}

/// Request metadata naming this service's signing name and a region, which is how a
/// request names the region a handler records.
fn signed_request_parts() -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/us-west-2/healthlake/aws4_request, \
             SignedHeaders=host, Signature=emulator",
        )
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// The modelled `Message` of the create's refusal, or a panic naming the case.
fn create_refusal(case: &str, error: &CreateDataTransformationProfileError) -> String {
    match error {
        Scaffolded::Modeled(CreateDataTransformationProfileErrorModeled::ValidationException(
            refused,
        )) => refused.message.clone().unwrap_or_default(),
        other => panic!(
            "case {case:?}: CreateDataTransformationProfile must answer its modelled \
             ValidationException, got {other:?}"
        ),
    }
}

/// The modelled `Message` of the listing's refusal, or a panic naming the case.
fn list_refusal(case: &str, error: &ListDataTransformationProfilesError) -> String {
    match error {
        Scaffolded::Modeled(ListDataTransformationProfilesErrorModeled::ValidationException(
            refused,
        )) => refused.message.clone().unwrap_or_default(),
        other => panic!(
            "case {case:?}: ListDataTransformationProfiles must answer its modelled \
             ValidationException, got {other:?}"
        ),
    }
}

/// The modelled `Message` of the publish's refusal, or a panic naming the case.
fn publish_refusal(case: &str, error: &PublishDataTransformationProfileError) -> String {
    match error {
        Scaffolded::Modeled(PublishDataTransformationProfileErrorModeled::ValidationException(
            refused,
        )) => refused.message.clone().unwrap_or_default(),
        other => panic!(
            "case {case:?}: PublishDataTransformationProfile must answer its modelled \
             ValidationException, got {other:?}"
        ),
    }
}

/// `message` is this service's enum clause for `SourceFormat` and quotes nothing the
/// caller sent.
fn assert_enum_clause(case: &str, operation: &str, message: &str) {
    let expected = enum_envelope(SOURCE_FORMAT_PATH, REPORTED_SOURCE_FORMATS);
    assert_eq!(
        message, expected,
        "case {case:?}: {operation} must state the service's enum clause, expected \
         {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains(&format!("'{case}'")),
        "case {case:?}: {operation} must not quote the rejected value, got {message:?}"
    );
}

#[tokio::test]
async fn every_handler_that_restates_the_source_format_enum_states_one_clause() {
    let handlers = Handlers::new();

    for (row, case) in rejected_source_formats().iter().enumerate() {
        let refused = handlers
            .create(case, &format!("rejected-{row}"))
            .await
            .expect_err(&format!(
                "case {case:?}: a format outside the modelled set must be refused"
            ));
        assert_enum_clause(
            case,
            "CreateDataTransformationProfile",
            &create_refusal(case, &refused),
        );

        let refused = handlers.list(case).await.expect_err(&format!(
            "case {case:?}: a listing filtered by a format outside the modelled set must be \
             refused"
        ));
        assert_enum_clause(
            case,
            "ListDataTransformationProfiles",
            &list_refusal(case, &refused),
        );

        let refused = handlers
            .publish(WELL_FORMED_PROFILE_ID, case)
            .await
            .expect_err(&format!(
                "case {case:?}: a publish naming a format outside the modelled set must be \
                 refused"
            ));
        assert_enum_clause(
            case,
            "PublishDataTransformationProfile",
            &publish_refusal(case, &refused),
        );
    }

    // A refused create stored nothing: every modelled format lists empty.
    for source_format in SOURCE_FORMAT_VALUES {
        let listed = handlers
            .list(source_format)
            .await
            .unwrap_or_else(|error| panic!("{source_format} must list, got {error:?}"));
        assert!(
            listed.items.is_empty(),
            "case {source_format:?}: a refused create must mint no profile, got {:?}",
            listed.items
        );
    }

    // The positive control, and the boundary of the rule: every value the enum
    // DOES declare is served, and the profile it mints is publishable under that
    // same format.
    for source_format in SOURCE_FORMAT_VALUES {
        let created = handlers
            .create(source_format, &format!("accepted-{source_format}"))
            .await
            .unwrap_or_else(|error| {
                panic!("case {source_format:?}: a modelled format must be served, got {error:?}")
            });
        assert_eq!(
            created.modelled.source_format, *source_format,
            "case {source_format:?}: the created profile reports the requested format, got {:?}",
            created.modelled.source_format
        );
        let published = handlers
            .publish(&created.modelled.profile_id, source_format)
            .await
            .unwrap_or_else(|error| {
                panic!("case {source_format:?}: the minted profile must publish, got {error:?}")
            });
        assert_eq!(
            published.version, 1,
            "case {source_format:?}: the first publish is version 1, got {}",
            published.version
        );
    }
}

#[tokio::test]
async fn the_wire_states_the_same_clause_for_every_operation_and_mints_nothing() {
    let healthlake = Profiles::new();

    // A real profile, so a publish row's `ProfileId` names something and the only
    // rule it breaks is the one under test.
    let created = healthlake
        .ok(
            "the control profile",
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": "wire-control",
                "SourceFormat": SOURCE_FORMAT_VALUES[0],
                "Source": profile_mapping_source(&mapping(&[(
                    "patients.csv.map",
                    "Patient.id <- patient_id",
                )])),
            }),
        )
        .await;
    let control = string_at(&created, "ProfileId");

    for case in rejected_source_formats() {
        let requests: Vec<(&str, Value)> = vec![
            (
                "CreateDataTransformationProfile",
                json!({
                    "ProfileName": "rejected-on-the-wire",
                    "SourceFormat": case,
                    "Source": profile_mapping_source(&mapping(&[(
                        "patients.csv.map",
                        "Patient.id <- patient_id",
                    )])),
                }),
            ),
            (
                "ListDataTransformationProfiles",
                json!({ "SourceFormat": case }),
            ),
            (
                "PublishDataTransformationProfile",
                json!({ "ProfileId": &control, "SourceFormat": case }),
            ),
        ];
        for (operation, request) in requests {
            let (status, payload) = healthlake.call(operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} must refuse a format outside the modelled set \
                 with 400, got {status}: {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ValidationException",
                "case {case:?}: {operation} must answer ValidationException, got {payload}"
            );
            let message = modelled_message(&payload).unwrap_or_else(|| {
                panic!("case {case:?}: {operation} must carry the modelled Message, got {payload}")
            });
            assert_enum_clause(&case, operation, message);
        }
    }

    // Nothing the refusals touched was minted or published: the control profile is
    // the only one the service holds, still at its DRAFT.
    for source_format in SOURCE_FORMAT_VALUES {
        let listed = healthlake
            .ok(
                source_format,
                "ListDataTransformationProfiles",
                &json!({ "SourceFormat": source_format }),
            )
            .await;
        let items = listed["Items"].as_array().cloned().unwrap_or_default();
        let expected = usize::from(*source_format == SOURCE_FORMAT_VALUES[0]);
        assert_eq!(
            items.len(),
            expected,
            "case {source_format:?}: only the control profile may exist, got {listed}"
        );
        for item in &items {
            assert_eq!(
                item["ProfileId"].as_str(),
                Some(control.as_str()),
                "case {source_format:?}: the one profile must be the control, got {listed}"
            );
            assert_eq!(
                item["Version"].as_i64(),
                Some(0),
                "case {source_format:?}: a refused publish must leave the DRAFT, got {listed}"
            );
        }
    }

    // And the control profile is still servable under the format it was created
    // with, read back through the modelled reader.
    let draft = healthlake
        .ok(
            "the control profile",
            "GetDataTransformationProfile",
            &json!({ "ProfileId": &control, "ProfileVersion": 0 }),
        )
        .await;
    assert_eq!(
        draft["SourceFormat"].as_str(),
        Some(SOURCE_FORMAT_VALUES[0]),
        "the control profile must still report the format it was created with, got {draft}"
    );

    // A create at the protocol's success status, for each value the enum declares:
    // the refusals above are about the value, not about the operation.
    for source_format in SOURCE_FORMAT_VALUES {
        let (status, payload) = healthlake
            .call(
                "CreateDataTransformationProfile",
                &json!({
                    "ProfileName": format!("wire-accepted-{source_format}"),
                    "SourceFormat": source_format,
                    "Source": profile_mapping_source(&mapping(&[(
                        "patients.csv.map",
                        "Patient.id <- patient_id",
                    )])),
                }),
            )
            .await;
        assert_eq!(
            status, CREATE_SUCCESS_STATUS,
            "case {source_format:?}: a modelled format must be served, got {status}: {payload}"
        );
    }
}
