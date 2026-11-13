// SPDX-License-Identifier: CC0-1.0

//! A configuration profile's `JSON_SCHEMA` validator is EVALUATED against the candidate
//! configuration, and its verdict gates the call that would put that configuration into
//! service.
//!
//! The rule under test, in one sentence: "a validator provides a syntactic or semantic
//! check to ensure the configuration that you want to deploy functions as intended. To
//! validate your application configuration data, you provide a schema or an Amazon Web
//! Services Lambda function that runs against the configuration. The configuration
//! deployment or update can only proceed when the configuration data is valid"
//! (`smithy_query(query="doc",shape="Validator")`) — so a `JSON_SCHEMA` validator's
//! `Content` is a schema the candidate version's content is evaluated against at every call
//! that validates a configuration, a candidate the schema does not admit refuses that call
//! with `BadRequestException` (HTTP 400) and a message naming the violating node, and the
//! refusal leaves everything as it was: no deployment record, the environment not left
//! deploying, the stored version still stored and still served.
//!
//! The validators that decide a call are the ones the profile carries WHEN the call is
//! made, so a schema attached after a version was stored gates the next deployment of that
//! already-stored version, and clearing the validators lets the identical deployment
//! through again.
//!
//! Every row drives real HTTP requests through the router the emulator's front door uses.
//!
//! The tables span what the rule generalises over rather than one recorded schema: the
//! keyword that fails (`type` at a member, `type` at the document root, `required`, a
//! numeric bound), where in the document it fails, the JSON type of the offending value,
//! whether the candidate is JSON at all, and — for the bounded row — each bound the schema
//! publishes and one value past it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, json_call, router, send, Answer,
};

/// A draft-07 schema requiring an integer `replicas` member: the shape of a schema an
/// operator attaches to guard a configuration.
const REPLICAS_SCHEMA: &str = r#"{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{"replicas":{"type":"integer"}},"required":["replicas"]}"#;

/// A schema whose own published bounds the table below exercises — each bound and one value
/// past it. The bounds are this schema document's, not a service limit.
const PORT_SCHEMA: &str =
    r#"{"type":"object","properties":{"port":{"type":"integer","minimum":1,"maximum":65535}}}"#;

/// A schema that admits any JSON document at all.
const ANY_SCHEMA: &str = "{}";

/// A schema requiring the document to be an array.
const ARRAY_SCHEMA: &str = r#"{"type":"array"}"#;

/// A schema requiring the document to be a string, which is what content that is not a
/// JSON document is handed to the schema as.
const STRING_SCHEMA: &str = r#"{"type":"string"}"#;

/// A schema that states its constraint through a `$ref` into its own `$defs`, which is how
/// an operator reuses a definition — the shape the reference bound below is about.
const REFERENCED_SCHEMA: &str = r##"{"$schema":"http://json-schema.org/draft-07/schema#","$defs":{"count":{"type":"integer"}},"type":"object","properties":{"replicas":{"$ref":"#/$defs/count"}}}"##;

/// The node a violation of the document as a whole is reported at.
const ROOT_NODE: &str = "/";

/// What a row expects of the call its content drives.
enum Expect {
    /// The candidate satisfies the schema, so the call proceeds.
    Admitted,
    /// The candidate does not, so the call is refused. `node` is the node the refusal must
    /// name; `finding` is the full finding text when this error class has recorded wording
    /// for it, and `None` when the evaluator's own report is what the refusal carries.
    Refused {
        node: &'static str,
        finding: Option<&'static str>,
    },
}

