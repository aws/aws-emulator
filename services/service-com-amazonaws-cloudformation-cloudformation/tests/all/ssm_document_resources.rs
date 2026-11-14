// SPDX-License-Identifier: CC0-1.0

//! `AWS::SSM::Document` end to end: the document a stack declares exists in AWS Systems
//! Manager itself, under the name the stack publishes.
//!
//! RULE: a stack that reaches `CREATE_COMPLETE` has created every resource it declares in the
//! service that OWNS it, so a resource type that service holds under a NAME is resolvable
//! there by the very name the stack's `Ref` returns. For a Systems Manager document that name
//! is what every call of that service on a document takes — `GetDocument`, `DescribeDocument`,
//! and `StartAutomationExecution`'s `DocumentName` for a runbook — so a client that deploys a
//! runbook and immediately runs it is answered an execution rather than a not-found refusal.
//! Deleting the stack releases the document there again.
//!
//! The rows differ along what the rule generalises over: the template spelling (JSON or YAML),
//! the document TYPE (each of the ones a stack usefully declares), the document FORMAT, the
//! schema version that type's content declares, whether the declaration names the document or
//! leaves CloudFormation to name it, and whether it carries tags of its own.
//!
//! Every case drives the real request path for both services through one router — the
//! CloudFormation calls over the awsQuery wire and the Systems Manager calls over its
//! awsJson1_1 wire, each as a client's SDK makes them — so what is asserted is what a client
//! that deploys the template and then uses the document observes.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dynamic_references::ssm;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text,
};

/// How long a case waits for a stack operation to settle, and for an automation to reach its
/// outcome.
///
/// Generous on purpose: a create holds until the services that own the stack's resources
/// report them ready, and every one of those transitions is a stored due instant that nothing
/// sleeps for — so a case LOOKS AGAIN until the condition holds rather than waiting a fixed
/// time.
const DEADLINE: Duration = Duration::from_secs(120);

/// How long one look-again cycle pauses before polling again.
const POLL_PAUSE: Duration = Duration::from_millis(100);

/// A well-formed AMI id for the instance the end-to-end case stops: the issued form Amazon
/// EC2's own `ImageId` takes.
const IMAGE: &str = "ami-0123456789abcdef0";

/// The status a stack settles into, polled exactly as a client waiting on a deploy polls.
async fn settled_status(case: &str, router: &MultiServiceRouter, stack: &str) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (code, body) = describe_stack(router, stack).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected DescribeStacks({stack}) to answer, got {code} {body}"
        );
        let status = xml_text(&body, "StackStatus").unwrap_or_default();
        if !status.ends_with("_IN_PROGRESS") {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the stack to reach a terminal status, got {status:?}"
        );
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// The value the settled stack publishes under the output `key`.
async fn output(case: &str, router: &MultiServiceRouter, stack: &str, key: &str) -> String {
    let (_, body) = describe_stack(router, stack).await;
    let block = xml_blocks(&body, "Outputs")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the stack published no outputs: {body}"));
    xml_blocks(&block, "member")
        .into_iter()
        .find_map(|entry| {
            (xml_text(&entry, "OutputKey").as_deref() == Some(key))
                .then(|| xml_text(&entry, "OutputValue"))
                .flatten()
        })
        .unwrap_or_else(|| panic!("case {case:?}: the stack published no {key} output: {body}"))
}

/// What Systems Manager answers for a document of that name, or `None` where it holds none —
/// including where it refuses the name as a document it does not have.
///
/// `GetDocument` is the read that answers the document's CONTENT; the format it reports is the
/// one the read rendered that content in ("returns the document in the specified format"), so
/// the document's own type, format and status are read through [`described_document`].
async fn held_document(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let (status, document) = ssm(router, "GetDocument", &json!({"Name": name})).await;
    (status == 200).then_some(document)
}

/// The `DocumentDescription` Systems Manager reports for a document of that name, which
/// carries the document's own type, format and status.
async fn described_document(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let (status, answered) = ssm(router, "DescribeDocument", &json!({"Name": name})).await;
    (status == 200).then(|| answered["Document"].clone())
}

/// One row of the table: a template declaring a document, and what the rule says must then be
/// in Systems Manager.
struct Row {
    case: &'static str,
    stack: &'static str,
    template: String,
    /// The document type that service must report, which is the one the declaration states.
    document_type: &'static str,
    /// The format it must report.
    document_format: &'static str,
    /// Whether the declaration names the document itself (so the published name is that name)
    /// or leaves CloudFormation to name it.
    declared_name: Option<&'static str>,
}

