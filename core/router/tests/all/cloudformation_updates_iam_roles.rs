// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack update that re-declares an `AWS::IAM::Role` it created with a
//! different configuration applies that change to the role — in IAM.
//!
//! The rule under test: **a deployment that re-declares a resource the stack already
//! created with a changed configuration applies the change through the owning service's own
//! in-place update APIs, one call per property that service can change on an existing
//! resource, and the stack reaches `UPDATE_COMPLETE` with the resource's identity
//! untouched.** For `AWS::IAM::Role` the role's identity is its NAME, so every property
//! except the name is applied to the role the stack created — the trust policy through
//! `UpdateAssumeRolePolicy`, the description and the session length through `UpdateRole`,
//! the inline policies through `PutRolePolicy`/`DeleteRolePolicy`, the attached managed
//! policies through `AttachRolePolicy`/`DetachRolePolicy`, and the tags through
//! `TagRole`/`UntagRole` — and the role keeps its `RoleId`, its ARN and its creation
//! instant. Two crates have to agree for that to hold (CloudFormation makes the calls, IAM
//! serves them), so the suite lives here: a crate-local test can only ever see one side of
//! it.
//!
//! The rows differ along what the rule generalises over: which property the update changes
//! (the trust policy's principal, the description, the maximum session duration, the inline
//! policies, the attached managed policies, the tags, and all of them at once), whether the
//! change re-values a declared property, ADDS one the create did not declare or DROPS one
//! the create did, how the value is written (an inline JSON object, a quoted string, a
//! number), and whether the template NAMES the role or leaves the name to CloudFormation.
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked.
//!
//! A change to the role's NAME is the other half of the same rule and is its own test: the
//! name is the role's identity, so that change is a REPLACEMENT — a new role, with a new
//! `RoleId`, and the one the stack had created deleted.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_provisions_stack_resources::{outputs_of, xml_text, xml_texts};

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms) to
/// come due. A role has no transient status of its own: IAM answers `CreateRole` with a
/// role that exists at once.
/// How long a settled stack is waited for: a generous deadline the poll below gives up at,
/// not a sleep. The stack's own transition span is the emulator's, so what a case waits for
/// is the CONDITION — a status that is no longer in progress — rather than a fixed time.
const SETTLED_DEADLINE: Duration = Duration::from_secs(60);

/// How often that poll looks again.
const SETTLED_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// IAM's own default maximum session length, which a role goes back to when a deployment
/// stops declaring `MaxSessionDuration`: `UpdateRole` documents that member as "if you do
/// not specify a value for this setting, the default value of one hour is applied".
const DEFAULT_MAX_SESSION_DURATION: &str = "3600";

/// The logical id every template here declares its role under.
const ROLE_LOGICAL_ID: &str = "Role";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&crate::query_wire::form_encode(key));
        body.push('=');
        body.push_str(&crate::query_wire::form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one IAM operation over its awsQuery wire. IAM is global, so the request is signed
/// for this suite's Region only because that is what routes it.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in members {
        body.push('&');
        body.push_str(&crate::query_wire::form_encode(key));
        body.push('=');
        body.push_str(&crate::query_wire::form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One IAM call that must succeed.
async fn iam_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = iam(router, action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from IAM {action}, got {status} with body {body}"
    );
    body
}

/// The template a row deploys: one `AWS::IAM::Role` carrying `properties`, publishing its
/// name and its ARN.
fn one_role_template(properties: &Value) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": { ROLE_LOGICAL_ID: { "Type": "AWS::IAM::Role", "Properties": properties } },
        "Outputs": {
            "RoleName": { "Value": { "Ref": ROLE_LOGICAL_ID } },
            "RoleArn": { "Value": { "Fn::GetAtt": [ROLE_LOGICAL_ID, "Arn"] } },
            "RoleId": { "Value": { "Fn::GetAtt": [ROLE_LOGICAL_ID, "RoleId"] } }
        }
    })
    .to_string()
}

/// Create a stack, wait past its transition, and answer the `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    deploy_and_settle(router, case, "CreateStack", stack_name, template, &[]).await
}

