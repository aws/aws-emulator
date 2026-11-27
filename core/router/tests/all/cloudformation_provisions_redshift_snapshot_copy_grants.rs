// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::Redshift::SnapshotCopyGrant` deploys: the
//! grant is created in Amazon Redshift's own control plane under the name the stack publishes,
//! deleting the stack takes the grant away — and a declaration naming a Key Management Service
//! key that service cannot read fails the RESOURCE with Amazon Redshift's own refusal and
//! rolls the stack back.
//!
//! The rule under test: **a resource type AWS publishes is one a template may declare, and a
//! declaration of it is deployed by calling the service that OWNS the type — so the handle the
//! stack publishes (`Ref`) resolves in that service, and what that service refuses is a FAILED
//! RESOURCE rather than a template-format error.** Three crates have to agree for that to hold
//! (CloudFormation submits the template and makes the call, Amazon Redshift serves it, and Key
//! Management Service answers for the key that grant covers), so the suite lives here: a
//! crate-local test can only ever see one side of it.
//!
//! The rows of the deploy table differ along what the rule generalises over: how the
//! declaration names the key (a key id, a key ARN, no key at all — which is the account's
//! default key), whether it NAMES the grant or leaves the name to CloudFormation, whether it
//! carries the type's own `Tags`, the template spelling (JSON with long-form intrinsics, YAML
//! with the short-form tags), and how many grants one stack declares.
//!
//! The refusal rows differ along the FORM the unreadable key is named in (a bare key id, a key
//! ARN, an alias), each a key nothing in this account ever created. Each is the owning
//! service's own refusal, reported on the resource that failed — never the `Template format
//! error: Unrecognized resource types` a type CloudFormation does not know is refused with,
//! which no row of either table may ever see.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{block_of, elements, form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region the stack's resources
/// belong to.
const REGION: &str = "us-east-1";

/// How long the suite is willing to wait for a stack operation to settle, and how often it
/// looks again. Generous: the stack's own clock-owed transition has to come due, and the suite
/// waits for the CONDITION (the stack's status) rather than for a fixed time.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);
const LOOK_AGAIN_AFTER: Duration = Duration::from_millis(100);

/// The fixed text CloudFormation refuses a template declaring a type it does NOT know with,
/// which is what the defect this suite pins answered for a type AWS publishes.
const UNRECOGNIZED_TYPES: &str = "Unrecognized resource types";

/// The resource type under test, and the property it takes its name from.
const GRANT: &str = "AWS::Redshift::SnapshotCopyGrant";
const NAME_PROPERTY: &str = "SnapshotCopyGrantName";

/// The key a grant that names none covers: "If no key is specified, the default key is used"
/// (`CreateSnapshotCopyGrant$KmsKeyId`), which for snapshot copies is that service's own
/// Amazon-Web-Services-managed key.
const DEFAULT_KEY: &str = "alias/aws/redshift";

/// How long a snapshot copy grant's name may be, and the alphabet it is held to: "Must contain
/// from 1 to 63 alphanumeric characters or hyphens. Alphabetic characters must be lowercase.
/// First character must be a letter. Cannot end with a hyphen or contain two consecutive
/// hyphens." (`smithy_query(query="doc", shape="CreateSnapshotCopyGrantMessage",
/// member="SnapshotCopyGrantName")`.)
const MAX_GRANT_NAME_CHARACTERS: usize = 63;

/// The `X-Amz-Target` prefix of Key Management Service's awsJson1_1 wire.
const KMS_TARGET_PREFIX: &str = "TrentService";

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
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
        service_com_amazonaws_redshift_redshiftserviceversion20121201::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one query-protocol operation of `service`, at `version`, with these form members.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one Amazon Redshift operation over its awsQuery wire.
async fn redshift(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "redshift", "2012-12-01", action, members).await
}

