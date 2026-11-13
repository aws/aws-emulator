// SPDX-License-Identifier: CC0-1.0

//! `ValidateConfiguration` gives a VERDICT about one stored configuration version: it runs
//! every validator the configuration profile carries against that version's content and
//! answers the modelled empty 204 only when all of them admit it.
//!
//! The rule under test, in one sentence: "a validator provides a syntactic or semantic
//! check to ensure the configuration that you want to deploy functions as intended. To
//! validate your application configuration data, you provide a schema or an Amazon Web
//! Services Lambda function that runs against the configuration. The configuration
//! deployment or update can only proceed when the configuration data is valid"
//! (`smithy_query(query="doc",shape="Validator")`) — so an operation AppConfig documents as
//! "uses the validators in a configuration profile to validate a configuration"
//! (`smithy_query(query="doc",shape="ValidateConfiguration")`) refuses, with
//! `BadRequestException` (HTTP 400), a candidate any one of those validators reports
//! non-compliant, and reports its verdict without changing anything: the version stays
//! stored and served, nothing is deployed, and the same call asked twice answers twice the
//! same.
//!
//! The tables span what the rule generalises over rather than the one recorded candidate:
//! how many validators the profile carries and WHICH of them gives the verdict, the keyword
//! that fails, the depth of the violating node, the JSON type of the offending value,
//! whether the candidate is JSON at all, a profile carrying no validator, and the two
//! spellings `ConfigurationVersion` accepts (a version number and a version label).
//!
//! Every row drives real HTTP requests through the router the emulator's front door uses.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, json_call, router, send, Answer,
};

/// A draft-07 schema requiring a boolean `enabled` member: the shape of the schema an
/// operator attaches to guard a feature-toggle configuration.
const ENABLED_SCHEMA: &str = r#"{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"enabled":{"type":"boolean"}},"required":["enabled"]}"#;

/// A schema that types a member one level DOWN, so the node a refusal names is a nested
/// pointer rather than a top-level member.
const NESTED_SCHEMA: &str = r#"{"type":"object","properties":{"db":{"type":"object","properties":{"port":{"type":"integer"}}}}}"#;

/// A schema that admits any JSON document at all: the validator that is present and gives
/// no reason to refuse.
const ANY_SCHEMA: &str = "{}";

/// The node a violation of the candidate document as a whole is reported at, written as the
/// root JSON pointer.
const ROOT_NODE: &str = "/";

/// The violation a refused row must be refused for.
struct Refusal {
    /// Which of the row's validators gives the verdict — the refusal echoes THAT
    /// validator's schema document, so the index is what the assertion checks.
    validator: usize,
    /// The node the refusal names.
    node: &'static str,
    /// The full finding text where this error class has recorded wording, and `None` where
    /// the evaluator's own report is what the refusal carries.
    finding: Option<&'static str>,
}

/// One row: the validators a profile carries, a candidate configuration, and the verdict
/// `ValidateConfiguration` must report for it.
struct Row {
    case: &'static str,
    /// The `JSON_SCHEMA` documents the profile's `Validators` carry, in order
    /// (`ValidatorList` is `length 0..2`).
    validators: &'static [&'static str],
    content: &'static [u8],
    /// `None` when every validator admits the candidate, so the call answers 204.
    refusal: Option<Refusal>,
}

/// `AppConfig`'s refusal of a call whose candidate configuration a `JSON_SCHEMA` validator
/// reported non-compliant: the preamble, the schema document echoed from the profile, and
/// the bracketed report of the violation.
fn schema_refusal(schema: &str, finding: &str, node: &str) -> String {
    format!(
        "Could not build JSON schema from invalid input:\n {schema}.\n Original error \
         message:\n[message: \"{finding}\" - parent node: {node}]"
    )
}

/// One `JSON_SCHEMA` entry of a profile's `Validators` list.
fn schema_validator(schema: &str) -> Value {
    json!({ "Type": "JSON_SCHEMA", "Content": schema })
}

