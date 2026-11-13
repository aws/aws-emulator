// SPDX-License-Identifier: CC0-1.0

//! The operations that SUBMIT a template carrying a macro, end to end over the awsQuery
//! wire.
//!
//! Three rules, each asserted over a table whose rows differ along what the rule
//! generalises over:
//!
//! * **A template that INVOKES a macro requires `CAPABILITY_AUTO_EXPAND`**: "if your stack
//!   template contains one or more macros, and you choose to create a stack directly from
//!   the processed template [...] you must acknowledge this capability", and a macro is
//!   invoked either by the `Transform` section naming it for the whole template or by an
//!   `Fn::Transform` intrinsic naming one for part of it — so a request that acknowledges
//!   neither is refused with the modelled `InsufficientCapabilitiesException` and creates
//!   nothing. The rows differ along where the intrinsic stands, which macro it names, the
//!   spelling of the template, and (as the control) a template that invokes no macro at
//!   all.
//! * **A transform failure is reported with the service's own fixed sentence for that
//!   failure class**: an object a snippet is read from whose content is neither JSON nor
//!   YAML fails the operation with one sentence, the same for every object and every
//!   unreadable content — it names neither the location the template gave nor what was
//!   wrong with the bytes. The rows differ along what makes the content unreadable.
//! * **The snippet is expanded at EVERY operation**: the template an operation deploys is
//!   the document the objects in force at that moment make, so revising the object a
//!   template includes and re-submitting the same body is a real change the update carries
//!   out — and an update over an UNCHANGED body and an unchanged object is still the no-op
//!   CloudFormation refuses. The rows differ along what the revision does to the snippet
//!   (a member re-valued, added, dropped) and the spelling it is written in.

use serde_json::json;

use crate::harness::xml_blocks;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, sns, wait_for_the_transition, xml_text,
    xml_texts,
};
use crate::stack_resource_provisioning::s3;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    rollback_requested, status, CAPABILITY_AUTO_EXPAND,
};
use smithy_server_runtime::MultiServiceRouter;

/// The bucket the snippets these cases include live in, as a client's own `PutObject`
/// stored them.
const BUCKET: &str = "macro-snippet-bucket";

/// The reason CloudFormation records when the object a snippet is read from holds content
/// it cannot read as a template fragment. A FIXED sentence: it names no object, no key and
/// nothing about the bytes.
const CONTENT_REASON: &str =
    "Transform AWS::Include failed with: The specified S3 object's content should be valid \
     Yaml/JSON";

/// The `InsufficientCapabilitiesException` message that refuses a request which did not
/// acknowledge the macro its template invokes.
const REQUIRES_AUTO_EXPAND: &str = "Requires capabilities : [CAPABILITY_AUTO_EXPAND]";

/// What CloudFormation refuses an update that would change nothing with.
const NO_UPDATES: &str = "No updates are to be performed.";

/// One `Fn::Transform` directive naming `macro_name` with `Location`.
fn transform(macro_name: &str, location: &str) -> serde_json::Value {
    json!({"Name": macro_name, "Parameters": {"Location": location}})
}

/// The S3 URI of one snippet in [`BUCKET`].
fn location(key: &str) -> String {
    format!("s3://{BUCKET}/{key}")
}

/// The `Capabilities` member that acknowledges the macro a template invokes.
fn acknowledging_the_macro() -> Vec<(String, String)> {
    vec![member("Capabilities.member.1", CAPABILITY_AUTO_EXPAND)]
}

/// Create the snippet bucket, as a client does before putting a snippet in it.
async fn create_the_bucket(case: &str, router: &MultiServiceRouter) {
    let created = s3(router, "PUT", &format!("/{BUCKET}"), b"").await;
    assert!(
        created.status == 200,
        "case {case:?}: the snippet bucket must be created first, got {} {}",
        created.status,
        created.body
    );
}

