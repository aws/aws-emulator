// SPDX-License-Identifier: CC0-1.0

//! Narrowing and widening what one configuration recorder records, end to end over the
//! wire.
//!
//! The rule pinned here: `AssociateResourceTypes` and `DisassociateResourceTypes` change
//! the resource types a recorder records by changing the LIST its recording strategy
//! expresses that scope in — an inclusion group lists the types it records, so a type
//! joins or leaves that list; an exclusion group records every type it does not list, so
//! a type LEAVES the exclusion list to become recorded and joins it to stop being
//! recorded — and a recorder whose strategy enumerates nothing
//! (`ALL_SUPPORTED_RESOURCE_TYPES`) has no such list, so the change is refused 400
//! `ValidationException`. A type the recorder already treats the way the request asks for
//! is refused the same way: there is no change to make. The recorder each call answers is
//! the one `DescribeConfigurationRecorders` then reports, and a refused call changes
//! nothing.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — using the
//! `Emulator` harness `config_setup_and_rule_loop` defines.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

/// Entries of `com.amazonaws.configservice#ResourceType`, from
/// `smithy_query(query="shape", shape="ResourceType")`.
const EC2_VOLUME: &str = "AWS::EC2::Volume";
/// See [`EC2_VOLUME`].
const S3_BUCKET: &str = "AWS::S3::Bucket";
/// See [`EC2_VOLUME`].
const DYNAMODB_TABLE: &str = "AWS::DynamoDB::Table";
/// See [`EC2_VOLUME`]: a type no case starts the recorder out recording explicitly.
const LAMBDA_FUNCTION: &str = "AWS::Lambda::Function";

/// Every published `RecordingStrategyType` value, from
/// `smithy_query(query="shape", shape="RecordingStrategyType")`.
const ALL_SUPPORTED: &str = "ALL_SUPPORTED_RESOURCE_TYPES";
/// See [`ALL_SUPPORTED`].
const INCLUSION: &str = "INCLUSION_BY_RESOURCE_TYPES";
/// See [`ALL_SUPPORTED`].
const EXCLUSION: &str = "EXCLUSION_BY_RESOURCE_TYPES";

/// The name Config assigns a customer managed recorder put without one
/// (`ConfigurationRecorder$name`).
const RECORDER: &str = "default";

/// A recording group that RECORDS the types in `included` and nothing else.
fn inclusion_group(included: &[&str]) -> Value {
    json!({
        "allSupported": false,
        "resourceTypes": included,
        "recordingStrategy": { "useOnly": INCLUSION },
    })
}

/// A recording group that records every supported type EXCEPT those in `excluded`.
fn exclusion_group(excluded: &[&str]) -> Value {
    json!({
        "allSupported": false,
        "exclusionByResourceTypes": { "resourceTypes": excluded },
        "recordingStrategy": { "useOnly": EXCLUSION },
    })
}

/// A recording group that records every supported type, which enumerates none of them.
fn all_supported_group() -> Value {
    json!({ "allSupported": true, "recordingStrategy": { "useOnly": ALL_SUPPORTED } })
}

/// The scope a recording group expresses, as a comparable projection: the strategy it
/// records under, the types it includes and the types it excludes.
///
/// A projection rather than a whole-document comparison, because the members a group
/// carries at their modelled default are free to be omitted on the wire while the scope
/// it expresses is not.
fn scope(case: &str, recorder: &Value) -> (String, Vec<String>, Vec<String>) {
    let group = &recorder["recordingGroup"];
    let names = |value: &Value| -> Vec<String> {
        value
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| entry.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    (
        group["recordingStrategy"]["useOnly"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the recorder must report a recording strategy, got {recorder}")
            })
            .to_string(),
        names(&group["resourceTypes"]),
        names(&group["exclusionByResourceTypes"]["resourceTypes"]),
    )
}

/// An emulator holding one customer managed recorder with the recording group `group`,
/// and that recorder's ARN.
async fn emulator_recording(group: Value) -> (Emulator, String) {
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN, "recordingGroup": group })),
        )
        .await;
    let described = emulator
        .ok("DescribeConfigurationRecorders", &json!({}))
        .await;
    let arn = described["ConfigurationRecorders"][0]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("the recorder must carry an ARN, got {described}"))
        .to_string();
    (emulator, arn)
}