/// A hosted configuration profile carrying `validators`. Answers its minted id.
pub(crate) async fn given_profile(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    name: &str,
    validators: &[&str],
) -> String {
    let validators: Vec<Value> = validators.iter().copied().map(schema_validator).collect();
    let created = json_call(
        router,
        "POST",
        &format!("/applications/{application}/configurationprofiles"),
        &json!({ "Name": name, "LocationUri": "hosted", "Validators": validators }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateConfigurationProfile must answer 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: CreateConfigurationProfile answered no Id"))
}

/// Store one hosted configuration version, optionally labelled, and answer the version
/// number it was given.
///
/// Asserted to be accepted: a profile's validators gate the calls that put a configuration
/// into service, not its STORAGE, so a version no validator admits is stored verbatim.
pub(crate) async fn given_version(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    profile: &str,
    content: &[u8],
    label: Option<&str>,
) -> String {
    let headers: Vec<(&str, String)> = label
        .map(|label| vec![("VersionLabel", label.to_string())])
        .unwrap_or_default();
    let created = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        content.to_vec(),
        &headers,
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateHostedConfigurationVersion must answer 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .header("Version-Number")
        .unwrap_or_else(|| panic!("case {case:?}: no Version-Number was answered"))
}

/// `ValidateConfiguration` of one configuration version, named as the caller spells it.
pub(crate) async fn validate_configuration(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    version: &str,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/validators\
             ?configuration_version={version}"
        ),
        &Value::Null,
    )
    .await
}

/// The bytes `GetHostedConfigurationVersion` still holds for one version.
async fn stored_content(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    version: &str,
) -> bytes::Bytes {
    json_call(
        router,
        "GET",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions/{version}"
        ),
        &Value::Null,
    )
    .await
    .body
}

/// How many hosted configuration versions one profile holds.
async fn stored_versions(router: &MultiServiceRouter, application: &str, profile: &str) -> usize {
    json_call(
        router,
        "GET",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        &Value::Null,
    )
    .await
    .json()["Items"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// The deployment numbers one environment's history holds.
async fn deployment_numbers(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
) -> Vec<i64> {
    json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &Value::Null,
    )
    .await
    .json()["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["DeploymentNumber"].as_i64())
        .collect()
}

/// Assert `answer` is the modelled empty 204.
fn assert_admitted(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        204,
        "case {case:?}: every validator admits this candidate, so ValidateConfiguration must \
         answer the modelled 204, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.body.is_empty(),
        "case {case:?}: the modelled 204 carries an empty body, got {:?}",
        String::from_utf8_lossy(&answer.body)
    );
}

/// Assert `answer` is the modelled 400 refusal carrying the schema-violation report the row
/// expects, echoing the schema of the validator that gave the verdict.
fn assert_refused(case: &str, answer: &Answer, schema: &str, refusal: &Refusal) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: ValidateConfiguration must answer the modelled 400, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.error_code().contains("BadRequestException"),
        "case {case:?}: ValidateConfiguration must answer BadRequestException, got {:?}",
        answer.error_code()
    );
    let message = answer.string("Message").unwrap_or_default();
    match refusal.finding {
        Some(finding) => assert_eq!(
            message,
            schema_refusal(schema, finding, refusal.node),
            "case {case:?}: ValidateConfiguration must answer the schema-violation template, \
             got {message:?}"
        ),
        None => {
            let opening = format!("Could not build JSON schema from invalid input:\n {schema}.");
            assert!(
                message.starts_with(&opening),
                "case {case:?}: the refusal must open with the preamble and the schema that \
                 gave the verdict, got {message:?}"
            );
            assert!(
                message.ends_with(&format!("- parent node: {}]", refusal.node)),
                "case {case:?}: the refusal must name the violating node {:?}, got {message:?}",
                refusal.node
            );
        }
    }
}