/// Call one Key Management Service operation on its awsJson1_1 wire.
async fn kms(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("kms.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{KMS_TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization("kms"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
}

/// A symmetric encryption key this account really holds, created through KMS's own
/// `CreateKey`: the pair `(key id, key ARN)`, which are the two identifiers a declaration may
/// name it by.
async fn create_key(router: &MultiServiceRouter, description: &str) -> (String, String) {
    let (status, created) = kms(router, "CreateKey", &json!({ "Description": description })).await;
    assert_eq!(
        status, 200,
        "case {description:?}: expected CreateKey to succeed, got {status} with {created}"
    );
    let metadata = &created["KeyMetadata"];
    let key_id = metadata["KeyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {description:?}: no KeyId in {created}"))
        .to_string();
    let arn = metadata["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {description:?}: no Arn in {created}"))
        .to_string();
    (key_id, arn)
}

/// The key identifier Key Management Service resolves `named` to, read from its own
/// `DescribeKey`: the ONE key id behind a key id, a key ARN, an alias name or an alias ARN,
/// which is what a snapshot copy grant covers when its declaration named NO key and there is
/// no supplied identifier for it to report.
async fn resolved_key_id(router: &MultiServiceRouter, case: &str, named: &str) -> String {
    let (status, described) = kms(router, "DescribeKey", &json!({ "KeyId": named })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeKey to resolve {named:?}, got {status} with {described}"
    );
    described["KeyMetadata"]["KeyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no KeyId in {described}"))
        .to_string()
}

/// The `<SnapshotCopyGrant>` Amazon Redshift reports for `name`, or the refusal it answered.
async fn describe_grant(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    redshift(
        router,
        "DescribeSnapshotCopyGrants",
        &[(NAME_PROPERTY, name)],
    )
    .await
}

/// Why a stack's resources failed, as CloudFormation records it on the stack's own events.
async fn failure_reasons(router: &MultiServiceRouter, stack_name: &str) -> Vec<String> {
    let (_, body) =
        cloudformation(router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    elements(&body, "ResourceStatusReason")
}

/// The `DescribeStacks` body once the stack rests in a status that is not in progress, or the
/// last one read when the deadline passes. Waits for the CONDITION, looking again until the
/// stack has settled.
async fn settled_stack(router: &MultiServiceRouter, case: &str, stack_name: &str) -> String {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
        );
        let settled = opt_element(&body, "StackStatus")
            .is_some_and(|stack_status| !stack_status.ends_with("_IN_PROGRESS"));
        if settled || std::time::Instant::now() >= deadline {
            return body;
        }
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// Create the stack `stack_name` from `template` and answer its settled `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack — an {GRANT} is a type a template \
         may declare — got {status} with body {body}"
    );
    assert!(
        !body.contains(UNRECOGNIZED_TYPES),
        "case {case:?}: expected the type to be recognised, got {body}"
    );
    settled_stack(router, case, stack_name).await
}

/// Whether `name` is a name Amazon Redshift's published syntax for a snapshot copy grant
/// admits, which is what a name CloudFormation GENERATES has to be inside.
fn is_a_published_grant_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_GRANT_NAME_CHARACTERS
        && name.starts_with(|first: char| first.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.ends_with('-')
        && !name.contains("--")
}

/// What one row expects of one grant the stack declares.
struct Declared {
    /// The output key whose value is the grant's name — what its `Ref` returns.
    name_output: &'static str,
    /// The name the declaration states, or `None` where it leaves the name to CloudFormation.
    declared_name: Option<&'static str>,
    /// The `KmsKeyId` the grant must report: the value the declaration stated, or the default
    /// key where it stated none.
    expected_key: ExpectedKey,
}

/// Which key a row's grant must come out covering. A declaration that NAMES a key has that
/// identifier reported back in the form it spelled; one that names none covers the account's
/// default key.
enum ExpectedKey {
    /// The key the row's own template named, by the id KMS minted for it.
    KeyId,
    /// The same key, named in the template by its ARN.
    KeyArn,
    /// No key stated at all, which is the account's default key.
    Default,
}