/// One row: a schema, a candidate configuration and what the gate must do with it.
struct Row {
    case: &'static str,
    schema: &'static str,
    content: &'static [u8],
    expect: Expect,
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

/// Assert `answer` is the modelled 400 refusal carrying the schema-violation report the row
/// expects.
fn assert_schema_refusal(
    case: &str,
    operation: &str,
    answer: &Answer,
    schema: &str,
    node: &str,
    finding: Option<&str>,
) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: {operation} must answer the modelled 400, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.error_code().contains("BadRequestException"),
        "case {case:?}: {operation} must answer BadRequestException, got {:?}",
        answer.error_code()
    );
    let message = answer.string("Message").unwrap_or_default();
    match finding {
        Some(finding) => assert_eq!(
            message,
            schema_refusal(schema, finding, node),
            "case {case:?}: {operation} must answer the schema-violation template, got \
             {message:?}"
        ),
        None => {
            let opening = format!("Could not build JSON schema from invalid input:\n {schema}.");
            assert!(
                message.starts_with(&opening),
                "case {case:?}: {operation} must open with the preamble and the schema it ran, \
                 got {message:?}"
            );
            assert!(
                message.ends_with(&format!("- parent node: {node}]")),
                "case {case:?}: {operation} must name the violating node {node:?}, got \
                 {message:?}"
            );
        }
    }
}

/// A hosted configuration profile carrying `validators`. Answers its minted id.
async fn given_profile(
    router: &MultiServiceRouter,
    application: &str,
    name: &str,
    validators: &[Value],
) -> String {
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
        "case {name:?}: CreateConfigurationProfile answered {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {name:?}: CreateConfigurationProfile answered no Id"))
}

/// One `JSON_SCHEMA` entry of a profile's `Validators` list.
fn schema_validator(schema: &str) -> Value {
    json!({ "Type": "JSON_SCHEMA", "Content": schema })
}