/// Update a stack with a new template, wait past its transition, and answer the
/// `DescribeStacks` body.
async fn update_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    deploy_and_settle(router, case, "UpdateStack", stack_name, template, &[]).await
}

/// One stack operation (a create or an update), waited past its transition and answered
/// with the `DescribeStacks` body. `extra` carries the stack-level members a row needs —
/// the stack's own `Tags`, which CloudFormation propagates to the resources in it.
async fn deploy_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    stack_name: &str,
    template: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut members: Vec<(&str, &str)> = vec![
        ("StackName", stack_name),
        ("TemplateBody", template),
        // The template declares an IAM resource, and `CAPABILITY_NAMED_IAM` covers a role
        // the template names as well as one CloudFormation names.
        ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
    ];
    members.extend_from_slice(extra);
    let (status, body) = cloudformation(router, action, &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    // Wait for the stack to SETTLE rather than for a span to pass: the describe is taken
    // again until the stack reports a status that is not in progress, so a loaded host only
    // makes the wait longer and never changes the answer.
    let deadline = ::std::time::Instant::now() + SETTLED_DEADLINE;
    loop {
        let body = describe(router, case, stack_name).await;
        let reported = xml_text(&body, "StackStatus").unwrap_or_default();
        if !reported.ends_with("_IN_PROGRESS") {
            return body;
        }
        assert!(
            ::std::time::Instant::now() < deadline,
            "case {case:?}: the stack never settled, last status {reported:?}"
        );
        tokio::time::sleep(SETTLED_POLL_INTERVAL).await;
    }
}

/// The one stack `stack_name` names, as the response body.
async fn describe(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// What IAM reports about a role, as the record this suite compares against a declaration.
#[derive(Debug, PartialEq, Eq)]
struct ReportedRole {
    trust_policy: String,
    description: String,
    max_session_duration: String,
    /// The role's inline policies, by `PolicyName`.
    inline_policies: BTreeMap<String, String>,
    /// The ARNs of the managed policies attached to the role.
    attached: Vec<String>,
    tags: BTreeMap<String, String>,
}

/// The role's identity, which no in-place update may move.
#[derive(Debug, PartialEq, Eq)]
struct RoleIdentity {
    role_id: String,
    arn: String,
    create_date: String,
}

/// The policy document the `tag` member of an IAM answer carries, percent-decoded back to
/// plain JSON text.
///
/// IAM reports a policy document "URL-encoded compliant with RFC 3986" and a caller "can use
/// a URL decoding method to convert the policy back to plain JSON text" (`GetRole`,
/// `GetRolePolicy`), so a comparison with the text a template declared reads it through this
/// decode.
fn policy_document(body: &str, tag: &str) -> String {
    ::smithy_server_runtime::percent_decode(&xml_text(body, tag).unwrap_or_default())
}

/// Read the role `role_name` back out of IAM, through IAM's own readers.
async fn reported_role(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
) -> (ReportedRole, RoleIdentity) {
    let body = iam_ok(router, case, "GetRole", &[("RoleName", role_name)]).await;
    let one = |tag: &str| xml_text(&body, tag).unwrap_or_default();
    let tags: BTreeMap<String, String> = xml_texts(&body, "Key")
        .into_iter()
        .zip(xml_texts(&body, "Value"))
        .collect();

    let listed = iam_ok(
        router,
        case,
        "ListRolePolicies",
        &[("RoleName", role_name)],
    )
    .await;
    let mut inline_policies = BTreeMap::new();
    for policy_name in xml_texts(&listed, "member") {
        let policy = iam_ok(
            router,
            case,
            "GetRolePolicy",
            &[("RoleName", role_name), ("PolicyName", &policy_name)],
        )
        .await;
        inline_policies.insert(
            policy_name,
            policy_document(&policy, "PolicyDocument"),
        );
    }

    let attached_body = iam_ok(
        router,
        case,
        "ListAttachedRolePolicies",
        &[("RoleName", role_name)],
    )
    .await;
    let mut attached = xml_texts(&attached_body, "PolicyArn");
    attached.sort();

    (
        ReportedRole {
            trust_policy: policy_document(&body, "AssumeRolePolicyDocument"),
            description: one("Description"),
            max_session_duration: one("MaxSessionDuration"),
            inline_policies,
            attached,
            tags,
        },
        RoleIdentity {
            role_id: one("RoleId"),
            arn: one("Arn"),
            create_date: one("CreateDate"),
        },
    )
}

/// What a `Properties` document says the role must be: the record IAM has to report for a
/// role deployed with it. Computed from the DECLARATION, so no assertion reads its expected
/// value out of the answer it is checking.
fn declared_role(properties: &Value) -> ReportedRole {
    let document = |value: Option<&Value>| match value {
        Some(Value::String(text)) => text.clone(),
        Some(composite) => composite.to_string(),
        None => String::new(),
    };
    let inline_policies = properties
        .get("Policies")
        .and_then(Value::as_array)
        .map(|policies| {
            policies
                .iter()
                .map(|policy| {
                    (
                        document(policy.get("PolicyName")),
                        document(policy.get("PolicyDocument")),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut attached: Vec<String> = properties
        .get("ManagedPolicyArns")
        .and_then(Value::as_array)
        .map(|arns| arns.iter().map(|arn| document(Some(arn))).collect())
        .unwrap_or_default();
    attached.sort();
    let tags = properties
        .get("Tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .map(|tag| (document(tag.get("Key")), document(tag.get("Value"))))
                .collect()
        })
        .unwrap_or_default();
    ReportedRole {
        trust_policy: document(properties.get("AssumeRolePolicyDocument")),
        description: document(properties.get("Description")),
        // A declaration that states no session length leaves the role at IAM's own
        // documented default.
        max_session_duration: properties
            .get("MaxSessionDuration")
            .map_or(DEFAULT_MAX_SESSION_DURATION.to_string(), |value| {
                document(Some(value))
            }),
        inline_policies,
        attached,
        tags,
    }
}

/// A trust policy trusting `principal`, written as the template writes one: an inline
/// object.
fn trust(principal: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": principal,
            "Action": "sts:AssumeRole"
        }]
    })
}

/// An inline permission policy allowing `action`.
fn permission(action: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": action, "Resource": "*"}]
    })
}