/// One row: a template and the grants it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    declared: Vec<Declared>,
}

/// The rows of the deploy table, built around a key this account really holds — the key the
/// rows that name one name.
fn rows(key_id: &str, key_arn: &str) -> Vec<Row> {
    vec![
        // The issue's own stack: one named grant over a key id, with the name read back
        // through `Ref`, in JSON with a long-form intrinsic.
        Row {
            case: "a named grant over a key id, Ref read back, JSON spelling",
            stack_name: "redshift-grant-round-trip",
            template: json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "Grant": {
                        "Type": GRANT,
                        "Properties": {
                            NAME_PROPERTY: "cfn-grant-round-trip",
                            "KmsKeyId": key_id,
                        }
                    }
                },
                "Outputs": {"GrantName": {"Value": {"Ref": "Grant"}}}
            })
            .to_string(),
            declared: vec![Declared {
                name_output: "GrantName",
                declared_name: Some("cfn-grant-round-trip"),
                expected_key: ExpectedKey::KeyId,
            }],
        },
        // The same key named by its ARN, with the grant's name left to CloudFormation: the
        // type names itself nowhere, so the name the stack publishes is one CloudFormation
        // generated — and Amazon Redshift has to hold a grant under exactly it.
        Row {
            case: "an unnamed grant over a key ARN",
            stack_name: "redshift-grant-generated-name",
            template: json!({
                "Resources": {
                    "Grant": {
                        "Type": GRANT,
                        "Properties": { "KmsKeyId": key_arn }
                    }
                },
                "Outputs": {"GrantName": {"Value": {"Ref": "Grant"}}}
            })
            .to_string(),
            declared: vec![Declared {
                name_output: "GrantName",
                declared_name: None,
                expected_key: ExpectedKey::KeyArn,
            }],
        },
        // A declaration naming NO key at all, in YAML with the short-form `!Ref` and the
        // type's own `Tags`.
        Row {
            case: "a grant that names no key, YAML spelling with declared tags",
            stack_name: "redshift-grant-default-key",
            template: format!(
                "\
Resources:
  Grant:
    Type: {GRANT}
    Properties:
      {NAME_PROPERTY}: cfn-grant-default-key
      Tags:
        - Key: team
          Value: analytics
Outputs:
  GrantName:
    Value: !Ref Grant
"
            ),
            declared: vec![Declared {
                name_output: "GrantName",
                declared_name: Some("cfn-grant-default-key"),
                expected_key: ExpectedKey::Default,
            }],
        },
        // Two grants in one stack: one named over the key ARN, one unnamed over no key at
        // all. Each publishes its OWN name, and each is a grant that service holds.
        Row {
            case: "two grants in one stack",
            stack_name: "redshift-grant-pair",
            template: json!({
                "Resources": {
                    "First": {
                        "Type": GRANT,
                        "Properties": {
                            NAME_PROPERTY: "cfn-grant-pair-first",
                            "KmsKeyId": key_arn,
                        }
                    },
                    "Second": {
                        "Type": GRANT,
                        "Properties": {}
                    }
                },
                "Outputs": {
                    "FirstName": {"Value": {"Ref": "First"}},
                    "SecondName": {"Value": {"Ref": "Second"}}
                }
            })
            .to_string(),
            declared: vec![
                Declared {
                    name_output: "FirstName",
                    declared_name: Some("cfn-grant-pair-first"),
                    expected_key: ExpectedKey::KeyArn,
                },
                Declared {
                    name_output: "SecondName",
                    declared_name: None,
                    expected_key: ExpectedKey::Default,
                },
            ],
        },
    ]
}

