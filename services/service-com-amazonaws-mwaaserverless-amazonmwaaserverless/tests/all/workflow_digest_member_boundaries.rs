// SPDX-License-Identifier: CC0-1.0

//! End-to-end and invariant tests for the MEMBER BOUNDARIES of the digest that tells a
//! repeated `ClientToken` apart from the same token reused for a different request.
//!
//! RULE under test: the digest a request is identified by determines the request —
//! because every member whose extent depends on what the request carries (an optional
//! member, a list, a map) announces how many fields it contributes before contributing
//! them, no value supplied for one member can occupy the position another member's value
//! would have taken. So two requests that differ only in WHICH member carries a value, or
//! in how many values a list carries, are different requests: under one `ClientToken` the
//! second is the modelled `ConflictException`, and only a byte-identical request replays
//! the first answer.
//!
//! The suite has two legs:
//!
//! * a table of request PAIRS driven through `CreateWorkflow` over the wire, each pair
//!   moving the same text from one member to another, with `GetWorkflow` and
//!   `ListTagsForResource` showing the refused request stored nothing;
//! * an exhaustive sweep of the member-presence space through the ONE shared
//!   configuration walk, asserting the digest is injective over it. The sweep draws every
//!   value from a deliberately SHARED pool, so any two layouts whose fields could line up
//!   do line up on the values too — the worst case for the rule rather than a random one.

use std::collections::HashMap;

use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::ops::types::{
    Code, DefinitionS3Location, EncryptionConfiguration, Environment, LoggingConfiguration,
    NetworkConfiguration, S3Location,
};
use service_com_amazonaws_mwaaserverless_amazonmwaaserverless::state::{
    DigestFields, WorkflowConfigurationView,
};

use crate::workflow_lifecycle::{
    assert_service_default, role_arn, Harness, SERVICE_DEFAULTED_MEMBERS,
};

/// The members `GetWorkflow` reports back of what a create supplied, from
/// `smithy_query(query="operation", operation="CreateWorkflow,GetWorkflow")`. After a
/// refused second create, every one of them must still read back as the FIRST request
/// supplied it — and the two the service defaults itself
/// ([`SERVICE_DEFAULTED_MEMBERS`]) as the value it applied for that first request.
const READABLE_MEMBERS: &[&str] = &[
    "Description",
    "DefinitionS3Location",
    "Code",
    "RoleArn",
    "EncryptionConfiguration",
    "LoggingConfiguration",
    "EngineVersion",
    "NetworkConfiguration",
    "Environment",
    "TriggerMode",
];

/// The request every row starts from: only the members `CreateWorkflowRequest` marks
/// required, so each row's overlay is exactly the member layout under test.
fn base_request() -> serde_json::Value {
    serde_json::json!({
        "DefinitionS3Location": {"Bucket": "defs", "ObjectKey": "dags/base.yaml"},
        "RoleArn": role_arn(),
    })
}

/// [`base_request`] with `overlay`'s members added.
fn request_with(overlay: &serde_json::Value) -> serde_json::Value {
    let mut request = base_request();
    let members = request
        .as_object_mut()
        .expect("the base request is a JSON object");
    for (member, value) in overlay
        .as_object()
        .expect("every overlay is a JSON object")
        .iter()
    {
        members.insert(member.clone(), value.clone());
    }
    request
}

/// A `CreateWorkflow` body for `request` under `name` and `token`.
fn create_body(request: &serde_json::Value, name: &str, token: &str) -> serde_json::Value {
    let mut body = request.clone();
    let members = body.as_object_mut().expect("a request is an object");
    members.insert("Name".to_string(), serde_json::json!(name));
    members.insert("ClientToken".to_string(), serde_json::json!(token));
    body
}