/// Store one snippet object, with the BYTES the case wants in it (which is not always
/// text).
async fn put_snippet(case: &str, router: &MultiServiceRouter, key: &str, body: &[u8]) {
    let stored = s3(router, "PUT", &format!("/{BUCKET}/{key}"), body).await;
    assert!(
        stored.status == 200,
        "case {case:?}: the snippet {key} must be stored first, got {} {}",
        stored.status,
        stored.body
    );
}

/// The `PhysicalResourceId` the stack reports for `logical_id`, or `None` when it reports no
/// such resource.
async fn physical_id(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> Option<String> {
    let (status, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    (status == 200)
        .then(|| xml_text(&body, "PhysicalResourceId"))
        .flatten()
}

/// The template body `GetTemplate` serves for `stage`.
async fn template_stage(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    stage: &str,
) -> String {
    let (status, body) = call(
        router,
        "GetTemplate",
        &[member("StackName", stack), member("TemplateStage", stage)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetTemplate({stage}) must answer 200, got {status} {body}"
    );
    body
}

/// One row of the acknowledgement table: the template a request submits, and whether its
/// own declarations require the macro acknowledgement.
struct Acknowledged<'a> {
    case: &'a str,
    stack: &'a str,
    template: String,
    /// Whether this template INVOKES a macro, and so may only be submitted by a request
    /// that acknowledged it.
    invokes_a_macro: bool,
}

#[tokio::test]
async fn a_template_that_invokes_a_macro_is_refused_without_the_auto_expand_acknowledgement() {
    let rows: &[Acknowledged<'_>] = &[
        Acknowledged {
            case: "the intrinsic stands in a resource's properties",
            stack: "macro-in-properties",
            template: json!({
                "Resources": {
                    "Data": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "macro-in-properties-bucket",
                            "Fn::Transform": transform("AWS::Include", &location("snippet.json"))
                        }
                    }
                }
            })
            .to_string(),
            invokes_a_macro: true,
        },
        Acknowledged {
            case: "the intrinsic stands as the whole Resources section",
            stack: "macro-in-resources",
            template: json!({
                "Resources": {
                    "Fn::Transform": transform("AWS::Include", &location("snippet.json"))
                }
            })
            .to_string(),
            invokes_a_macro: true,
        },
        Acknowledged {
            case: "the intrinsic stands deep inside a property value",
            stack: "macro-in-nested-property",
            template: json!({
                "Resources": {
                    "Data": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "macro-in-nested-bucket",
                            "Tags": [{
                                "Key": "owner",
                                "Fn::Transform": transform(
                                    "AWS::Include",
                                    &location("snippet.json"),
                                )
                            }]
                        }
                    }
                }
            })
            .to_string(),
            invokes_a_macro: true,
        },
        Acknowledged {
            case: "the intrinsic names a macro of the caller's own",
            stack: "macro-private-name",
            template: json!({
                "Resources": {
                    "Data": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "macro-private-name-bucket",
                            "Fn::Transform": transform("MySnippetMacro", &location("snippet.json"))
                        }
                    }
                }
            })
            .to_string(),
            invokes_a_macro: true,
        },
        Acknowledged {
            case: "the template is spelled as YAML",
            stack: "macro-yaml-body",
            template: concat!(
                "Resources:\n",
                "  Data:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: macro-yaml-body-bucket\n",
                "      Fn::Transform:\n",
                "        Name: AWS::Include\n",
                "        Parameters:\n",
                "          Location: s3://macro-snippet-bucket/snippet.json\n",
            )
            .to_string(),
            invokes_a_macro: true,
        },
        Acknowledged {
            case: "the macro is named by the Transform section instead of the intrinsic",
            stack: "macro-transform-section",
            template: json!({
                "Transform": "MyWholeTemplateMacro",
                "Resources": {
                    "Data": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {"BucketName": "macro-transform-section-bucket"}
                    }
                }
            })
            .to_string(),
            invokes_a_macro: true,
        },
        Acknowledged {
            // The control: the gate is about a template that invokes a macro, so one that
            // invokes none is created with no acknowledgement at all.
            case: "the template invokes no macro",
            stack: "macro-none",
            template: json!({
                "Resources": {
                    "Data": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {"BucketName": "macro-none-bucket"}
                    }
                }
            })
            .to_string(),
            invokes_a_macro: false,
        },
    ];

    for row in rows {
        let router = router();
        create_the_bucket(row.case, &router).await;
        put_snippet(
            row.case,
            &router,
            "snippet.json",
            br#"{"VersioningConfiguration":{"Status":"Enabled"}}"#,
        )
        .await;

        let (status, answered) = create_stack(&router, row.stack, &row.template, &[]).await;
        if !row.invokes_a_macro {
            assert_eq!(
                status, 200,
                "case {:?}: a template invoking no macro must be accepted with no \
                 acknowledgement, got {status} {answered}",
                row.case
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {:?}: expected the unacknowledged request to be refused, got {status} \
             {answered}",
            row.case
        );
        assert_eq!(
            xml_text(&answered, "Code").as_deref(),
            Some("InsufficientCapabilitiesException"),
            "case {:?}: expected the modelled refusal, got {answered}",
            row.case
        );
        let message = xml_text(&answered, "Message").unwrap_or_default();
        assert_eq!(
            message, REQUIRES_AUTO_EXPAND,
            "case {:?}: expected the message {REQUIRES_AUTO_EXPAND:?}, got {message:?}",
            row.case
        );
        // Refused before anything was written: there is no such stack.
        let (described, body) = describe_stack(&router, row.stack).await;
        assert_eq!(
            described, 400,
            "case {:?}: the refused request must have created no stack, got {described} \
             {body}",
            row.case
        );

        // The same request, acknowledging the macro: accepted.
        let (status, created) =
            create_stack(&router, row.stack, &row.template, &acknowledging_the_macro()).await;
        assert_eq!(
            status, 200,
            "case {:?}: the acknowledged request must be accepted, got {status} {created}",
            row.case
        );
    }
}