/// Replace a profile's `Validators` through `UpdateConfigurationProfile` and answer what it
/// reported.
async fn set_validators(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    profile: &str,
    validators: &[Value],
) -> Answer {
    let answer = json_call(
        router,
        "PATCH",
        &format!("/applications/{application}/configurationprofiles/{profile}"),
        &json!({ "Validators": validators }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: UpdateConfigurationProfile must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
}

/// Write one hosted configuration version and answer the version number it was given.
///
/// Asserted to be accepted: the storage of a version is not what a profile's validators
/// gate here, so a version a validator would refuse to deploy is stored verbatim.
async fn given_version(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    profile: &str,
    content: &[u8],
) -> String {
    let created = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        content.to_vec(),
        &[],
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

/// A deployment strategy with nothing to wait for, so a deployment that is accepted is
/// `COMPLETE` when `StartDeployment` answers. Answers its id.
async fn given_strategy(router: &MultiServiceRouter, name: &str) -> String {
    let created = json_call(
        router,
        "POST",
        "/deploymentstrategies",
        &json!({
            "Name": name,
            "DeploymentDurationInMinutes": 0,
            "FinalBakeTimeInMinutes": 0,
            "GrowthFactor": 100.0,
            "GrowthType": "LINEAR",
            "ReplicateTo": "NONE",
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {name:?}: CreateDeploymentStrategy answered {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {name:?}: the strategy answered no Id"))
}

/// `StartDeployment` of one configuration version.
async fn start_deployment(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
    strategy: &str,
    version: &str,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": strategy,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": version,
        }),
    )
    .await
}

/// `ValidateConfiguration` of one configuration version, which reports the same verdict
/// without putting anything into service.
async fn validate_configuration(
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

/// `GetEnvironment`'s `State`.
async fn environment_state(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
) -> Option<String> {
    json_call(
        router,
        "GET",
        &format!("/applications/{application}/environments/{environment}"),
        &Value::Null,
    )
    .await
    .string("State")
}

/// The bytes `GetConfiguration` serves for one triple, which is what the data plane of this
/// environment is delivering.
async fn served_configuration(
    router: &MultiServiceRouter,
    application: &str,
    environment: &str,
    profile: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!(
            "/applications/{application}/environments/{environment}/configurations/{profile}\
             ?client_id=schema-gate"
        ),
        "application/json",
        Vec::new(),
        &[],
    )
    .await
}

/// A `JSON_SCHEMA` validator decides a deployment on the candidate configuration: the
/// content the schema admits deploys, the content it does not is refused with the
/// schema-violation report and leaves nothing behind, and `ValidateConfiguration` reports
/// the same verdict.
#[tokio::test]
async fn a_json_schema_validator_decides_a_deployment_on_the_candidate_content() {
    let router = router();
    let application = create_application(&router, "schema-gated").await;
    let strategy = given_strategy(&router, "schema-gate-instant").await;

    let rows = [
        Row {
            case: "a string where the schema requires an integer",
            schema: REPLICAS_SCHEMA,
            content: br#"{"replicas":"three"}"#,
            expect: Expect::Refused {
                node: "/replicas",
                finding: Some(
                    "instance type (string) does not match any allowed primitive type \
                     (allowed: [\\\"integer\\\"])",
                ),
            },
        },
        Row {
            case: "a boolean where the schema requires an integer",
            schema: REPLICAS_SCHEMA,
            content: br#"{"replicas":true}"#,
            expect: Expect::Refused {
                node: "/replicas",
                finding: Some(
                    "instance type (boolean) does not match any allowed primitive type \
                     (allowed: [\\\"integer\\\"])",
                ),
            },
        },
        Row {
            case: "the integer the schema requires",
            schema: REPLICAS_SCHEMA,
            content: br#"{"replicas":3}"#,
            expect: Expect::Admitted,
        },
        Row {
            case: "the required member missing",
            schema: REPLICAS_SCHEMA,
            content: br#"{"other":1}"#,
            expect: Expect::Refused {
                node: ROOT_NODE,
                finding: None,
            },
        },
        Row {
            case: "an array where the schema requires an object",
            schema: REPLICAS_SCHEMA,
            content: br#"[{"replicas":3}]"#,
            expect: Expect::Refused {
                node: ROOT_NODE,
                finding: Some(
                    "instance type (array) does not match any allowed primitive type \
                     (allowed: [\\\"object\\\"])",
                ),
            },
        },
        Row {
            case: "a value at the schema's published lower bound",
            schema: PORT_SCHEMA,
            content: br#"{"port":1}"#,
            expect: Expect::Admitted,
        },
        Row {
            case: "a value at the schema's published upper bound",
            schema: PORT_SCHEMA,
            content: br#"{"port":65535}"#,
            expect: Expect::Admitted,
        },
        Row {
            case: "a value one past the schema's lower bound",
            schema: PORT_SCHEMA,
            content: br#"{"port":0}"#,
            expect: Expect::Refused {
                node: "/port",
                finding: None,
            },
        },
        Row {
            case: "a value one past the schema's upper bound",
            schema: PORT_SCHEMA,
            content: br#"{"port":65536}"#,
            expect: Expect::Refused {
                node: "/port",
                finding: None,
            },
        },
        Row {
            case: "an array for a schema that requires one",
            schema: ARRAY_SCHEMA,
            content: br#"["alpha","beta"]"#,
            expect: Expect::Admitted,
        },
        Row {
            case: "a value a referenced definition admits",
            schema: REFERENCED_SCHEMA,
            content: br#"{"replicas":3}"#,
            expect: Expect::Admitted,
        },
        Row {
            case: "a value a referenced definition does not admit",
            schema: REFERENCED_SCHEMA,
            content: br#"{"replicas":"three"}"#,
            expect: Expect::Refused {
                node: "/replicas",
                finding: Some(
                    "instance type (string) does not match any allowed primitive type \
                     (allowed: [\\\"integer\\\"])",
                ),
            },
        },
        Row {
            case: "any document for a schema that constrains nothing",
            schema: ANY_SCHEMA,
            content: br#"{"db":{"host":"h","port":5432}}"#,
            expect: Expect::Admitted,
        },
        Row {
            // Content that is not a JSON document is handed to the schema as the JSON
            // string its text is, so the finding is the evaluator's own type report:
            // a string where this schema requires an object.
            case: "content that is not a JSON document at all",
            schema: REPLICAS_SCHEMA,
            content: b"allow_caching = true",
            expect: Expect::Refused {
                node: ROOT_NODE,
                finding: Some(
                    "instance type (string) does not match any allowed primitive type \
                     (allowed: [\\\"object\\\"])",
                ),
            },
        },
        Row {
            // The same content against a schema that constrains nothing: a JSON string
            // satisfies it, so there is no verdict to refuse the call with.
            case: "content that is not a JSON document, for a schema that constrains nothing",
            schema: ANY_SCHEMA,
            content: b"allow_caching = true",
            expect: Expect::Admitted,
        },
        Row {
            // Text the schema's own type admits: the string is the instance, so a
            // string-typed schema takes it.
            case: "content that is not a JSON document, for a schema requiring a string",
            schema: STRING_SCHEMA,
            content: b"allow_caching = true",
            expect: Expect::Admitted,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // One environment and one profile per row, so each row's deployment numbering
        // starts in the same place. Resources are named by the row's index, because `Name`
        // is `length 1..64`.
        let environment = create_environment(&router, &application, &format!("env-{index}")).await;
        let profile = given_profile(
            &router,
            &application,
            &format!("gated-{index}"),
            &[schema_validator(row.schema)],
        )
        .await;
        let version = given_version(&router, case, &application, &profile, row.content).await;

        let answer = start_deployment(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            &version,
        )
        .await;
        let reported = validate_configuration(&router, &application, &profile, &version).await;

        match row.expect {
            Expect::Admitted => {
                assert_eq!(
                    answer.status,
                    201,
                    "case {case:?}: the schema admits this content, so the deployment must \
                     proceed, got {} / {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    deployment_numbers(&router, &application, &environment).await,
                    vec![1],
                    "case {case:?}: the deployment that proceeded belongs in the history"
                );
                assert_eq!(
                    reported.status,
                    204,
                    "case {case:?}: ValidateConfiguration must report the same verdict, got \
                     {} / {}",
                    reported.status,
                    String::from_utf8_lossy(&reported.body)
                );
            }
            Expect::Refused { node, finding } => {
                assert_schema_refusal(
                    case,
                    "StartDeployment",
                    &answer,
                    row.schema,
                    node,
                    finding,
                );
                assert_schema_refusal(
                    case,
                    "ValidateConfiguration",
                    &reported,
                    row.schema,
                    node,
                    finding,
                );
                // The refusal left nothing behind: no deployment record, the environment
                // not left deploying, and the non-compliant version still stored.
                assert!(
                    deployment_numbers(&router, &application, &environment)
                        .await
                        .is_empty(),
                    "case {case:?}: a refused deployment leaves no record, got {:?}",
                    deployment_numbers(&router, &application, &environment).await
                );
                assert_eq!(
                    environment_state(&router, &application, &environment)
                        .await
                        .as_deref(),
                    Some("ReadyForDeployment"),
                    "case {case:?}: a refused deployment must not leave the environment \
                     deploying"
                );
                let stored = json_call(
                    &router,
                    "GET",
                    &format!(
                        "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions/{version}"
                    ),
                    &Value::Null,
                )
                .await;
                assert_eq!(
                    stored.body.as_ref(),
                    row.content,
                    "case {case:?}: the refused version must still be stored verbatim, got {:?}",
                    String::from_utf8_lossy(&stored.body)
                );

                // The gate is the validator and nothing else: with the validators cleared,
                // the identical deployment of the identical version proceeds.
                set_validators(&router, case, &application, &profile, &[]).await;
                let ungated = start_deployment(
                    &router,
                    &application,
                    &environment,
                    &profile,
                    &strategy,
                    &version,
                )
                .await;
                assert_eq!(
                    ungated.status,
                    201,
                    "case {case:?}: with no validator left nothing gates the deployment, got \
                     {} / {}",
                    ungated.status,
                    String::from_utf8_lossy(&ungated.body)
                );
                assert_eq!(
                    ungated.json()["DeploymentNumber"].as_i64(),
                    Some(1),
                    "case {case:?}: the refusal must consume no deployment number, got {}",
                    ungated.json()
                );
            }
        }
    }
}

/// A validator attached AFTER a version was stored gates the next deployment of that
/// already-stored version: the deployment that succeeded before the schema existed is
/// refused once it is attached, the data plane keeps serving what it was serving, and
/// clearing the validators lets the identical deployment through again.
///
/// Rows differ along the stored content and the schema attached to it, so the rule is
/// asserted for a type violation at a member and for one at the document root rather than
/// for one recorded pair.
#[tokio::test]
async fn a_validator_attached_after_a_version_is_stored_gates_the_next_deployment_of_it() {
    let router = router();
    let application = create_application(&router, "late-validator").await;
    let strategy = given_strategy(&router, "late-validator-instant").await;

    let rows = [
        (
            "a stored string where the schema later requires an integer",
            REPLICAS_SCHEMA,
            &br#"{"replicas":"three"}"#[..],
            "/replicas",
            "instance type (string) does not match any allowed primitive type (allowed: \
             [\\\"integer\\\"])",
        ),
        (
            "a stored object where the schema later requires an array",
            ARRAY_SCHEMA,
            &br#"{"replicas":3}"#[..],
            ROOT_NODE,
            "instance type (object) does not match any allowed primitive type (allowed: \
             [\\\"array\\\"])",
        ),
    ];

    for (index, (case, schema, content, node, finding)) in rows.into_iter().enumerate() {
        let environment = create_environment(&router, &application, &format!("env-{index}")).await;
        // The profile carries NO validator while the version is stored and deployed.
        let profile = given_profile(&router, &application, &format!("late-{index}"), &[]).await;
        let version = given_version(&router, case, &application, &profile, content).await;

        let first = start_deployment(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            &version,
        )
        .await;
        assert_eq!(
            first.status,
            201,
            "case {case:?}: with no validator attached the deployment must proceed, got {} / {}",
            first.status,
            String::from_utf8_lossy(&first.body)
        );
        // The positive control: the data plane serves exactly the deployed bytes.
        let served = served_configuration(&router, &application, &environment, &profile).await;
        assert_eq!(
            served.body.as_ref(),
            content,
            "case {case:?}: the deployed bytes must be served, got {:?}",
            String::from_utf8_lossy(&served.body)
        );

        // The schema is attached now, and `GetConfigurationProfile` confirms it.
        let patched = set_validators(
            &router,
            case,
            &application,
            &profile,
            &[schema_validator(schema)],
        )
        .await;
        assert_eq!(
            patched.json()["Validators"],
            json!([schema_validator(schema)]),
            "case {case:?}: the attached validator must be reported, got {}",
            patched.json()
        );

        // The IDENTICAL deployment is now refused, and nothing moves.
        let refused = start_deployment(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            &version,
        )
        .await;
        assert_schema_refusal(
            case,
            "StartDeployment after the validator was attached",
            &refused,
            schema,
            node,
            Some(finding),
        );
        assert_eq!(
            deployment_numbers(&router, &application, &environment).await,
            vec![1],
            "case {case:?}: the refused deployment must add no record"
        );
        assert_eq!(
            environment_state(&router, &application, &environment)
                .await
                .as_deref(),
            Some("ReadyForDeployment"),
            "case {case:?}: the refused deployment must not leave the environment deploying"
        );
        let still_served =
            served_configuration(&router, &application, &environment, &profile).await;
        assert_eq!(
            still_served.body.as_ref(),
            content,
            "case {case:?}: the data plane must keep serving the deployed bytes, got {:?}",
            String::from_utf8_lossy(&still_served.body)
        );

        // Clearing the validators lets the identical deployment through again.
        set_validators(&router, case, &application, &profile, &[]).await;
        let again = start_deployment(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            &version,
        )
        .await;
        assert_eq!(
            again.status,
            201,
            "case {case:?}: with the validators cleared the deployment must proceed, got {} / {}",
            again.status,
            String::from_utf8_lossy(&again.body)
        );
        let mut history = deployment_numbers(&router, &application, &environment).await;
        history.sort();
        assert_eq!(
            history,
            vec![1, 2],
            "case {case:?}: only the two deployments that proceeded belong in the history"
        );
    }
}

/// A `JSON_SCHEMA` validator whose `Content` is no schema this evaluator can compile gives
/// NO verdict, so it vetoes nothing: a refusal reported for it would be a verdict no
/// validator gave.
/// Rows differ along why there is no schema to apply: a `Content` that is not a JSON
/// document, one that is JSON but not a schema this evaluator can build, and one whose
/// `$ref` names a resource outside its own document — which resolves to nothing, because
/// this emulator resolves no schema resource beyond the validator's own.
#[tokio::test]
async fn a_content_that_is_no_compilable_schema_gives_no_verdict() {
    let router = router();
    let application = create_application(&router, "uncompilable-schema").await;
    let strategy = given_strategy(&router, "uncompilable-instant").await;

    let rows = [
        ("a Content that is not a JSON document", "not a schema"),
        ("a schema whose type keyword is not a type", r#"{"type":7}"#),
        (
            "a schema referencing a resource outside its own document",
            r#"{"$ref":"https://example.invalid/schema.json"}"#,
        ),
    ];

    for (index, (case, schema)) in rows.into_iter().enumerate() {
        let environment = create_environment(&router, &application, &format!("env-{index}")).await;
        let profile = given_profile(
            &router,
            &application,
            &format!("noschema-{index}"),
            &[schema_validator(schema)],
        )
        .await;
        let version =
            given_version(&router, case, &application, &profile, br#"{"replicas":3}"#).await;
        let answer = start_deployment(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            &version,
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {case:?}: no verdict was produced, so nothing may veto the deployment, got \
             {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            deployment_numbers(&router, &application, &environment).await,
            vec![1],
            "case {case:?}: the deployment that proceeded belongs in the history"
        );
    }
}

/// A schema whose own SHAPE is outside what the evaluator runs — the nested-combinator
/// document whose cost grows with its depth rather than with its size, and the
/// reference-duplicating one whose cost grows with neither — does not run unbounded: the
/// check answers inside its bounds and, having NOT shown the configuration valid, refuses
/// the deployment with this service's own validator-rejection message and leaves no record.
///
/// The three bounds the rows step past are the EMULATOR's own (`state::validators`:
/// `MAX_SCHEMA_DEPTH` = 24 nesting levels, `MAX_SCHEMA_NODES` = 4096 JSON nodes,
/// `MAX_SCHEMA_REFS` = 8 `$ref` keywords), not published AppConfig limits, so each row is
/// built just past one of them while staying inside `Validator$Content`'s modelled
/// `length 0..32768`. A schema an operator writes is far inside all three, which the suites
/// above exercise — including one that states its constraint through a `$ref`.
#[tokio::test]
async fn a_schema_beyond_the_evaluators_bounds_refuses_the_deployment_without_running_it() {
    let router = router();
    let application = create_application(&router, "schema-bounds").await;
    let strategy = given_strategy(&router, "schema-bounds-instant").await;

    /// `levels` nested `anyOf` combinators around one object schema: each level adds two
    /// levels of JSON nesting, so this steps past the depth bound while staying small.
    fn nested_combinators(levels: usize) -> String {
        let mut schema = String::new();
        for _ in 0..levels {
            schema.push_str(r#"{"anyOf":["#);
        }
        schema.push_str(r#"{"type":"object"}"#);
        for _ in 0..levels {
            schema.push_str("]}");
        }
        schema
    }

    /// One `anyOf` holding `branches` empty subschemas: shallow, but past the node bound.
    fn wide_combinator(branches: usize) -> String {
        let members = vec!["{}"; branches].join(",");
        format!(r#"{{"anyOf":[{members}]}}"#)
    }

    /// A chain of `$defs` subschemas, each naming the next TWICE by `$ref`: shallow and
    /// small, and the one shape whose cost is not bounded by its depth or its size, because
    /// each reference is another path through the same subschema.
    fn reference_chain(links: usize) -> String {
        let mut definitions: Vec<String> = Vec::new();
        for link in 0..links {
            let next = if link + 1 == links {
                r#"{"type":"object"}"#.to_string()
            } else {
                let target = link + 1;
                format!(
                    r##"{{"anyOf":[{{"$ref":"#/$defs/d{target}"}},{{"$ref":"#/$defs/d{target}"}}]}}"##
                )
            };
            definitions.push(format!(r#""d{link}":{next}"#));
        }
        format!(
            r##"{{"$defs":{{{}}},"$ref":"#/$defs/d0"}}"##,
            definitions.join(",")
        )
    }

    let rows = [
        (
            "a schema nested deeper than the evaluator's depth bound",
            nested_combinators(20),
        ),
        (
            "a schema holding more nodes than the evaluator's node bound",
            wide_combinator(4200),
        ),
        (
            "a schema whose references can unfold past the evaluator's reference bound",
            reference_chain(8),
        ),
    ];

    for (index, (case, schema)) in rows.into_iter().enumerate() {
        assert!(
            schema.len() <= 32768,
            "case {case:?}: the schema must stay inside Validator$Content's modelled \
             length 0..32768, got {}",
            schema.len()
        );
        let environment = create_environment(&router, &application, &format!("env-{index}")).await;
        let profile = given_profile(
            &router,
            &application,
            &format!("bounded-{index}"),
            &[schema_validator(&schema)],
        )
        .await;
        let version =
            given_version(&router, case, &application, &profile, br#"{"replicas":3}"#).await;

        let answer = start_deployment(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            &version,
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: a check this emulator cannot run must refuse the deployment, got \
             {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {}",
            answer.error_code()
        );
        let message = answer.string("Message").unwrap_or_default();
        let expected = format!(
            "Configuration version '{version}' of configuration profile '{profile}' did not \
             pass the JSON_SCHEMA validator: "
        );
        assert!(
            message.starts_with(&expected),
            "case {case:?}: expected a refusal opening {expected:?}, got {message:?}"
        );
        assert!(
            message.contains("was not shown valid"),
            "case {case:?}: the refusal must say the configuration was not shown valid, got \
             {message:?}"
        );
        assert!(
            !message.contains("anyOf"),
            "case {case:?}: the schema is a sensitive member and must not be echoed by this \
             refusal, got {message:?}"
        );
        assert!(
            deployment_numbers(&router, &application, &environment)
                .await
                .is_empty(),
            "case {case:?}: a refused deployment leaves no record, got {:?}",
            deployment_numbers(&router, &application, &environment).await
        );

        // The gate is the validator's shape and nothing else: with the validators cleared,
        // the identical deployment of the identical version proceeds.
        set_validators(&router, case, &application, &profile, &[]).await;
        let ungated = start_deployment(
            &router,
            &application,
            &environment,
            &profile,
            &strategy,
            &version,
        )
        .await;
        assert_eq!(
            ungated.status,
            201,
            "case {case:?}: with no validator left nothing gates the deployment, got {} / {}",
            ungated.status,
            String::from_utf8_lossy(&ungated.body)
        );
    }
}