/// One row of the in-place update table: the properties the role is created with, and the
/// properties the update declares for it. What IAM must then report is computed from the
/// second ([`declared_role`]).
struct UpdateRow {
    case: &'static str,
    stack_name: &'static str,
    /// Whether the template NAMES the role; where it does, the name is the stack's, so the
    /// row's role is its own.
    named: bool,
    /// The `Properties` the create declares, given the name the row's template names the
    /// role (unused by a row that leaves the name to CloudFormation).
    created: fn(&str) -> Value,
    /// The `Properties` the update declares, given the name the role REALLY has — so a row
    /// may state the name CloudFormation generated and have it read as no change at all.
    updated: fn(&str) -> Value,
}

/// The `Properties` of a role, with `RoleName` set to `name` where the row names it.
fn with_name(name: Option<&str>, mut properties: Value) -> Value {
    if let (Some(name), Some(object)) = (name, properties.as_object_mut()) {
        object.insert("RoleName".to_string(), json!(name));
    }
    properties
}

const UPDATE_ROWS: &[UpdateRow] = &[
    UpdateRow {
        case: "the trust policy's principal replaced",
        stack_name: "trust-flip-stack",
        named: true,
        created: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument":
                        trust(json!({"AWS": format!("arn:aws:iam::{ACCOUNT_ID}:root")})),
                }),
            )
        },
        updated: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                }),
            )
        },
    },
    UpdateRow {
        case: "the trust policy of a role whose name CloudFormation generated",
        stack_name: "trust-generated-stack",
        named: false,
        created: |_| {
            json!({
                "AssumeRolePolicyDocument": trust(json!({"Service": "lambda.amazonaws.com"})),
                "Description": "the description the create carried",
            })
        },
        updated: |_| {
            json!({
                "AssumeRolePolicyDocument":
                    trust(json!({"Service": ["lambda.amazonaws.com", "ec2.amazonaws.com"]})),
                "Description": "the description the update declares",
            })
        },
    },
    UpdateRow {
        case: "the description re-valued and a session duration added",
        stack_name: "settings-stack",
        named: true,
        created: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                    "Description": "assumed by the ingest pipeline",
                }),
            )
        },
        updated: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                    "Description": "assumed by the nightly compaction job",
                    "MaxSessionDuration": 43200,
                }),
            )
        },
    },
    UpdateRow {
        case: "the session duration re-valued and the description dropped",
        stack_name: "dropped-settings-stack",
        named: true,
        created: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                    "Description": "a description the update stops declaring",
                    "MaxSessionDuration": 7200,
                }),
            )
        },
        updated: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                    "MaxSessionDuration": 3600,
                }),
            )
        },
    },
    UpdateRow {
        case: "the session duration dropped, so it goes back to the documented default",
        stack_name: "default-duration-stack",
        named: true,
        created: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                    "MaxSessionDuration": 10800,
                }),
            )
        },
        updated: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                }),
            )
        },
    },
    UpdateRow {
        case: "an inline policy's document changed, one added and one dropped",
        stack_name: "inline-policy-stack",
        named: true,
        created: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                    "Policies": [
                        {"PolicyName": "Reader", "PolicyDocument": permission("s3:GetObject")},
                        {"PolicyName": "Retired", "PolicyDocument": permission("s3:ListBucket")},
                    ],
                }),
            )
        },
        updated: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                    "Policies": [
                        {"PolicyName": "Reader", "PolicyDocument": permission("s3:PutObject")},
                        {"PolicyName": "Added", "PolicyDocument": permission("sqs:SendMessage")},
                    ],
                }),
            )
        },
    },
    UpdateRow {
        case: "a managed policy attached and another detached",
        stack_name: "managed-policy-stack",
        named: true,
        created: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                    "ManagedPolicyArns": ["arn:aws:iam::aws:policy/ReadOnlyAccess"],
                }),
            )
        },
        updated: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                    "ManagedPolicyArns": [
                        "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess",
                        "arn:aws:iam::aws:policy/AWSLambdaExecute",
                    ],
                }),
            )
        },
    },
    UpdateRow {
        case: "tags: one added, one re-valued and one dropped",
        stack_name: "role-tags-stack",
        named: true,
        created: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                    "Tags": [
                        {"Key": "team", "Value": "platform"},
                        {"Key": "env", "Value": "staging"},
                        {"Key": "retire", "Value": "soon"},
                    ],
                }),
            )
        },
        updated: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                    "Tags": [
                        {"Key": "team", "Value": "payments"},
                        {"Key": "env", "Value": "staging"},
                        {"Key": "owner", "Value": "core"},
                    ],
                }),
            )
        },
    },
    UpdateRow {
        case: "a generated name written down by the update, beside a changed description",
        stack_name: "named-later-stack",
        named: false,
        created: |_| {
            json!({
                "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                "Description": "a role CloudFormation named",
            })
        },
        // `name` here is the name the role REALLY has, so this revision states the very
        // name the create left to CloudFormation: the role keeps it, and the only change to
        // apply is the description.
        updated: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                    "Description": "the description the update declares",
                }),
            )
        },
    },
    UpdateRow {
        case: "every in-place property at once, the trust policy written as a string",
        stack_name: "everything-stack",
        named: true,
        created: |name| {
            with_name(
                Some(name),
                json!({
                    "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                    "Description": "the first configuration",
                    "MaxSessionDuration": 3600,
                    "Policies": [
                        {"PolicyName": "Reader", "PolicyDocument": permission("s3:GetObject")},
                    ],
                    "ManagedPolicyArns": ["arn:aws:iam::aws:policy/ReadOnlyAccess"],
                    "Tags": [{"Key": "team", "Value": "platform"}],
                }),
            )
        },
        updated: |name| {
            with_name(
                Some(name),
                json!({
                    // The same document a template may equally write as a quoted string,
                    // which is the other spelling CloudFormation accepts for a policy.
                    "AssumeRolePolicyDocument":
                        trust(json!({"Service": "states.amazonaws.com"})).to_string(),
                    "Description": "the second configuration",
                    "MaxSessionDuration": 21600,
                    "Policies": [
                        {"PolicyName": "Writer", "PolicyDocument": permission("s3:PutObject")},
                    ],
                    "ManagedPolicyArns": ["arn:aws:iam::aws:policy/AWSLambdaExecute"],
                    "Tags": [
                        {"Key": "team", "Value": "payments"},
                        {"Key": "tier", "Value": "gold"},
                    ],
                }),
            )
        },
    },
];