/// An `Automation` runbook's content: schema version `0.3`, one parameter and one step.
fn automation_content() -> Value {
    json!({
        "schemaVersion": "0.3",
        "description": "Stop the instance the parameter names",
        "parameters": {
            "InstanceId": {"type": "String", "description": "the instance to stop"}
        },
        "mainSteps": [{
            "name": "stopInstance",
            "action": "aws:changeInstanceState",
            "inputs": {"InstanceIds": ["{{ InstanceId }}"], "DesiredState": "stopped"}
        }]
    })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a JSON template declaring a named Automation runbook with tags of its own",
            stack: "ssm-document-runbook",
            template: json!({
                "Resources": {
                    "StopDoc": {
                        "Type": "AWS::SSM::Document",
                        "Properties": {
                            "Name": "stack-declared-runbook",
                            "DocumentType": "Automation",
                            "DocumentFormat": "JSON",
                            "TargetType": "/AWS::EC2::Instance",
                            "Content": automation_content(),
                            "Tags": [{"Key": "owner", "Value": "platform"}]
                        }
                    }
                },
                "Outputs": {"DocumentName": {"Value": {"Ref": "StopDoc"}}}
            })
            .to_string(),
            document_type: "Automation",
            document_format: "JSON",
            declared_name: Some("stack-declared-runbook"),
        },
        Row {
            case: "a JSON template declaring a Command document and leaving CloudFormation to \
                   name it",
            stack: "ssm-document-command",
            template: json!({
                "Resources": {
                    "RunDoc": {
                        "Type": "AWS::SSM::Document",
                        "Properties": {
                            "DocumentType": "Command",
                            "Content": {
                                "schemaVersion": "2.2",
                                "description": "Echo a message on a managed node",
                                "mainSteps": [{
                                    "name": "runShell",
                                    "action": "aws:runShellScript",
                                    "inputs": {"runCommand": ["echo deployed"]}
                                }]
                            }
                        }
                    }
                },
                "Outputs": {"DocumentName": {"Value": {"Ref": "RunDoc"}}}
            })
            .to_string(),
            document_type: "Command",
            document_format: "JSON",
            declared_name: None,
        },
        Row {
            case: "a YAML template declaring a YAML-format Session document",
            stack: "ssm-document-session",
            template: [
                "Resources:",
                "  SessionDoc:",
                "    Type: AWS::SSM::Document",
                "    Properties:",
                "      Name: stack-declared-session",
                "      DocumentType: Session",
                "      DocumentFormat: YAML",
                "      Content:",
                "        schemaVersion: '1.0'",
                "        description: Open a shell session",
                "        sessionType: Standard_Stream",
                "Outputs:",
                "  DocumentName:",
                "    Value: !Ref SessionDoc",
            ]
            .join("\n"),
            document_type: "Session",
            document_format: "YAML",
            declared_name: Some("stack-declared-session"),
        },
    ]
}