/// The recorder `DescribeConfigurationRecorders` reports for this account.
async fn described_recorder(emulator: &Emulator) -> Value {
    let described = emulator
        .ok("DescribeConfigurationRecorders", &json!({}))
        .await;
    described["ConfigurationRecorders"][0].clone()
}

/// The two sentences `ValidationException` carries on these operations.
///
/// RECORDED behaviour words the strategy refusal: it is ONE sentence covering both
/// directions and carries no part of the request, so every such refusal reads the same.
/// The already-settled sentence is the one the model states for exactly these two
/// operations (`smithy_query(query="doc", shape="ValidationException")`: "For
/// AssociateResourceTypes and DisassociateResourceTypes, one of the following errors:
/// … One or more of the specified resource types are already associated or disassociated
/// with the configuration recorder"), reproduced verbatim.
const STRATEGY_FORBIDS: &str = "The operation failed. configuration recorder may have a recording \
                                strategy that does not permit association or disassociation of \
                                resource types.";
/// See [`STRATEGY_FORBIDS`].
const ALREADY_SETTLED: &str = "One or more of the specified resource types are already associated \
                               or disassociated with the configuration recorder.";

/// What one row expects of the call it makes.
enum Expected {
    /// The call succeeds and leaves the recorder recording this scope: the strategy, the
    /// included types and the excluded types.
    Records(&'static str, Vec<&'static str>, Vec<&'static str>),
    /// The call is refused with this error code and message, and changes nothing.
    Refused(&'static str, &'static str),
}

/// One row: the operation it calls, the group the recorder starts out with, the types
/// the request names, and what it expects.
struct Case {
    name: &'static str,
    operation: &'static str,
    group: Value,
    resource_types: Vec<&'static str>,
    expected: Expected,
}

#[tokio::test]
async fn associating_and_disassociating_resource_types_moves_the_strategys_own_list() {
    // Rows differ along what the rule generalises over: which direction the change is,
    // which strategy the recorder records under, how many types one request names,
    // whether the named type is one the recorder already treats that way, whether the
    // strategy's list is empty to begin with, and whether the group states the strategy
    // it records under or only implies it.
    let cases = vec![
        Case {
            name: "an inclusion recorder takes on another type",
            operation: "AssociateResourceTypes",
            group: inclusion_group(&[EC2_VOLUME]),
            resource_types: vec![S3_BUCKET],
            expected: Expected::Records(INCLUSION, vec![EC2_VOLUME, S3_BUCKET], vec![]),
        },
        Case {
            name: "an inclusion recorder takes on two types at once",
            operation: "AssociateResourceTypes",
            group: inclusion_group(&[EC2_VOLUME]),
            resource_types: vec![S3_BUCKET, DYNAMODB_TABLE],
            expected: Expected::Records(
                INCLUSION,
                vec![EC2_VOLUME, S3_BUCKET, DYNAMODB_TABLE],
                vec![],
            ),
        },
        Case {
            name: "an inclusion recorder already records the named type",
            operation: "AssociateResourceTypes",
            group: inclusion_group(&[EC2_VOLUME, S3_BUCKET]),
            resource_types: vec![S3_BUCKET],
            expected: Expected::Refused("ValidationException", ALREADY_SETTLED),
        },
        Case {
            name: "one request names the same type twice",
            operation: "AssociateResourceTypes",
            group: inclusion_group(&[EC2_VOLUME]),
            resource_types: vec![S3_BUCKET, S3_BUCKET],
            expected: Expected::Refused("ValidationException", ALREADY_SETTLED),
        },
        Case {
            name: "an exclusion recorder stops excluding the named type",
            operation: "AssociateResourceTypes",
            group: exclusion_group(&[S3_BUCKET, DYNAMODB_TABLE]),
            resource_types: vec![S3_BUCKET],
            expected: Expected::Records(EXCLUSION, vec![], vec![DYNAMODB_TABLE]),
        },
        Case {
            name: "an exclusion recorder already records the named type",
            operation: "AssociateResourceTypes",
            group: exclusion_group(&[S3_BUCKET]),
            resource_types: vec![EC2_VOLUME],
            expected: Expected::Refused("ValidationException", ALREADY_SETTLED),
        },
        Case {
            name: "a recorder recording every supported type",
            operation: "AssociateResourceTypes",
            group: all_supported_group(),
            resource_types: vec![EC2_VOLUME],
            expected: Expected::Refused("ValidationException", STRATEGY_FORBIDS),
        },
        Case {
            name: "an inclusion recorder gives up one of its types",
            operation: "DisassociateResourceTypes",
            group: inclusion_group(&[EC2_VOLUME, S3_BUCKET]),
            resource_types: vec![S3_BUCKET],
            expected: Expected::Records(INCLUSION, vec![EC2_VOLUME], vec![]),
        },
        Case {
            name: "an inclusion recorder gives up every type it had",
            operation: "DisassociateResourceTypes",
            group: inclusion_group(&[EC2_VOLUME, S3_BUCKET]),
            resource_types: vec![EC2_VOLUME, S3_BUCKET],
            expected: Expected::Records(INCLUSION, vec![], vec![]),
        },
        Case {
            name: "an inclusion recorder does not record the named type",
            operation: "DisassociateResourceTypes",
            group: inclusion_group(&[EC2_VOLUME]),
            resource_types: vec![LAMBDA_FUNCTION],
            expected: Expected::Refused("ValidationException", ALREADY_SETTLED),
        },
        Case {
            name: "an exclusion recorder starts excluding another type",
            operation: "DisassociateResourceTypes",
            group: exclusion_group(&[S3_BUCKET]),
            resource_types: vec![EC2_VOLUME],
            expected: Expected::Records(EXCLUSION, vec![], vec![S3_BUCKET, EC2_VOLUME]),
        },
        Case {
            name: "an exclusion recorder excluding nothing yet",
            operation: "DisassociateResourceTypes",
            group: exclusion_group(&[]),
            resource_types: vec![LAMBDA_FUNCTION],
            expected: Expected::Records(EXCLUSION, vec![], vec![LAMBDA_FUNCTION]),
        },
        Case {
            name: "an exclusion recorder already excludes the named type",
            operation: "DisassociateResourceTypes",
            group: exclusion_group(&[S3_BUCKET]),
            resource_types: vec![S3_BUCKET],
            expected: Expected::Refused("ValidationException", ALREADY_SETTLED),
        },
        Case {
            name: "a recorder recording every supported type gives one up",
            operation: "DisassociateResourceTypes",
            group: all_supported_group(),
            resource_types: vec![EC2_VOLUME],
            expected: Expected::Refused("ValidationException", STRATEGY_FORBIDS),
        },
        Case {
            name: "a recorder recording every supported type without stating the strategy",
            operation: "AssociateResourceTypes",
            group: json!({ "allSupported": true }),
            resource_types: vec![EC2_VOLUME, S3_BUCKET],
            expected: Expected::Refused("ValidationException", STRATEGY_FORBIDS),
        },
        Case {
            name: "a recorder recording under the strategy alone, enumerating nothing",
            operation: "DisassociateResourceTypes",
            group: json!({ "recordingStrategy": { "useOnly": ALL_SUPPORTED } }),
            resource_types: vec![LAMBDA_FUNCTION],
            expected: Expected::Refused("ValidationException", STRATEGY_FORBIDS),
        },
    ];

    for case in cases {
        let (emulator, arn) = emulator_recording(case.group.clone()).await;
        let before = scope(case.name, &described_recorder(&emulator).await);
        let request = json!({
            "ConfigurationRecorderArn": arn,
            "ResourceTypes": case.resource_types,
        });

        match case.expected {
            Expected::Records(strategy, included, excluded) => {
                let expected = (
                    strategy.to_string(),
                    included.iter().map(|name| (*name).to_string()).collect(),
                    excluded.iter().map(|name| (*name).to_string()).collect(),
                );
                let answered = emulator.ok(case.operation, &request).await;
                let reported = scope(case.name, &answered["ConfigurationRecorder"]);
                assert_eq!(
                    reported, expected,
                    "case {:?}: expected {} to answer the recorder recording {expected:?}, got \
                     {reported:?} in {answered}",
                    case.name, case.operation,
                );

                // The recorder the call answered is the recorder the account now holds:
                // the change is PERSISTED, not just reported back.
                let read_back = scope(case.name, &described_recorder(&emulator).await);
                assert_eq!(
                    read_back, expected,
                    "case {:?}: expected DescribeConfigurationRecorders to report the scope \
                     {expected:?} the change left, got {read_back:?}",
                    case.name,
                );
            }
            Expected::Refused(code, expected_message) => {
                let (status, parsed) = emulator.call(case.operation, &request).await;
                let actual = parsed["__type"]
                    .as_str()
                    .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
                    .unwrap_or("<none>");
                assert_eq!(
                    (status, actual),
                    (400, code),
                    "case {:?}: expected {} to refuse with 400 {code}, got {status} {parsed}",
                    case.name,
                    case.operation,
                );
                // The refusal's own sentence, which is the half of the contract a client
                // surfaces: the model states one sentence per rule broken, so each row
                // pins the one its rule carries.
                assert_eq!(
                    parsed["message"].as_str(),
                    Some(expected_message),
                    "case {:?}: expected the refusal to be worded {expected_message:?}, got \
                     {parsed}",
                    case.name,
                );

                // A refused change changes nothing about what the recorder records.
                let after = scope(case.name, &described_recorder(&emulator).await);
                assert_eq!(
                    after, before,
                    "case {:?}: a refused {} must leave the scope {before:?} untouched, got \
                     {after:?}",
                    case.name, case.operation,
                );
            }
        }
    }
}

#[tokio::test]
async fn a_recorder_arn_this_account_does_not_hold_is_refused_as_no_such_recorder() {
    let (emulator, arn) = emulator_recording(inclusion_group(&[EC2_VOLUME])).await;

    // Rows differ along HOW the ARN fails to name one of this account's recorders: a
    // well-formed ARN of another account, one of another Region, one naming another
    // recorder name, the recorder's own ARN with its assigned id changed, and a value
    // that is no ARN at all. Each is within the modelled `@length(1, 1000)`, so the
    // handler is what refuses it.
    let cases: [(&str, String); 5] = [
        (
            "another account's recorder",
            format!(
                "arn:aws:config:{}:210987654321:configuration-recorder/{RECORDER}/cr-abc123",
                emulator.region,
            ),
        ),
        (
            "another Region's recorder",
            format!(
                "arn:aws:config:eu-central-1:{}:configuration-recorder/{RECORDER}/cr-abc123",
                emulator_server::emulator_account_id(),
            ),
        ),
        (
            "another recorder name",
            arn.replace(&format!("/{RECORDER}/"), "/other-recorder/"),
        ),
        ("another recorder id", format!("{arn}-and-more")),
        ("a value that is not an ARN", "not-an-arn".to_string()),
    ];

    for (case, candidate) in cases {
        for operation in ["AssociateResourceTypes", "DisassociateResourceTypes"] {
            let (status, code) = emulator
                .refused(
                    operation,
                    &json!({
                        "ConfigurationRecorderArn": candidate,
                        "ResourceTypes": [S3_BUCKET],
                    }),
                )
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "NoSuchConfigurationRecorderException"),
                "case {case:?}: expected {operation} on {candidate:?} to be refused 400 \
                 NoSuchConfigurationRecorderException, got {status} {code}"
            );
        }
    }

    // The recorder's own ARN is what the two operations serve, so the same request that
    // was refused for every ARN above is accepted for this one.
    let answered = emulator
        .ok(
            "AssociateResourceTypes",
            &json!({ "ConfigurationRecorderArn": arn, "ResourceTypes": [S3_BUCKET] }),
        )
        .await;
    assert_eq!(
        answered["ConfigurationRecorder"]["arn"].as_str(),
        Some(arn.as_str()),
        "the recorder answered is the one the request named, got {answered}"
    );
}
