// SPDX-License-Identifier: CC0-1.0

//! The OTHER wire format this service declares, driven end to end.
//!
//! The model declares `rpcv2Cbor` beside `awsJson1_1`
//! (`smithy_query(query="service_info")`), so a client may call every operation
//! over `POST /service/CodeDeploy_20141006/operation/<Operation>` with
//! `smithy-protocol: rpc-v2-cbor` and a CBOR body.
//!
//! RULE under test: **a request is served in the wire format it ARRIVED in, and
//! that format changes only the envelope — the operation it names, the state it
//! writes and reads, and the failure it earns are the same ones the other format
//! answers.** A service that declares several protocols is selected by its signing
//! name, which is the same for all of them, so each request carries its own
//! format; nothing about the answer may depend on which one the model happens to
//! declare first.
//!
//! The rows differ along what the rule generalises over: the OPERATION called (a
//! create, two reads and a list), the compute PLATFORM of the resource, the
//! DIRECTION a value crosses the two formats (written on one and read on the
//! other, both ways), success against failure, and the LAYER that answers — a
//! handler's modelled error and a refusal this service words ahead of the handler,
//! which has to name the operation out of the request before any handler runs.

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use serde_json::{json, Value};

use service_com_amazonaws_codedeploy_codedeploy_20141006::ops::types::{
    CreateApplicationInput, CreateApplicationOutput, CreateDeploymentGroupInput,
    CreateDeploymentGroupOutput, EC2TagFilter, GetApplicationInput, GetApplicationOutput,
    GetDeploymentGroupInput, GetDeploymentGroupOutput, ListApplicationsInput,
    ListApplicationsOutput, CREATE_APPLICATION_INPUT_SCHEMA, CREATE_DEPLOYMENT_GROUP_INPUT_SCHEMA,
    GET_APPLICATION_INPUT_SCHEMA, GET_DEPLOYMENT_GROUP_INPUT_SCHEMA, LIST_APPLICATIONS_INPUT_SCHEMA,
};

use crate::codedeploy_lifecycle::Emulator;

/// The service role every deployment group here is created with.
const ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The member a CBOR error map names the failure's class under.
const TYPE_MEMBER: &str = "__type";

/// The member the service's error shapes carry their sentence under.
const MESSAGE_MEMBER: &str = "message";

/// `input` as the CBOR request body a client sends: written through the protocol's
/// own codec against the operation's input schema, so nothing here encodes CBOR by
/// hand.
fn request_body<I: SerializableStruct>(input_schema: &Schema, input: &I) -> bytes::Bytes {
    let codec = CborCodec::default();
    let mut serializer = codec.create_serializer();
    serializer
        .write_struct(input_schema, input)
        .expect("the input shape serializes");
    bytes::Bytes::from(serializer.finish())
}

/// A successful answer decoded against its own output schema, as a client decodes
/// one.
fn decoded<O: DeserializableStruct>(case: &str, body: &[u8]) -> O {
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(body);
    O::deserialize(&mut deserializer).unwrap_or_else(|error| {
        panic!("case {case:?}: the answer must decode against its output schema, got {error}")
    })
}

/// The TEXT entries of the CBOR map `body` carries, in the order the map carries
/// them.
///
/// Read with the maintained CBOR decoder rather than against a schema, because the
/// members of an answered ERROR map are not known from any shape: the class is
/// named under a member no error shape declares. An item that is not text is read
/// past, so a shape carrying a member that is not a string still reads.
fn error_entries(case: &str, body: &[u8]) -> Vec<(String, String)> {
    fn entries(body: &[u8]) -> Option<Vec<(String, String)>> {
        let mut decoder = minicbor::Decoder::new(body);
        // A map of a declared length, or an indefinite one closed by a break item:
        // an encoder is free to write either.
        let declared = decoder.map().ok()?;
        let mut entries = Vec::new();
        let mut read = 0_u64;
        loop {
            match declared {
                Some(count) if read == count => break,
                None if matches!(decoder.datatype().ok()?, minicbor::data::Type::Break) => {
                    decoder.skip().ok()?;
                    break;
                }
                _ => {}
            }
            let key = decoder.str().ok()?.to_string();
            if matches!(decoder.datatype().ok()?, minicbor::data::Type::String) {
                entries.push((key, decoder.str().ok()?.to_string()));
            } else {
                decoder.skip().ok()?;
            }
            read = read.saturating_add(1);
        }
        Some(entries)
    }

    entries(body)
        .unwrap_or_else(|| panic!("case {case:?}: the failure must answer a CBOR map, got {body:?}"))
}