/// RULE: every row's stack completes, publishes the document's NAME, and Systems Manager
/// really holds a document of that name carrying the declared type, format and content.
#[tokio::test]
async fn a_declared_document_exists_in_systems_manager_under_the_published_name() {
    for row in rows() {
        let case = row.case;
        let router = router();
        let (status, body) = create_stack(&router, row.stack, &row.template, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
        );
        let settled = settled_status(case, &router, row.stack).await;
        assert_eq!(
            settled, "CREATE_COMPLETE",
            "case {case:?}: expected the stack to complete, got {settled:?}"
        );

        let published = output(case, &router, row.stack, "DocumentName").await;
        match row.declared_name {
            Some(name) => assert_eq!(
                published, name,
                "case {case:?}: expected the declared name to be published, got {published:?}"
            ),
            // A declaration that names no document is named by CloudFormation, within the
            // bound `CreateDocument$Name`'s own pattern fixes (3..128 characters of letters,
            // digits, `_`, `-` and `.`).
            None => {
                assert!(
                    (3..=128).contains(&published.len())
                        && published.chars().all(|character| {
                            character.is_ascii_alphanumeric()
                                || ['_', '-', '.'].contains(&character)
                        }),
                    "case {case:?}: expected a generated name a document can carry, got \
                     {published:?}"
                );
            }
        }

        let document = held_document(&router, &published).await.unwrap_or_else(|| {
            panic!(
                "case {case:?}: Systems Manager holds no document under the name the stack \
                 published ({published:?})"
            )
        });
        assert_eq!(
            document["Name"].as_str(),
            Some(published.as_str()),
            "case {case:?}: expected the document to report the published name, got {document}"
        );
        let described = described_document(&router, &published)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: Systems Manager describes no document {published:?}")
            });
        assert_eq!(
            described["DocumentType"].as_str(),
            Some(row.document_type),
            "case {case:?}: expected the declared document type, got {described}"
        );
        assert_eq!(
            described["DocumentFormat"].as_str(),
            Some(row.document_format),
            "case {case:?}: expected the declared document format, got {described}"
        );
        // The stack reports the resource created only once that service reports the document
        // in place, so a client reading the stack as complete finds a usable document.
        assert_eq!(
            described["Status"].as_str(),
            Some("Active"),
            "case {case:?}: expected the completed stack to have left the document ready, got \
             {described}"
        );
        let content = document["Content"].as_str().unwrap_or_default();
        assert!(
            content.contains("schemaVersion"),
            "case {case:?}: expected the declared content to reach that service, got \
             {content:?}"
        );

        // Deleting the stack releases the document in Systems Manager.
        let (status, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with {body}"
        );
        wait_for_the_transition().await;
        let held = held_document(&router, &published).await;
        assert!(
            held.is_none(),
            "case {case:?}: expected the deleted stack to have released the document, Systems \
             Manager still reports {held:?}"
        );
    }
}