/// Request PAIRS that differ only in which member carries a value: each row's two
/// requests supply the same text, in different members or split differently between two
/// lists, and each row is a pair of genuinely different requests that every modelled
/// constraint accepts. The id forms come from the element patterns the model declares
/// (`^sg-[a-z0-9]*$`, `^subnet-[a-z0-9]*$`); `LogGroupName`, `KmsKeyId` and a `Tags`
/// entry are plain strings
/// (`smithy_query(query="shape", shape="LoggingConfiguration,EncryptionConfiguration,Tags")`),
/// so they can carry that same text.
fn member_boundary_cases() -> Vec<(&'static str, serde_json::Value, serde_json::Value)> {
    vec![
        (
            "a log group name against a security group id of the same text",
            serde_json::json!({"LoggingConfiguration": {"LogGroupName": "sg-1"}}),
            serde_json::json!({"NetworkConfiguration": {"SecurityGroupIds": ["sg-1"]}}),
        ),
        (
            "a log group name against a subnet id of the same text",
            serde_json::json!({"LoggingConfiguration": {"LogGroupName": "subnet-9"}}),
            serde_json::json!({"NetworkConfiguration": {"SubnetIds": ["subnet-9"]}}),
        ),
        (
            "one tag entry against the two subnet ids of the same text",
            serde_json::json!({"Tags": {"subnet-x": "subnet-y"}}),
            serde_json::json!({"NetworkConfiguration": {
                "SubnetIds": ["subnet-x", "subnet-y"],
            }}),
        ),
        (
            "a log group name beside a subnet list, against both network lists",
            serde_json::json!({"LoggingConfiguration": {"LogGroupName": "sg-1"},
                               "NetworkConfiguration": {"SubnetIds": ["subnet-1"]}}),
            serde_json::json!({"NetworkConfiguration": {
                "SecurityGroupIds": ["sg-1"],
                "SubnetIds": ["subnet-1"],
            }}),
        ),
        (
            // `EncryptionType` is `AWS_MANAGED_KEY` or `CUSTOMER_MANAGED_KEY`
            // (`smithy_query(query="shape", shape="EncryptionType")`).
            "an encryption type and key id, against a log group and a security group id \
             of the same text",
            serde_json::json!({"LoggingConfiguration": {"LogGroupName": "AWS_MANAGED_KEY"},
                               "NetworkConfiguration": {"SecurityGroupIds": ["sg-kms"]}}),
            serde_json::json!({"EncryptionConfiguration": {"Type": "AWS_MANAGED_KEY",
                                                           "KmsKeyId": "sg-kms"}}),
        ),
        (
            "an omitted security group list against an empty one",
            serde_json::json!({"NetworkConfiguration": {"SubnetIds": ["subnet-1"]}}),
            serde_json::json!({"NetworkConfiguration": {
                "SecurityGroupIds": [],
                "SubnetIds": ["subnet-1"],
            }}),
        ),
        (
            "an omitted subnet list against an empty one",
            serde_json::json!({"NetworkConfiguration": {"SecurityGroupIds": ["sg-1"]}}),
            serde_json::json!({"NetworkConfiguration": {
                "SecurityGroupIds": ["sg-1"],
                "SubnetIds": [],
            }}),
        ),
        (
            "a description against a trigger mode of the same text",
            serde_json::json!({"Description": "ON_DEMAND"}),
            serde_json::json!({"TriggerMode": "ON_DEMAND"}),
        ),
        (
            "a code object version against none, under the same bucket and key",
            serde_json::json!({"Code": {"S3Location": {"Bucket": "code",
                                                       "ObjectKey": "base.zip"}}}),
            serde_json::json!({"Code": {"S3Location": {"Bucket": "code",
                                                       "ObjectKey": "base.zip",
                                                       "VersionId": "codeversion1"}}}),
        ),
        (
            // An environment variable NAME satisfies `^[a-zA-Z][a-zA-Z0-9_]*$`
            // (`smithy_query(query="shape", shape="EnvironmentVariablesMap")`), which a
            // tag key may also carry — `TagKey` is `^.*$`.
            "one tag entry against one environment variable of the same name and value",
            serde_json::json!({"Tags": {"AIRFLOW_HOME": "sg-1"}}),
            serde_json::json!({"Environment": {"Variables": {"AIRFLOW_HOME": "sg-1"}}}),
        ),
        (
            "an omitted variables map against an empty one",
            serde_json::json!({"Environment": {}}),
            serde_json::json!({"Environment": {"Variables": {}}}),
        ),
        (
            "two environment variables against one carrying the other's name in its value",
            serde_json::json!({"Environment": {"Variables": {"A": "1", "B": "2"}}}),
            serde_json::json!({"Environment": {"Variables": {"A": "1B2"}}}),
        ),
    ]
}