#[tokio::test]
async fn a_deployed_stack_publishes_the_grant_redshift_holds() {
    // ONE router for the table: the key the rows name is created in it first, so every
    // template is deployed against the account that really holds exactly that key.
    let router = build_router();
    let (key_id, key_arn) = create_key(&router, "the key the declared grants cover").await;

    for row in rows(&key_id, &key_arn) {
        let case = row.case;
        let body = create_and_settle(&router, case, row.stack_name, &row.template).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the create to complete, got {body} with failure reasons \
             {:?}",
            failure_reasons(&router, row.stack_name).await
        );
        let outputs: BTreeMap<String, String> = outputs_of(&body);

        for declared in &row.declared {
            let name = outputs
                .get(declared.name_output)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no {} output in {body}", declared.name_output)
                })
                .clone();
            match declared.declared_name {
                // A declaration that NAMES the grant decides the name, so the handle the
                // stack publishes is exactly it.
                Some(stated) => assert_eq!(
                    name, stated,
                    "case {case:?}: expected the declared name to be the published handle, got \
                     {name:?}"
                ),
                // One that does not leaves it to CloudFormation, whose generated name still
                // has to be inside the syntax the owning service publishes.
                None => assert!(
                    is_a_published_grant_name(&name),
                    "case {case:?}: expected a generated name inside Amazon Redshift's \
                     published syntax, got {name:?}"
                ),
            }

            // THE RULE: the grant the stack claims to have created is a grant Amazon Redshift
            // holds, under the name the stack published, covering the key the template
            // declared.
            let (status, reported) = describe_grant(&router, &name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected Amazon Redshift to report the grant the stack \
                 created under {name:?}, got {status} with body {reported}"
            );
            let grant = block_of(&reported, "SnapshotCopyGrant")
                .unwrap_or_else(|| panic!("case {case:?}: no SnapshotCopyGrant in {reported}"));
            assert_eq!(
                opt_element(grant, NAME_PROPERTY).as_deref(),
                Some(name.as_str()),
                "case {case:?}: expected the grant to carry the published name, got {grant}"
            );
            // The identifier a declaration STATES is what the grant covers, character for
            // character — a key id stays a key id and a key ARN stays that ARN. A
            // declaration stating no key covers the account's default key, and there the
            // identifier comes from Key Management Service, because nothing was supplied to
            // report.
            let expected_key = match declared.expected_key {
                ExpectedKey::KeyId => key_id.clone(),
                ExpectedKey::KeyArn => key_arn.clone(),
                ExpectedKey::Default => resolved_key_id(&router, case, DEFAULT_KEY).await,
            };
            assert_eq!(
                opt_element(grant, "KmsKeyId").as_deref(),
                Some(expected_key.as_str()),
                "case {case:?}: expected the grant to cover {expected_key:?}, got {grant}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_grants_it_created() {
    let router = build_router();
    let case = "two grants deleted with their stack";
    let stack_name = "redshift-grant-deleted";
    let (key_id, _) = create_key(&router, case).await;
    let template = json!({
        "Resources": {
            "First": {
                "Type": GRANT,
                "Properties": { NAME_PROPERTY: "cfn-grant-deleted-first", "KmsKeyId": key_id }
            },
            "Second": {
                "Type": GRANT,
                "Properties": { NAME_PROPERTY: "cfn-grant-deleted-second" }
            }
        },
        "Outputs": {
            "FirstName": {"Value": {"Ref": "First"}},
            "SecondName": {"Value": {"Ref": "Second"}}
        }
    })
    .to_string();

    let body = create_and_settle(&router, case, stack_name, &template).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the create to complete, got {body} with failure reasons {:?}",
        failure_reasons(&router, stack_name).await
    );
    let outputs = outputs_of(&body);
    let names: Vec<String> = ["FirstName", "SecondName"]
        .iter()
        .map(|key| {
            outputs
                .get(*key)
                .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
                .clone()
        })
        .collect();
    for name in &names {
        let (status, reported) = describe_grant(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the grant {name:?} to exist before the delete, got \
             {status} with body {reported}"
        );
    }

    let (status, deleted) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {deleted}"
    );
    let settled = settled_deletion(&router, case, stack_name, &names).await;
    assert!(
        !settled.contains("DELETE_FAILED"),
        "case {case:?}: expected the delete to succeed, got {settled}"
    );

    // The grants the stack created are gone from Amazon Redshift, which is what makes a
    // stack's resources the stack's own rather than leftovers.
    for name in &names {
        let (status, reported) = describe_grant(&router, name).await;
        assert_eq!(
            (status, elements(&reported, "Code")),
            (400, vec!["SnapshotCopyGrantNotFoundFault".to_string()]),
            "case {case:?}: expected the grant {name:?} to be gone with the stack, got \
             {status} with body {reported}"
        );
    }
}