/// RULE: because the document really exists in Systems Manager under the published name, the
/// Automation surface of that service resolves it — `StartAutomationExecution` with the
/// stack's own output answers an execution id, the execution reaches `Success`, and the
/// `aws:changeInstanceState` step really stops the instance the same stack launched in Amazon
/// EC2.
#[tokio::test]
async fn a_declared_runbook_runs_and_stops_the_stacks_own_instance() {
    let case = "a stack declaring an Automation runbook beside the instance it stops";
    let router = router();
    let stack = "ssm-document-runbook-stops-an-instance";
    let template = json!({
        "Resources": {
            "Instance": {
                "Type": "AWS::EC2::Instance",
                "Properties": {"ImageId": IMAGE, "InstanceType": "t3.micro"}
            },
            "StopDoc": {
                "Type": "AWS::SSM::Document",
                "Properties": {
                    "DocumentType": "Automation",
                    "Content": automation_content()
                }
            }
        },
        "Outputs": {
            "InstanceId": {"Value": {"Ref": "Instance"}},
            "DocumentName": {"Value": {"Ref": "StopDoc"}}
        }
    })
    .to_string();

    let (status, body) = create_stack(&router, stack, &template, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    let settled = settled_status(case, &router, stack).await;
    assert_eq!(
        settled, "CREATE_COMPLETE",
        "case {case:?}: expected the stack to complete, got {settled:?}"
    );
    let document_name = output(case, &router, stack, "DocumentName").await;
    let instance_id = output(case, &router, stack, "InstanceId").await;

    // The very next call: the runbook the stack published, run by name.
    let (status, started) = ssm(
        &router,
        "StartAutomationExecution",
        &json!({
            "DocumentName": document_name,
            "Parameters": {"InstanceId": [instance_id.clone()]},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected StartAutomationExecution({document_name}) to answer an \
         execution id, got {status} with {started}"
    );
    let execution_id = started["AutomationExecutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the start answered no execution id: {started}"))
        .to_string();

    // The execution reaches `Success`, and the step's work really happened in Amazon EC2.
    let deadline = Instant::now() + DEADLINE;
    let reported = loop {
        let (status, body) = ssm(
            &router,
            "GetAutomationExecution",
            &json!({"AutomationExecutionId": execution_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetAutomationExecution to answer, got {status} with {body}"
        );
        let execution = body["AutomationExecution"].clone();
        let reported = execution["AutomationExecutionStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if !["Pending", "InProgress", "Waiting"].contains(&reported.as_str()) {
            break execution;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the execution to reach an outcome, it reports {reported:?}"
        );
        tokio::time::sleep(POLL_PAUSE).await;
    };
    assert_eq!(
        reported["AutomationExecutionStatus"].as_str(),
        Some("Success"),
        "case {case:?}: expected the execution to succeed, got {reported}"
    );

    let (status, described) = call(
        &router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", "Instance"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeStackResource to answer, got {status} with {described}"
    );
    assert_eq!(
        xml_text(&described, "PhysicalResourceId").as_deref(),
        Some(instance_id.as_str()),
        "case {case:?}: expected the published instance id to be the stack's own resource, got \
         {described}"
    );
    let state = instance_state(case, &router, &instance_id).await;
    assert_eq!(
        state, "stopped",
        "case {case:?}: expected Amazon EC2 to report {instance_id} stopped after the \
         automation ran, it reports {state:?}"
    );
}

/// RULE (the other half): a changed declaration reaches the document Systems Manager already
/// holds — its NAME is the handle `UpdateDocument` takes — so the stack keeps publishing that
/// name, the document gains a version, and the content a later run resolves is the one the
/// template now states.
#[tokio::test]
async fn a_changed_declaration_adds_a_version_to_the_document_in_place() {
    let router = router();
    let stack = "ssm-document-updated";
    let template = |description: &str| {
        json!({
            "Resources": {
                "RunDoc": {
                    "Type": "AWS::SSM::Document",
                    "Properties": {
                        "Name": "updated-command-document",
                        "DocumentType": "Command",
                        "Content": {
                            "schemaVersion": "2.2",
                            "description": description,
                            "mainSteps": [{
                                "name": "runShell",
                                "action": "aws:runShellScript",
                                "inputs": {"runCommand": [format!("echo {description}")]}
                            }]
                        }
                    }
                }
            },
            "Outputs": {"DocumentName": {"Value": {"Ref": "RunDoc"}}}
        })
        .to_string()
    };

    let case = "the stack that creates the document";
    let (status, body) = create_stack(&router, stack, &template("first"), &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    let settled = settled_status(case, &router, stack).await;
    assert_eq!(
        settled, "CREATE_COMPLETE",
        "case {case:?}: expected the stack to complete, got {settled:?}"
    );
    let created = output(case, &router, stack, "DocumentName").await;

    let case = "the update, which restates the content";
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &template("second")),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with {body}"
    );
    let settled = settled_status(case, &router, stack).await;
    assert_eq!(
        settled, "UPDATE_COMPLETE",
        "case {case:?}: expected the update to complete, got {settled:?}"
    );
    let updated = output(case, &router, stack, "DocumentName").await;
    assert_eq!(
        updated, created,
        "case {case:?}: expected the document to keep the name it was created under, got \
         {updated:?} after {created:?}"
    );

    // The default version a client running the document by name resolves carries the NEW
    // content, and the document really holds more than one version.
    let document = held_document(&router, &updated)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: Systems Manager holds no document {updated:?}"));
    let content = document["Content"].as_str().unwrap_or_default();
    assert!(
        content.contains("second"),
        "case {case:?}: expected the restated content to be what the name resolves to, got \
         {content:?}"
    );
    let described = described_document(&router, &updated)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: Systems Manager describes no {updated:?}"));
    assert_eq!(
        described["LatestVersion"].as_str(),
        described["DefaultVersion"].as_str(),
        "case {case:?}: expected the version the update created to be the default one, got \
         {described}"
    );
    assert_ne!(
        described["DocumentVersion"].as_str(),
        Some("1"),
        "case {case:?}: expected the update to have added a version, got {described}"
    );
}

/// The state Amazon EC2 reports for one instance, read through that service's own
/// `ec2Query` wire.
async fn instance_state(case: &str, router: &MultiServiceRouter, instance_id: &str) -> String {
    let body = format!(
        "Action=DescribeInstances&Version=2016-11-15&InstanceId.1={instance_id}"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("https://ec2.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/ec2/aws4_request, \
             SignedHeaders=host, Signature=0123456789abcdef",
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed ec2Query request");
    let response = router.dispatch(request).await;
    let answered = String::from_utf8_lossy(response.body()).to_string();
    let from = answered.find("<instanceState>").unwrap_or_else(|| {
        panic!("case {case:?}: DescribeInstances({instance_id}) reported no state: {answered}")
    });
    xml_text(&answered[from..], "name").unwrap_or_else(|| {
        panic!("case {case:?}: DescribeInstances({instance_id}) reported no state: {answered}")
    })
}