/// One row of the unreadable-content table: the bytes the snippet object holds, and why no
/// reader will read them.
struct Unreadable<'a> {
    case: &'a str,
    stack: &'a str,
    key: &'a str,
    /// The object's content, as a client stored it — bytes, because one row's content is no
    /// text at all.
    content: &'a [u8],
}

#[tokio::test]
async fn an_include_whose_object_is_not_a_template_fragment_fails_with_the_services_own_reason() {
    let rows: &[Unreadable<'_>] = &[
        Unreadable {
            case: "the object holds bytes that are not text",
            stack: "snippet-not-text",
            key: "snippets/not-text.bin",
            content: &[0xff, 0xfe, 0x00, 0x01, 0x80],
        },
        Unreadable {
            case: "the flow mapping the object holds is never closed",
            stack: "snippet-unclosed",
            key: "snippets/unclosed.json",
            content: br#"{"BucketName": "half-written""#,
        },
        Unreadable {
            case: "the object holds two documents",
            stack: "snippet-two-documents",
            key: "snippets/two.yaml",
            content: b"BucketName: first\n---\nBucketName: second\n",
        },
        Unreadable {
            case: "the object holds an alias of its own anchor",
            stack: "snippet-alias",
            key: "snippets/alias.yaml",
            content: b"Anchored: &a\n  Status: Enabled\nCopy: *a\n",
        },
        Unreadable {
            case: "the object's second line is indented under a scalar",
            stack: "snippet-bad-indent",
            key: "snippets/indent.yaml",
            content: b"BucketName: one\n  Also: two\n",
        },
    ];

    for row in rows {
        let router = router();
        create_the_bucket(row.case, &router).await;
        put_snippet(row.case, &router, row.key, row.content).await;

        let template = json!({
            "Resources": {
                "Data": {
                    "Type": "AWS::S3::Bucket",
                    "Properties": {
                        "BucketName": "unreadable-snippet-bucket",
                        "Fn::Transform": transform("AWS::Include", &location(row.key))
                    }
                }
            }
        })
        .to_string();

        // "After the call completes successfully, the stack creation starts": the request is
        // accepted, and the transform's failure is the STACK's.
        let (status, created) =
            create_stack(&router, row.stack, &template, &acknowledging_the_macro()).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateStack must answer 200, got {status} {created}",
            row.case
        );
        wait_for_the_transition().await;

        let (status, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeStacks must answer 200, got {status} {described}",
            row.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::ROLLBACK_COMPLETE),
            "case {:?}: a failed transform must roll the stack back, got {described}",
            row.case
        );

        // The event log carries the transform's reason EXACTLY as the service words it,
        // which is what a client polling the create classifies the failure by.
        let (status, events) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeStackEvents must answer 200, got {status} {events}",
            row.case
        );
        let reasons = xml_texts(&events, "ResourceStatusReason");
        // The one stack-level event that EXPLAINS the rollback carries that sentence verbatim,
        // with only the clause saying the rollback was asked for appended to it; the terminal
        // status the rollback settles into adds no second copy of the failure.
        let on_the_rollback = rollback_requested(CONTENT_REASON);
        assert!(
            reasons.contains(&on_the_rollback),
            "case {:?}: expected an event whose reason is exactly {on_the_rollback:?}, got \
             {reasons:?}",
            row.case
        );
        // Nothing of the REQUEST is in any of them: the sentence is fixed, so the key, the
        // bucket and the location the template named appear nowhere.
        let stack_reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
        for reported in reasons.iter().chain(std::iter::once(&stack_reason)) {
            for interpolated in [row.key, BUCKET, "s3://"] {
                assert!(
                    !reported.contains(interpolated),
                    "case {:?}: the reason must carry nothing from the request, but \
                     {reported:?} carries {interpolated:?}",
                    row.case
                );
            }
        }
        assert!(
            stack_reason.contains(CONTENT_REASON),
            "case {:?}: expected the stack's reason to carry {CONTENT_REASON:?}, got \
             {stack_reason:?}",
            row.case
        );
        // The transform fails before anything is provisioned.
        assert_eq!(
            physical_id(&router, row.stack, "Data").await,
            None,
            "case {:?}: the stack must own no resource, got one for Data",
            row.case
        );
    }
}