/// RULE: an update that changes an in-place property of an `AWS::IAM::Role` the stack
/// already created is APPLIED to that role and the stack reaches `UPDATE_COMPLETE`: IAM
/// reports the configuration the new template declares, and the role keeps its identity —
/// the same name, `RoleId`, ARN and creation instant — so nothing was replaced.
#[tokio::test]
async fn an_update_applies_changed_role_properties_in_place() {
    let router = build_router();

    for row in UPDATE_ROWS {
        let case = row.case;
        // The name the row's template declares for the role, where it declares one.
        let declared_name = format!("{}-role", row.stack_name);
        let created = (row.created)(&declared_name);
        let body = create_and_settle(&router, case, row.stack_name, &one_role_template(&created))
            .await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete before the update, got {body}"
        );
        let outputs = outputs_of(&body);
        let role_name = outputs
            .get("RoleName")
            .unwrap_or_else(|| panic!("case {case:?}: no RoleName output in {body}"))
            .clone();
        if row.named {
            assert_eq!(
                role_name, declared_name,
                "case {case:?}: expected the stack to create the role the template names"
            );
        } else {
            let prefix = format!("{}-{ROLE_LOGICAL_ID}-", row.stack_name);
            assert!(
                role_name.starts_with(&prefix),
                "case {case:?}: expected a generated <stack>-<logical id>-<suffix> name, got \
                 {role_name:?}"
            );
        }

        // What IAM holds before the update: the created configuration, and the identity the
        // update must not move.
        let (held, identity_before) = reported_role(&router, case, &role_name).await;
        assert_eq!(
            held,
            declared_role(&created),
            "case {case:?}: expected the created role to match its declaration"
        );

        let updated = (row.updated)(&role_name);
        let body =
            update_and_settle(&router, case, row.stack_name, &one_role_template(&updated)).await;
        let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
        assert_eq!(
            (xml_text(&body, "StackStatus").as_deref(), reason.as_str()),
            (Some("UPDATE_COMPLETE"), ""),
            "case {case:?}: expected the update to be applied in place, got {body}"
        );

        // The configuration the update declared is the configuration the role has.
        let (now, identity_after) = reported_role(&router, case, &role_name).await;
        assert_eq!(
            now,
            declared_role(&updated),
            "case {case:?}: expected IAM to report the configuration the update declares"
        );

        // The same role, not a new one: the stack still publishes the same handles, and IAM
        // reports the same unique id, ARN and creation instant.
        assert_eq!(
            identity_after, identity_before,
            "case {case:?}: expected the role the stack created, not a replacement"
        );
        let outputs_after = outputs_of(&body);
        assert_eq!(
            (
                outputs_after.get("RoleName").map(String::as_str),
                outputs_after.get("RoleArn").map(String::as_str),
                outputs_after.get("RoleId").map(String::as_str),
            ),
            (
                Some(role_name.as_str()),
                Some(identity_before.arn.as_str()),
                Some(identity_before.role_id.as_str()),
            ),
            "case {case:?}: expected the stack to keep publishing the role's own handles, got \
             {outputs_after:?}"
        );
    }
}