/// Wait until the deletion of `stack_name` has run its course: every grant it created is gone
/// from Amazon Redshift, or the stack itself reports a status that is not in progress (a
/// deleted stack is no longer readable by name at all, which is the same end state). Answers
/// the last stack answer read, for the assertion's message.
///
/// Waits for the CONDITION rather than for a span, under a generous deadline so a loaded host
/// cannot decide the outcome.
async fn settled_deletion(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    names: &[String],
) -> String {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let mut gone = true;
        for name in names {
            let (status, _) = describe_grant(router, name).await;
            gone &= status != 200;
        }
        let (_, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        if gone || std::time::Instant::now() >= deadline {
            return body;
        }
        assert!(
            !body.contains("DELETE_FAILED"),
            "case {case:?}: expected the delete to succeed, got {body}"
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

#[tokio::test]
async fn a_redeclared_key_is_refused_rather_than_silently_dropped() {
    // The key a grant covers is fixed when Amazon Redshift makes it — that service publishes
    // no modify for a grant at all — so a deployment that states another key cannot be
    // applied to the grant the stack has. What must never happen is the quiet outcome: an
    // update reported as complete while the grant goes on covering the old key.
    let router = build_router();
    let case = "an update that re-points the grant at another key";
    let stack_name = "redshift-grant-rekeyed";
    let (first_key, _) = create_key(&router, "the key the grant is created with").await;
    let (second_key, _) = create_key(&router, "the key the update asks for").await;
    let template_of = |key: &str| {
        json!({
            "Resources": {
                "Grant": {
                    "Type": GRANT,
                    "Properties": { NAME_PROPERTY: "cfn-grant-rekeyed", "KmsKeyId": key }
                }
            },
            "Outputs": {"GrantName": {"Value": {"Ref": "Grant"}}}
        })
        .to_string()
    };

    let body = create_and_settle(&router, case, stack_name, &template_of(&first_key)).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the create to complete, got {body} with failure reasons {:?}",
        failure_reasons(&router, stack_name).await
    );
    let name = outputs_of(&body)
        .get("GrantName")
        .unwrap_or_else(|| panic!("case {case:?}: no GrantName output in {body}"))
        .clone();

    let (status, updated) = cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &template_of(&second_key)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {updated}"
    );
    let settled = settled_stack(&router, case, stack_name).await;
    assert_eq!(
        opt_element(&settled, "StackStatus").as_deref(),
        Some("UPDATE_ROLLBACK_COMPLETE"),
        "case {case:?}: expected the update to be rolled back rather than reported as applied, \
         since Amazon Redshift cannot re-point a grant at another key, got {settled} with \
         failure reasons {:?}",
        failure_reasons(&router, stack_name).await
    );
    // ...and the resource's own event says WHY: the property the deployment restated, on the
    // type it was restated for, rather than a stack that failed for no stated reason.
    let reasons = failure_reasons(&router, stack_name).await;
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("KmsKeyId") && reason.contains(GRANT)),
        "case {case:?}: expected the failed resource to report the property it could not \
         change, got {reasons:?}"
    );

    // And the grant that service holds is untouched: it still covers the key it was created
    // with, rather than reporting a key nothing wrote.
    let (status, reported) = describe_grant(&router, &name).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the grant to survive the refused update, got {status} with \
         body {reported}"
    );
    let grant = block_of(&reported, "SnapshotCopyGrant")
        .unwrap_or_else(|| panic!("case {case:?}: no SnapshotCopyGrant in {reported}"));
    assert_eq!(
        opt_element(grant, "KmsKeyId").as_deref(),
        Some(first_key.as_str()),
        "case {case:?}: expected the grant to still cover the key it was created with, got \
         {grant}"
    );
}