/// The value of one entry of a CBOR error map.
fn entry(entries: &[(String, String)], member: &str) -> Option<String> {
    entries
        .iter()
        .find(|(key, _)| key == member)
        .map(|(_, value)| value.clone())
}

/// The class an answer in the JSON envelope names, which carries it under the same
/// member the CBOR map does.
fn json_class(body: &Value) -> Option<String> {
    body[TYPE_MEMBER]
        .as_str()
        .map(|named| named.rsplit(['#', ':']).next().unwrap_or(named).to_string())
}

/// RULE: a create and the reads of what it wrote are served on the CBOR route, and
/// the resource they report is the one in the store — whichever route wrote it.
///
/// Rows differ along the compute platform of the application, which is what the
/// create carries beyond its name.
#[tokio::test]
async fn an_application_written_on_the_cbor_route_reads_back_on_both_routes() {
    let emulator = Emulator::new();

    // Every platform an application can be created for
    // (`smithy_query(query="shape",shape="ComputePlatform")`: Server | Lambda |
    // ECS).
    for (index, platform) in ["Server", "Lambda", "ECS"].into_iter().enumerate() {
        let case = format!("a {platform} application");
        let application = format!("cbor-application-{index:02}");

        let (status, answered) = emulator
            .cbor_call(
                "CreateApplication",
                request_body(
                    CREATE_APPLICATION_INPUT_SCHEMA,
                    &CreateApplicationInput {
                        application_name: application.clone(),
                        compute_platform: Some(platform.to_string()),
                        ..CreateApplicationInput::default()
                    },
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApplication must succeed on the CBOR route, got {status}"
        );
        let created: CreateApplicationOutput = decoded(&case, &answered);
        let application_id = created.application_id.clone().unwrap_or_else(|| {
            panic!("case {case:?}: the create must report the application id, got {created:?}")
        });

        let (status, answered) = emulator
            .cbor_call(
                "GetApplication",
                request_body(
                    GET_APPLICATION_INPUT_SCHEMA,
                    &GetApplicationInput {
                        application_name: application.clone(),
                    },
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetApplication must succeed on the CBOR route, got {status}"
        );
        let read: GetApplicationOutput = decoded(&case, &answered);
        let info = read.application.clone().unwrap_or_else(|| {
            panic!("case {case:?}: the read must report the application, got {read:?}")
        });
        assert_eq!(
            info.application_id.as_deref(),
            Some(application_id.as_str()),
            "case {case:?}: expected the id the create minted, got {info:?}"
        );
        assert_eq!(
            info.compute_platform.as_deref(),
            Some(platform),
            "case {case:?}: expected the platform the create named, got {info:?}"
        );

        // The SAME resource through the other route: one store behind two
        // envelopes.
        let over_json = emulator
            .ok("GetApplication", json!({"applicationName": application}))
            .await;
        assert_eq!(
            over_json["application"]["applicationId"].as_str(),
            Some(application_id.as_str()),
            "case {case:?}: the JSON route must report the same id, got {over_json:?}"
        );
        assert_eq!(
            over_json["application"]["computePlatform"].as_str(),
            Some(platform),
            "case {case:?}: the JSON route must report the same platform, got {over_json:?}"
        );

        // And a list on the CBOR route holds what either route wrote.
        let (status, answered) = emulator
            .cbor_call(
                "ListApplications",
                request_body(
                    LIST_APPLICATIONS_INPUT_SCHEMA,
                    &ListApplicationsInput::default(),
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListApplications must succeed on the CBOR route, got {status}"
        );
        let listed: ListApplicationsOutput = decoded(&case, &answered);
        let names = listed.applications.clone().unwrap_or_default();
        assert!(
            names.contains(&application),
            "case {case:?}: expected {application:?} among the listed applications, got {names:?}"
        );
    }
}

/// RULE, for a resource written on ONE route and read on the OTHER, both ways: a
/// deployment group crosses the two envelopes unchanged, because the envelope is
/// not where the group lives.
#[tokio::test]
async fn a_deployment_group_crosses_both_routes_in_either_direction() {
    let emulator = Emulator::new();

    // (case, the platform, whether the group is CREATED on the CBOR route)
    let cases: &[(&str, &str, bool)] = &[
        ("a Server group written on the CBOR route", "Server", true),
        ("a Lambda group written on the CBOR route", "Lambda", true),
        ("a Server group written on the JSON route", "Server", false),
        ("a Lambda group written on the JSON route", "Lambda", false),
    ];

    for (index, (case, platform, written_over_cbor)) in cases.iter().enumerate() {
        let application = format!("cbor-group-{index:02}");
        emulator
            .ok(
                "CreateApplication",
                json!({"applicationName": application, "computePlatform": platform}),
            )
            .await;
        let group = format!("{application}-dg");

        let group_id = if *written_over_cbor {
            let (status, answered) = emulator
                .cbor_call(
                    "CreateDeploymentGroup",
                    request_body(
                        CREATE_DEPLOYMENT_GROUP_INPUT_SCHEMA,
                        &CreateDeploymentGroupInput {
                            application_name: application.clone(),
                            deployment_group_name: group.clone(),
                            service_role_arn: ROLE.to_string(),
                            ..CreateDeploymentGroupInput::default()
                        },
                    ),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateDeploymentGroup must succeed on the CBOR route, got {status}"
            );
            let created: CreateDeploymentGroupOutput = decoded(case, &answered);
            created.deployment_group_id.clone().unwrap_or_else(|| {
                panic!("case {case:?}: the create must report the group id, got {created:?}")
            })
        } else {
            let created = emulator
                .ok(
                    "CreateDeploymentGroup",
                    json!({
                        "applicationName": application,
                        "deploymentGroupName": group,
                        "serviceRoleArn": ROLE,
                    }),
                )
                .await;
            created["deploymentGroupId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the create must report the group id, got {created:?}")
                })
                .to_string()
        };

        let (status, answered) = emulator
            .cbor_call(
                "GetDeploymentGroup",
                request_body(
                    GET_DEPLOYMENT_GROUP_INPUT_SCHEMA,
                    &GetDeploymentGroupInput {
                        application_name: application.clone(),
                        deployment_group_name: group.clone(),
                    },
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetDeploymentGroup must succeed on the CBOR route, got {status}"
        );
        let read: GetDeploymentGroupOutput = decoded(case, &answered);
        let info = read.deployment_group_info.clone().unwrap_or_else(|| {
            panic!("case {case:?}: the read must report the group, got {read:?}")
        });
        assert_eq!(
            info.deployment_group_id.as_deref(),
            Some(group_id.as_str()),
            "case {case:?}: expected the id the create minted, got {info:?}"
        );
        assert_eq!(
            info.service_role_arn.as_deref(),
            Some(ROLE),
            "case {case:?}: expected the role the create named, got {info:?}"
        );
        assert_eq!(
            info.compute_platform.as_deref(),
            Some(*platform),
            "case {case:?}: expected the application's platform, got {info:?}"
        );

        let over_json = emulator
            .ok(
                "GetDeploymentGroup",
                json!({"applicationName": application, "deploymentGroupName": group}),
            )
            .await;
        assert_eq!(
            over_json["deploymentGroupInfo"]["deploymentGroupId"].as_str(),
            Some(group_id.as_str()),
            "case {case:?}: the JSON route must report the same id, got {over_json:?}"
        );
    }
}

/// RULE, for the failing half: a failure of a CBOR call is the SAME failure the
/// JSON route answers — the class and its sentence — carried in the CBOR map a
/// client of that route decodes.
///
/// Rows differ along the LAYER that answers: a handler's modelled miss, and the
/// refusal this service words ahead of the handler for a request whose service role
/// it judges before the body (that layer has to name the operation out of the
/// request itself, which is a different read on each route).
#[tokio::test]
async fn a_failure_on_the_cbor_route_is_the_failure_the_json_route_answers() {
    let emulator = Emulator::new();
    let application = "cbor-failure";
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": "Server"}),
        )
        .await;

    // A tag filter whose `type` is outside the modelled `EC2TagFilterType`
    // (`smithy_query(query="shape",shape="EC2TagFilterType")`: KEY_ONLY |
    // VALUE_ONLY | KEY_AND_VALUE), which the boundary validator refuses before any
    // handler runs — beside a role that is no ARN, which this service judges first.
    let off_enum_filter = EC2TagFilter {
        key: Some("k".to_string()),
        value: Some("v".to_string()),
        r#type: Some("BOGUS_TYPE".to_string()),
    };

    // A read of an application nothing holds: the handler's own modelled miss.
    let (json_status, json_body) = emulator
        .call(
            "GetApplication",
            json!({"applicationName": "no-such-application"}),
        )
        .await;
    let (cbor_status, cbor_answered) = emulator
        .cbor_call(
            "GetApplication",
            request_body(
                GET_APPLICATION_INPUT_SCHEMA,
                &GetApplicationInput {
                    application_name: "no-such-application".to_string(),
                },
            ),
        )
        .await;
    assert_same_failure(
        "a read of an application nothing holds",
        "ApplicationDoesNotExistException",
        (json_status, &json_body),
        (cbor_status, &cbor_answered),
    );

    // A request whose body the boundary refuses AND whose role earns a refusal:
    // the role is the answer, which only the layer ahead of the handler can give.
    let refused_body = json!({
        "applicationName": application,
        "deploymentGroupName": "cbor-role-order",
        "serviceRoleArn": "not-a-role-arn",
        "ec2TagFilters": [{"Key": "k", "Value": "v", "Type": "BOGUS_TYPE"}],
    });
    let (json_status, json_body) = emulator.call("CreateDeploymentGroup", refused_body).await;
    let (cbor_status, cbor_answered) = emulator
        .cbor_call(
            "CreateDeploymentGroup",
            request_body(
                CREATE_DEPLOYMENT_GROUP_INPUT_SCHEMA,
                &CreateDeploymentGroupInput {
                    application_name: application.to_string(),
                    deployment_group_name: "cbor-role-order".to_string(),
                    service_role_arn: "not-a-role-arn".to_string(),
                    ec2_tag_filters: Some(vec![off_enum_filter]),
                    ..CreateDeploymentGroupInput::default()
                },
            ),
        )
        .await;
    assert_same_failure(
        "a refused body beside a role that is no ARN",
        "InvalidRoleException",
        (json_status, &json_body),
        (cbor_status, &cbor_answered),
    );
}

/// Assert that the two routes answered ONE failure — the same status, the same
/// class, and the same sentence — and that the class is the one the operation
/// models for it, so a pair that agreed on something else cannot pass.
fn assert_same_failure(
    case: &str,
    expected_class: &str,
    over_json: (u16, &Value),
    over_cbor: (u16, &[u8]),
) {
    let (json_status, json_body) = over_json;
    let (cbor_status, cbor_answered) = over_cbor;
    assert_eq!(
        json_class(json_body).as_deref(),
        Some(expected_class),
        "case {case:?}: expected the JSON route to answer {expected_class}, got {json_body:?}"
    );
    assert_eq!(
        json_status, 400,
        "case {case:?}: expected the modelled 400, got {json_status} with {json_body:?}"
    );
    assert_eq!(
        cbor_status, json_status,
        "case {case:?}: expected the status the JSON route answered ({json_status}), got \
         {cbor_status}"
    );
    let entries = error_entries(case, cbor_answered);
    assert_eq!(
        entry(&entries, TYPE_MEMBER)
            .map(|named| named.rsplit(['#', ':']).next().unwrap_or(&named).to_string()),
        json_class(json_body),
        "case {case:?}: expected the class the JSON route named ({:?}), got {entries:?}",
        json_class(json_body)
    );
    assert_eq!(
        entry(&entries, MESSAGE_MEMBER).as_deref(),
        json_body[MESSAGE_MEMBER].as_str(),
        "case {case:?}: expected the sentence the JSON route stated ({:?}), got {entries:?}",
        json_body[MESSAGE_MEMBER].as_str()
    );
}