/// RULE: the role's NAME is its identity, so an update that changes it is a REPLACEMENT
/// rather than an in-place change — the stack completes carrying a role with a DIFFERENT
/// `RoleId`, and the role the stack had created no longer exists in IAM.
#[tokio::test]
async fn an_update_that_changes_the_role_name_replaces_the_role() {
    let router = build_router();
    let case = "the role renamed";
    let stack_name = "rename-stack";
    let first = "rename-stack-role-one";
    let second = "rename-stack-role-two";
    let properties = |name: &str| {
        with_name(
            Some(name),
            json!({
                "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                "Description": "a role whose name the update changes",
            }),
        )
    };

    let body = create_and_settle(
        &router,
        case,
        stack_name,
        &one_role_template(&properties(first)),
    )
    .await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete before the update, got {body}"
    );
    let (_, before) = reported_role(&router, case, first).await;

    let body = update_and_settle(
        &router,
        case,
        stack_name,
        &one_role_template(&properties(second)),
    )
    .await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the replacement to complete, got {body}"
    );
    let outputs = outputs_of(&body);
    assert_eq!(
        outputs.get("RoleName").map(String::as_str),
        Some(second),
        "case {case:?}: expected the stack to publish the new role's name, got {outputs:?}"
    );

    // A replacement is a NEW role: its own unique id, and the ARN of the new name.
    let (reported, after) = reported_role(&router, case, second).await;
    assert_ne!(
        after.role_id, before.role_id,
        "case {case:?}: expected a replaced role to carry a different RoleId"
    );
    assert_eq!(
        after.arn,
        format!("arn:aws:iam::{ACCOUNT_ID}:role/{second}"),
        "case {case:?}: expected the new role's ARN to be built from its own name"
    );
    assert_eq!(
        reported,
        declared_role(&properties(second)),
        "case {case:?}: expected the new role to match the declaration"
    );

    // And the role the stack had created is gone: a replacement deletes it.
    let (status, body) = iam(&router, "GetRole", &[("RoleName", first)]).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the replaced role to be deleted, got {status} with {body}"
    );
}