/// One refusal row: the FORM an unreadable key is named in, and the stack it is declared by.
struct Refused {
    case: &'static str,
    stack_name: &'static str,
    grant_name: &'static str,
    /// The `KmsKeyId` the declaration states — a key this account never created, named in one
    /// of the forms `DescribeKey` accepts.
    kms_key_id: &'static str,
}

const REFUSED: &[Refused] = &[
    // The issue's own refusal: a bare key id nothing ever minted.
    Refused {
        case: "a key id this account does not hold",
        stack_name: "redshift-grant-unknown-key-id",
        grant_name: "cfn-grant-unknown-key-id",
        kms_key_id: "00000000-0000-0000-0000-000000000000",
    },
    // The same key named as an ARN: a well-formed identifier of a key that is not there.
    Refused {
        case: "a key ARN this account does not hold",
        stack_name: "redshift-grant-unknown-key-arn",
        grant_name: "cfn-grant-unknown-key-arn",
        kms_key_id:
            "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555",
    },
    // An ALIAS nobody created, which is the third form the member admits.
    Refused {
        case: "an alias no key carries",
        stack_name: "redshift-grant-unknown-alias",
        grant_name: "cfn-grant-unknown-alias",
        kms_key_id: "alias/no-such-key",
    },
];

#[tokio::test]
async fn a_key_kms_cannot_read_rolls_the_stack_back() {
    for row in REFUSED {
        let router = build_router();
        let case = row.case;
        let template = json!({
            "Resources": {
                "Grant": {
                    "Type": GRANT,
                    "Properties": {
                        NAME_PROPERTY: row.grant_name,
                        "KmsKeyId": row.kms_key_id,
                    }
                }
            },
            "Outputs": {"GrantName": {"Value": {"Ref": "Grant"}}}
        })
        .to_string();

        let body = create_and_settle(&router, case, row.stack_name, &template).await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the failed create to be rolled back rather than publish a \
             grant Amazon Redshift never held, got {body}"
        );
        assert!(
            outputs_of(&body).is_empty(),
            "case {case:?}: expected no output for a resource nothing created, got {body}"
        );

        // The failure is the OWNING service's own refusal, on the resource that failed, and
        // never a refusal of the template: no event reports a template-format error at all.
        let (_, events) =
            cloudformation(&router, "DescribeStackEvents", &[("StackName", row.stack_name)]).await;
        assert!(
            !events.contains(UNRECOGNIZED_TYPES),
            "case {case:?}: expected no template-format error for a type AWS publishes, got \
             {events}"
        );
        let reasons = elements(&events, "ResourceStatusReason");
        // The sentence Amazon Redshift words this class of refusal with, naming the value the
        // declaration stated, and the `HandlerErrorCode` class a refused REQUEST falls in.
        let sentence = format!(
            "Unable to create snapshot copy grant. The KMS key '{}' does not exist or is \
             inaccessible.",
            row.kms_key_id
        );
        assert!(
            reasons.iter().any(|reason| reason.contains(&sentence)
                && reason.contains("HandlerErrorCode: InvalidRequest")),
            "case {case:?}: expected a failure reason carrying {sentence:?}, got {reasons:?}"
        );

        // And nothing was created: Amazon Redshift holds no grant under the declared name.
        let (status, reported) = describe_grant(&router, row.grant_name).await;
        assert_eq!(
            (status, elements(&reported, "Code")),
            (400, vec!["SnapshotCopyGrantNotFoundFault".to_string()]),
            "case {case:?}: expected Amazon Redshift to hold no grant named {:?}, got {status} \
             with body {reported}",
            row.grant_name
        );
    }
}