/// `ValidateConfiguration` reports the verdict the profile's validators give about the named
/// version's content, and reports it without changing anything.
#[tokio::test]
async fn validate_configuration_reports_the_validators_verdict_on_the_named_version() {
    let router = router();
    let application = create_application(&router, "validated").await;
    // One environment, never deployed to: a reporting operation must leave its history
    // empty however many verdicts it gives.
    let environment = create_environment(&router, &application, "unused").await;

    let rows = [
        Row {
            case: "a profile carrying no validator has nothing to fail",
            validators: &[],
            content: br#"{"enabled":"not-a-boolean"}"#,
            refusal: None,
        },
        Row {
            case: "a profile carrying no validator admits content that is not JSON either",
            validators: &[],
            content: b"enabled = maybe",
            refusal: None,
        },
        Row {
            case: "the boolean the schema requires",
            validators: &[ENABLED_SCHEMA],
            content: br#"{"enabled":true}"#,
            refusal: None,
        },
        Row {
            case: "a string where the schema requires a boolean",
            validators: &[ENABLED_SCHEMA],
            content: br#"{"enabled":"not-a-boolean"}"#,
            refusal: Some(Refusal {
                validator: 0,
                node: "/enabled",
                finding: Some(
                    "instance type (string) does not match any allowed primitive type \
                     (allowed: [\\\"boolean\\\"])",
                ),
            }),
        },
        Row {
            case: "an array where the schema requires a boolean",
            validators: &[ENABLED_SCHEMA],
            content: br#"{"enabled":["true"]}"#,
            refusal: Some(Refusal {
                validator: 0,
                node: "/enabled",
                finding: Some(
                    "instance type (array) does not match any allowed primitive type \
                     (allowed: [\\\"boolean\\\"])",
                ),
            }),
        },
        Row {
            // A number is reported with the evaluator's own naming of the instance's type,
            // so the row asserts the template around it rather than inventing that name.
            case: "a number where the schema requires a boolean",
            validators: &[ENABLED_SCHEMA],
            content: br#"{"enabled":1}"#,
            refusal: Some(Refusal {
                validator: 0,
                node: "/enabled",
                finding: None,
            }),
        },
        Row {
            case: "the member the schema requires missing",
            validators: &[ENABLED_SCHEMA],
            content: br#"{"disabled":true}"#,
            refusal: Some(Refusal {
                validator: 0,
                node: ROOT_NODE,
                finding: None,
            }),
        },
        Row {
            case: "a violation one level down the document",
            validators: &[NESTED_SCHEMA],
            content: br#"{"db":{"port":"5432"}}"#,
            refusal: Some(Refusal {
                validator: 0,
                node: "/db/port",
                finding: Some(
                    "instance type (string) does not match any allowed primitive type \
                     (allowed: [\\\"integer\\\"])",
                ),
            }),
        },
        Row {
            // Content that is not a JSON document is handed to the schema as the JSON
            // string its text is, so the embedded diagnostic is the evaluator's own type
            // report rather than a sentence about the document being unreadable.
            case: "content that is not a JSON document at all",
            validators: &[ENABLED_SCHEMA],
            content: b"enabled = maybe",
            refusal: Some(Refusal {
                validator: 0,
                node: ROOT_NODE,
                finding: Some(
                    "instance type (string) does not match any allowed primitive type \
                     (allowed: [\\\"object\\\"])",
                ),
            }),
        },
        Row {
            case: "two validators and both admit the candidate",
            validators: &[ANY_SCHEMA, ENABLED_SCHEMA],
            content: br#"{"enabled":false}"#,
            refusal: None,
        },
        Row {
            case: "two validators and only the second refuses the candidate",
            validators: &[ANY_SCHEMA, ENABLED_SCHEMA],
            content: br#"{"enabled":"not-a-boolean"}"#,
            refusal: Some(Refusal {
                validator: 1,
                node: "/enabled",
                finding: Some(
                    "instance type (string) does not match any allowed primitive type \
                     (allowed: [\\\"boolean\\\"])",
                ),
            }),
        },
        Row {
            case: "two validators and the first one already refuses the candidate",
            validators: &[ENABLED_SCHEMA, NESTED_SCHEMA],
            content: br#"{"enabled":"not-a-boolean","db":{"port":"5432"}}"#,
            refusal: Some(Refusal {
                validator: 0,
                node: "/enabled",
                finding: Some(
                    "instance type (string) does not match any allowed primitive type \
                     (allowed: [\\\"boolean\\\"])",
                ),
            }),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // One profile per row, so each row's version numbering starts in the same place.
        // Resources are named by the row's index, because `Name` is `length 1..64`.
        let profile = given_profile(
            &router,
            case,
            &application,
            &format!("validated-{index}"),
            row.validators,
        )
        .await;
        let version = given_version(&router, case, &application, &profile, row.content, None).await;

        let reported = validate_configuration(&router, &application, &profile, &version).await;
        match &row.refusal {
            None => assert_admitted(case, &reported),
            Some(refusal) => assert_refused(
                case,
                &reported,
                row.validators[refusal.validator],
                refusal,
            ),
        }

        // The verdict changed nothing: the version is still stored verbatim, the profile
        // still holds exactly the one version, nothing was deployed, and asking again
        // answers exactly the same.
        assert_eq!(
            stored_content(&router, &application, &profile, &version)
                .await
                .as_ref(),
            row.content,
            "case {case:?}: the validated version must still be stored verbatim"
        );
        assert_eq!(
            stored_versions(&router, &application, &profile).await,
            1,
            "case {case:?}: reporting a verdict must neither store nor drop a version"
        );
        assert!(
            deployment_numbers(&router, &application, &environment)
                .await
                .is_empty(),
            "case {case:?}: reporting a verdict must deploy nothing, got {:?}",
            deployment_numbers(&router, &application, &environment).await
        );
        let again = validate_configuration(&router, &application, &profile, &version).await;
        assert_eq!(
            (again.status, again.body.as_ref()),
            (reported.status, reported.body.as_ref()),
            "case {case:?}: the same verdict must be reported again, got {} / {}",
            again.status,
            String::from_utf8_lossy(&again.body)
        );
    }
}

/// A row of the version-naming table: one stored version, named both ways.
struct NamedVersion {
    case: &'static str,
    content: &'static [u8],
    label: &'static str,
    /// The node the refusal must name, or `None` when the candidate is admitted.
    refused_at: Option<&'static str>,
}

/// `ConfigurationVersion` is "the version of the configuration to validate", which a caller
/// spells either as the version number or as the version label the stored version carries:
/// the verdict is about the CONTENT, so both spellings of the same version report the same
/// one.
#[tokio::test]
async fn the_verdict_is_the_same_whichever_way_the_version_is_named() {
    let router = router();
    let application = create_application(&router, "validated-by-label").await;

    let rows = [
        NamedVersion {
            case: "a labelled version the schema admits",
            content: br#"{"enabled":true}"#,
            label: "is-boolean",
            refused_at: None,
        },
        NamedVersion {
            case: "a labelled version the schema refuses",
            content: br#"{"enabled":"not-a-boolean"}"#,
            label: "not-boolean",
            refused_at: Some("/enabled"),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let profile = given_profile(
            &router,
            case,
            &application,
            &format!("labelled-{index}"),
            &[ENABLED_SCHEMA],
        )
        .await;
        let version = given_version(
            &router,
            case,
            &application,
            &profile,
            row.content,
            Some(row.label),
        )
        .await;

        let by_number = validate_configuration(&router, &application, &profile, &version).await;
        let by_label = validate_configuration(&router, &application, &profile, row.label).await;
        match row.refused_at {
            None => {
                assert_admitted(case, &by_number);
                assert_admitted(case, &by_label);
            }
            Some(node) => {
                let refusal = Refusal {
                    validator: 0,
                    node,
                    finding: None,
                };
                assert_refused(case, &by_number, ENABLED_SCHEMA, &refusal);
                assert_refused(case, &by_label, ENABLED_SCHEMA, &refusal);
            }
        }
        assert_eq!(
            (by_label.status, by_label.body.as_ref()),
            (by_number.status, by_number.body.as_ref()),
            "case {case:?}: naming the version by its label must report the verdict naming it \
             by its number reports, got {} / {}",
            by_label.status,
            String::from_utf8_lossy(&by_label.body)
        );
    }
}