/// RULE: the tags a stack propagates to its resources are part of the set each resource is
/// to carry, so an update that stops DECLARING a tag key the stack itself also states
/// leaves the role carrying the stack's value for that key — only a key nothing in the
/// stack states any more is removed from the role.
#[tokio::test]
async fn a_dropped_declared_tag_falls_back_to_the_stacks_own_tag() {
    let router = build_router();
    let case = "a declared tag that is also a stack tag, dropped by the update";
    let stack_name = "stack-tags-stack";
    let role_name = "stack-tags-stack-role";
    // The stack's own tags, stated identically on the create and on the update: one key the
    // role's declaration also states (`env`) and one only the stack states (`owner`).
    let stack_tags = &[
        ("Tags.member.1.Key", "env"),
        ("Tags.member.1.Value", "staging"),
        ("Tags.member.2.Key", "owner"),
        ("Tags.member.2.Value", "platform-team"),
    ];
    let properties = |tags: Value| {
        with_name(
            Some(role_name),
            json!({
                "AssumeRolePolicyDocument": trust(json!({"Service": "ec2.amazonaws.com"})),
                "Tags": tags,
            }),
        )
    };

    let body = deploy_and_settle(
        &router,
        case,
        "CreateStack",
        stack_name,
        &one_role_template(&properties(json!([
            {"Key": "env", "Value": "production"},
            {"Key": "team", "Value": "payments"},
        ]))),
        stack_tags,
    )
    .await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete before the update, got {body}"
    );
    // The resource's own declaration is the more specific statement about the key both
    // state, and the stack's own key is carried beside it.
    let (held, identity_before) = reported_role(&router, case, role_name).await;
    assert_eq!(
        held.tags,
        [
            ("env", "production"),
            ("owner", "platform-team"),
            ("team", "payments"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect::<BTreeMap<String, String>>(),
        "case {case:?}: expected the created role to carry the declared and the stack's tags"
    );

    // The update stops declaring BOTH of the role's own tags, with the stack's unchanged.
    let body = deploy_and_settle(
        &router,
        case,
        "UpdateStack",
        stack_name,
        &one_role_template(&properties(json!([]))),
        stack_tags,
    )
    .await;
    let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
    assert_eq!(
        (xml_text(&body, "StackStatus").as_deref(), reason.as_str()),
        (Some("UPDATE_COMPLETE"), ""),
        "case {case:?}: expected the update to be applied in place, got {body}"
    );
    let (now, identity_after) = reported_role(&router, case, role_name).await;
    assert_eq!(
        now.tags,
        [("env", "staging"), ("owner", "platform-team")]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<BTreeMap<String, String>>(),
        "case {case:?}: expected the key the stack still states to fall back to the stack's \
         value and only the key nothing states to be removed"
    );
    assert_eq!(
        identity_after, identity_before,
        "case {case:?}: expected the role the stack created, not a replacement"
    );
}