/// One row of the re-expansion table: the snippet the stack was created with, the one that
/// replaces it in Amazon S3, and the topic configuration each of the two makes.
struct Revised<'a> {
    case: &'a str,
    stack: &'a str,
    topic: &'a str,
    key: &'a str,
    /// The object the create read, and the `DisplayName` Amazon SNS holds after it.
    first: &'a [u8],
    created_display_name: &'a str,
    /// The object that replaces it, and the `DisplayName` the update must leave behind.
    second: &'a [u8],
    updated_display_name: &'a str,
}

#[tokio::test]
async fn an_update_re_expands_the_snippet_so_a_revised_object_is_a_real_change() {
    let rows: &[Revised<'_>] = &[
        Revised {
            case: "a member the revision re-values, in a JSON snippet",
            stack: "revised-json",
            topic: "revised-json-topic",
            key: "snippets/display.json",
            first: br#"{"DisplayName":"before"}"#,
            created_display_name: "before",
            second: br#"{"DisplayName":"after"}"#,
            updated_display_name: "after",
        },
        Revised {
            case: "a member the revision re-values, in a YAML snippet",
            stack: "revised-yaml",
            topic: "revised-yaml-topic",
            key: "snippets/display.yaml",
            first: b"DisplayName: before\n",
            created_display_name: "before",
            second: b"DisplayName: after\n",
            updated_display_name: "after",
        },
        Revised {
            case: "a member the revision adds",
            stack: "revised-added",
            topic: "revised-added-topic",
            key: "snippets/added.json",
            first: b"{}",
            // The documented absent value of a display name a topic was never given.
            created_display_name: "",
            second: br#"{"DisplayName":"added"}"#,
            updated_display_name: "added",
        },
        Revised {
            case: "a member the revision drops",
            stack: "revised-removal",
            topic: "revised-removal-topic",
            key: "snippets/removal.json",
            first: br#"{"DisplayName":"beforehand"}"#,
            created_display_name: "beforehand",
            second: b"{}",
            updated_display_name: "",
        },
    ];

    for row in rows {
        let router = router();
        create_the_bucket(row.case, &router).await;
        put_snippet(row.case, &router, row.key, row.first).await;

        // The outer template, submitted UNCHANGED by both operations: everything that
        // differs between them is in the object it includes.
        let template = json!({
            "Resources": {
                "Notify": {
                    "Type": "AWS::SNS::Topic",
                    "Properties": {
                        "TopicName": row.topic,
                        "Fn::Transform": transform("AWS::Include", &location(row.key))
                    }
                }
            }
        })
        .to_string();

        let (status, created) =
            create_stack(&router, row.stack, &template, &acknowledging_the_macro()).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateStack must answer 200, got {status} {created}",
            row.case
        );
        wait_for_the_transition().await;
        let (_, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must reach CREATE_COMPLETE, got {described}",
            row.case
        );
        let arn = physical_id(&router, row.stack, "Notify")
            .await
            .unwrap_or_default();
        assert_eq!(
            display_name(row.case, &router, &arn).await,
            row.created_display_name,
            "case {:?}: the create must have deployed the snippet in force then",
            row.case
        );

        // The snippet is REVISED in Amazon S3, independently of the template that includes
        // it, and the same body is submitted again.
        put_snippet(row.case, &router, row.key, row.second).await;
        let (status, updated) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", row.stack),
                member("TemplateBody", &template),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the revised snippet is a change, so UpdateStack must answer 200, \
             got {status} {updated}",
            row.case
        );
        wait_for_the_transition().await;
        let (_, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::UPDATE_COMPLETE),
            "case {:?}: the update must complete, got {described}",
            row.case
        );
        assert_eq!(
            display_name(row.case, &router, &arn).await,
            row.updated_display_name,
            "case {:?}: the update must have deployed the REVISED snippet",
            row.case
        );
        // ...and the stage the stack deploys is the re-expanded document.
        let processed = template_stage(row.case, &router, row.stack, "Processed").await;
        assert!(
            !processed.contains("Fn::Transform"),
            "case {:?}: the processed stage must carry no intrinsic, got {processed}",
            row.case
        );
        if !row.updated_display_name.is_empty() {
            assert!(
                processed.contains(row.updated_display_name),
                "case {:?}: the processed stage must carry {:?}, got {processed}",
                row.case,
                row.updated_display_name
            );
        }
        if !row.created_display_name.is_empty() {
            assert!(
                !processed.contains(row.created_display_name),
                "case {:?}: the processed stage must no longer carry {:?}, got {processed}",
                row.case,
                row.created_display_name
            );
        }

        // The no-op refusal is still in force: with neither the body nor the object
        // changed, there is nothing to deploy.
        let (status, refused) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", row.stack),
                member("TemplateBody", &template),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: an update changing nothing must be refused, got {status} {refused}",
            row.case
        );
        let message = xml_text(&refused, "Message").unwrap_or_default();
        assert_eq!(
            message, NO_UPDATES,
            "case {:?}: expected the message {NO_UPDATES:?}, got {message:?}",
            row.case
        );
    }
}

/// The `DisplayName` Amazon SNS holds for the topic `arn` names, or the empty string for a
/// topic that carries none — which is the value Amazon SNS documents for a display name a
/// topic was never given.
async fn display_name(case: &str, router: &MultiServiceRouter, arn: &str) -> String {
    let (status, body) = sns(router, "GetTopicAttributes", &[("TopicArn", arn)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetTopicAttributes must answer for {arn:?}, got {status} {body}"
    );
    xml_blocks(&body, "entry")
        .iter()
        .filter(|entry| xml_text(entry, "key").as_deref() == Some("DisplayName"))
        .map(|entry| xml_text(entry, "value").unwrap_or_default())
        .next()
        .unwrap_or_default()
}