/// The tag map `ListTagsForResource` must report for a request: the `Tags` it supplied,
/// or the empty map when it supplied none.
fn expected_tags(request: &serde_json::Value) -> serde_json::Value {
    request
        .get("Tags")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}))
}

#[tokio::test]
async fn a_reused_client_token_conflicts_when_the_request_moves_a_value_to_another_member() {
    for (index, (case, first_members, second_members)) in
        member_boundary_cases().into_iter().enumerate()
    {
        // A fresh emulator per row, so one row's workflow and token ledger cannot answer
        // another row's call.
        let harness = Harness::new("us-west-2");
        let first = request_with(&first_members);
        let second = request_with(&second_members);
        assert_ne!(
            first, second,
            "case {case:?}: the row's two requests must differ, got {first:?}"
        );
        let name = format!("bounded{index}");
        let token = format!("boundary-token-{index}");

        let (status, created) = harness
            .call("CreateWorkflow", create_body(&first, &name, &token))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first CreateWorkflow to succeed, got {status} \
             {created:?}"
        );
        let arn = created["WorkflowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The byte-identical request under the same token IS the replay.
        let (status, replay) = harness
            .call("CreateWorkflow", create_body(&first, &name, &token))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the unchanged request to replay, got {status} {replay:?}"
        );
        assert_eq!(
            replay, created,
            "case {case:?}: an unchanged request must replay the first response, got \
             {replay:?}"
        );

        // The row's other request is a DIFFERENT request, so the same token is the
        // modelled conflict rather than a replay of the first workflow.
        let (status, conflict) = harness
            .call("CreateWorkflow", create_body(&second, &name, &token))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected ConflictException (409) for the same token with a \
             request that carries the value in another member, got {status} {conflict:?}"
        );
        assert!(
            conflict["__type"]
                .as_str()
                .is_some_and(|kind| kind.contains("ConflictException")),
            "case {case:?}: expected a ConflictException, got {:?}",
            conflict["__type"]
        );
        assert!(
            conflict["Message"]
                .as_str()
                .is_some_and(|message| message.contains(&token)),
            "case {case:?}: expected the conflict to name the reused ClientToken, got {:?}",
            conflict["Message"]
        );
        assert_eq!(
            conflict["ResourceId"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the conflict to name the workflow the token created, \
             got {:?}",
            conflict["ResourceId"]
        );

        // The refused request stored nothing: the workflow still carries exactly what the
        // first request supplied.
        let (status, fetched) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": arn.clone()}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflow to succeed, got {status} {fetched:?}"
        );
        for member in READABLE_MEMBERS {
            let supplied = &first[*member];
            if supplied.is_null() {
                if SERVICE_DEFAULTED_MEMBERS.contains(member) {
                    // The service applies its own value for these two, so what the
                    // workflow carries is that value rather than nothing.
                    assert_service_default(case, member, &arn, &fetched);
                    continue;
                }
                assert!(
                    fetched[*member].is_null(),
                    "case {case:?}: the stored create supplied no {member}, so the workflow \
                     must carry none, got {:?}",
                    fetched[*member]
                );
            } else {
                assert_eq!(
                    &fetched[*member], supplied,
                    "case {case:?}: GetWorkflow must report the {member} the stored create \
                     supplied, got {:?}",
                    fetched[*member]
                );
            }
        }
        let (status, tags) = harness
            .call(
                "ListTagsForResource",
                serde_json::json!({"ResourceArn": arn.clone()}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTagsForResource to succeed, got {status} {tags:?}"
        );
        assert_eq!(
            tags["Tags"],
            expected_tags(&first),
            "case {case:?}: the refused request's tags must not be stored, got {:?}",
            tags["Tags"]
        );

        // And the refused request IS an acceptable one: under its own name and token it
        // creates a workflow, so the conflict above is the digest talking and not a
        // refusal of the values it carries.
        let (status, accepted) = harness
            .call(
                "CreateWorkflow",
                create_body(&second, &format!("{name}.other"), &format!("{token}-other")),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the other request to be createable on its own, got \
             {status} {accepted:?}"
        );
        let other_arn = accepted["WorkflowArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_ne!(
            other_arn, arn,
            "case {case:?}: the two requests must be two workflows, got {other_arn:?}"
        );
        let (status, other) = harness
            .call("GetWorkflow", serde_json::json!({"WorkflowArn": other_arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetWorkflow on the second workflow to succeed, got \
             {status} {other:?}"
        );
        for member in READABLE_MEMBERS {
            let supplied = &second[*member];
            if supplied.is_null() {
                if SERVICE_DEFAULTED_MEMBERS.contains(member) {
                    assert_service_default(case, member, &other_arn, &other);
                    continue;
                }
                assert!(
                    other[*member].is_null(),
                    "case {case:?}: the second create supplied no {member}, so its workflow \
                     must carry none, got {:?}",
                    other[*member]
                );
            } else {
                assert_eq!(
                    &other[*member], supplied,
                    "case {case:?}: GetWorkflow must report the {member} the second create \
                     supplied, got {:?}",
                    other[*member]
                );
            }
        }
    }
}

/// The text every plain-string value in the sweep below carries — a description, a
/// trigger mode, a log group name, a bucket, an object key, an object version — and also
/// the one security group id the sweep uses, which `SecurityGroupString`'s
/// `^sg-[a-z0-9]*$` accepts. One text across every member that may carry it is what puts
/// two layouts whose fields could line up on the same values too.
const SHARED: &str = "sg-1";

/// The one subnet id the sweep uses, as `SubnetString`'s `^subnet-[a-z0-9]*$` requires,
/// and the KMS key id too — `KmsKeyId` is a plain string, so it can carry exactly what a
/// subnet id carries.
const SHARED_SUBNET: &str = "subnet-1";

/// `EncryptionType`'s value the sweep uses
/// (`smithy_query(query="shape", shape="EncryptionType")`: `AWS_MANAGED_KEY` or
/// `CUSTOMER_MANAGED_KEY`).
const ENCRYPTION_TYPE: &str = "CUSTOMER_MANAGED_KEY";

/// The environment variable names the sweep uses, as `EnvironmentVariableKey`'s
/// `^[a-zA-Z][a-zA-Z0-9_]*$` requires
/// (`smithy_query(query="shape", shape="EnvironmentVariablesMap")`). Two of them, so the
/// sweep covers a map of one entry and a map of two.
const VARIABLE_NAMES: &[&str] = &["A", "B"];

/// Every `Option<&str>` a sweep member walks: absent, and present carrying the shared
/// text.
const OPTIONAL_TEXTS: &[Option<&str>] = &[None, Some(SHARED)];

/// Every shape of a sweep list: absent, empty, one element, and the same element twice —
/// so a list's presence, its length and its elements are all on the axis.
fn list_shapes(element: &str) -> Vec<Option<Vec<String>>> {
    vec![
        None,
        Some(Vec::new()),
        Some(vec![element.to_string()]),
        Some(vec![element.to_string(), element.to_string()]),
    ]
}

/// One sweep layout: the optional members a request can supply, each drawn from the
/// shared pool.
struct Layout {
    description: Option<&'static str>,
    trigger_mode: Option<&'static str>,
    engine_version: Option<i32>,
    code: Option<Code>,
    encryption: Option<EncryptionConfiguration>,
    logging: Option<LoggingConfiguration>,
    network: Option<NetworkConfiguration>,
    environment: Option<Environment>,
}

impl Layout {
    /// This layout's digest, taken through the ONE walk both `CreateWorkflow` and
    /// `UpdateWorkflow` drive.
    fn digest(&self, role_arn: &str, definition: &DefinitionS3Location) -> String {
        let mut fields = DigestFields::new();
        WorkflowConfigurationView {
            role_arn,
            description: self.description,
            trigger_mode: self.trigger_mode,
            engine_version: self.engine_version,
            definition_s3_location: definition,
            code: self.code.as_ref(),
            encryption_configuration: self.encryption.as_ref(),
            logging_configuration: self.logging.as_ref(),
            network_configuration: self.network.as_ref(),
            environment: self.environment.as_ref(),
        }
        .digest_group(&mut fields);
        fields.digest()
    }

    /// The layout, named by what it supplies, for an assertion message.
    fn label(&self) -> String {
        format!(
            "description={:?} trigger_mode={:?} engine_version={:?} code={:?} \
             encryption={:?} logging={:?} network={:?} environment={:?}",
            self.description,
            self.trigger_mode,
            self.engine_version,
            self.code,
            self.encryption,
            self.logging,
            self.network,
            self.environment
        )
    }
}

/// Every combination of the member-presence space above: each optional scalar absent or
/// present, the code union absent or present with and without an object version, the
/// encryption configuration absent or present with and without a key id, the logging
/// configuration absent or present, and the network configuration absent or present with
/// each pair of list shapes, and the environment absent or present in each shape its
/// variables map takes.
fn sweep_layouts() -> Vec<Layout> {
    let mut codes: Vec<Option<Code>> = vec![None];
    for version_id in OPTIONAL_TEXTS {
        codes.push(Some(Code::S3Location(S3Location {
            bucket: SHARED.to_string(),
            object_key: SHARED.to_string(),
            version_id: version_id.map(str::to_string),
        })));
    }
    let mut encryptions: Vec<Option<EncryptionConfiguration>> = vec![None];
    for kms_key_id in [None, Some(SHARED_SUBNET)] {
        encryptions.push(Some(EncryptionConfiguration {
            r#type: ENCRYPTION_TYPE.to_string(),
            kms_key_id: kms_key_id.map(str::to_string),
        }));
    }
    let mut networks: Vec<Option<NetworkConfiguration>> = vec![None];
    for security_group_ids in list_shapes(SHARED) {
        for subnet_ids in list_shapes(SHARED_SUBNET) {
            networks.push(Some(NetworkConfiguration {
                security_group_ids: security_group_ids.clone(),
                subnet_ids,
            }));
        }
    }
    // Every shape of the variables map: the member absent, the structure present with no
    // map at all, the map empty, one entry, and two entries. An `EnvironmentVariableKey`
    // is `^[a-zA-Z][a-zA-Z0-9_]*$`
    // (`smithy_query(query="shape", shape="EnvironmentVariablesMap")`), so the names come
    // from [`VARIABLE_NAMES`] while the values carry the text every other member carries.
    let mut environments: Vec<Option<Environment>> = vec![
        None,
        Some(Environment::default()),
        Some(Environment {
            variables: Some(HashMap::new()),
        }),
    ];
    for names in [&VARIABLE_NAMES[..1], &VARIABLE_NAMES[..2]] {
        environments.push(Some(Environment {
            variables: Some(
                names
                    .iter()
                    .map(|name| ((*name).to_string(), SHARED.to_string()))
                    .collect(),
            ),
        }));
    }

    let mut layouts = Vec::new();
    for description in OPTIONAL_TEXTS {
        for trigger_mode in OPTIONAL_TEXTS {
            for engine_version in [None, Some(1)] {
                for code in &codes {
                    for encryption in &encryptions {
                        for logging in [None, Some(SHARED)] {
                            for network in &networks {
                                for environment in &environments {
                                    layouts.push(Layout {
                                        description: *description,
                                        trigger_mode: *trigger_mode,
                                        engine_version,
                                        code: code.clone(),
                                        encryption: encryption.clone(),
                                        logging: logging.map(|name| LoggingConfiguration {
                                            log_group_name: name.to_string(),
                                        }),
                                        network: network.clone(),
                                        environment: environment.clone(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    layouts
}

#[tokio::test]
async fn the_shared_configuration_digest_is_injective_over_the_member_presence_space() {
    let role = role_arn();
    let definition = DefinitionS3Location {
        bucket: SHARED.to_string(),
        object_key: SHARED.to_string(),
        version_id: None,
    };
    let layouts = sweep_layouts();
    assert!(
        layouts.len() > 1000,
        "case sweep: expected the member-presence space to be swept in full, got {} layouts",
        layouts.len()
    );

    let mut seen: HashMap<String, String> = HashMap::new();
    for layout in &layouts {
        let label = layout.label();
        let digest = layout.digest(&role, &definition);
        if let Some(previous) = seen.insert(digest.clone(), label.clone()) {
            panic!(
                "case {label}: two different configurations share a digest, the other \
                 being {previous} — both digest to {digest}"
            );
        }
        // The same configuration always digests the same, which is what makes an
        // identical repeat a replay.
        assert_eq!(
            layout.digest(&role, &definition),
            digest,
            "case {label}: the same configuration must digest the same, got a second \
             digest of {:?}",
            layout.digest(&role, &definition)
        );
    }
    assert_eq!(
        seen.len(),
        layouts.len(),
        "case sweep: expected one digest per layout, got {} for {} layouts",
        seen.len(),
        layouts.len()
    );
}
