// SPDX-License-Identifier: CC0-1.0

//! The `AWS::Include` transform, end to end over the awsQuery wire.
//!
//! RULE: an `Fn::Transform` naming `AWS::Include` is a processing instruction, not data:
//! CloudFormation reads the Amazon S3 object its `Location` names and splices what that
//! object holds into the template where the intrinsic stood, BEFORE any resource is
//! provisioned — so the members a snippet contributes reach the service that owns the
//! resource and the intrinsic itself never does. A snippet that cannot be read fails the
//! stack OPERATION, with a stack-level reason naming the transform and what could not be
//! read, and provisions nothing.
//!
//! Every case drives the real request path for both services through one router — the
//! CloudFormation calls over the awsQuery wire (the harness `stack_lifecycle.rs` owns) and
//! the Amazon S3 calls as a client makes them — so the snippet a stack includes is an
//! object a client's own `PutObject` stored, and the resource the spliced template asks for
//! is read back from the service that owns it.
//!
//! The rows differ along what the rule generalises over: the spelling of the template and
//! of the snippet (JSON and YAML), WHERE the intrinsic stands (a resource's properties, the
//! `Resources` section itself), whether it stands alone or beside members the template
//! declares, and — for the failures — what about the include cannot be carried out (a key
//! that is not there, a bucket that is not there, a `Location` that is no S3 URI, an object
//! that is no set of template members).

use serde_json::json;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, iam, member, router,
    wait_for_the_transition, xml_text, xml_texts,
};
use crate::stack_resource_provisioning::s3;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    rollback_requested, status, CAPABILITY_AUTO_EXPAND,
};
use smithy_server_runtime::MultiServiceRouter;

/// The bucket the snippets live in, as a client's own `PutObject` stored them.
const SNIPPET_BUCKET: &str = "include-snippets";

/// One `Fn::Transform` directive that includes `location`.
fn include(location: &str) -> serde_json::Value {
    json!({"Name": "AWS::Include", "Parameters": {"Location": location}})
}

/// The S3 URI of one snippet in [`SNIPPET_BUCKET`].
fn location(key: &str) -> String {
    format!("s3://{SNIPPET_BUCKET}/{key}")
}

/// The `Capabilities` member a request submitting a template that invokes a macro carries:
/// "if your stack template contains one or more macros [...] you must acknowledge this
/// capability", which an `Fn::Transform` of `AWS::Include` is.
fn acknowledging_the_macro() -> Vec<(String, String)> {
    vec![member("Capabilities.member.1", CAPABILITY_AUTO_EXPAND)]
}

/// Does Amazon S3 hold this bucket?
async fn bucket_exists(router: &MultiServiceRouter, bucket: &str) -> bool {
    s3(router, "HEAD", &format!("/{bucket}"), b"").await.status == 200
}

/// Store the snippets a row includes, in the bucket a client created for them.
async fn seed_snippets(router: &MultiServiceRouter, snippets: &[(&str, &str)]) {
    let created = s3(router, "PUT", &format!("/{SNIPPET_BUCKET}"), b"").await;
    assert_eq!(
        created.status, 200,
        "the snippet bucket must be created first, got {} {}",
        created.status, created.body
    );
    for (key, body) in snippets {
        let stored = s3(
            router,
            "PUT",
            &format!("/{SNIPPET_BUCKET}/{key}"),
            body.as_bytes(),
        )
        .await;
        assert_eq!(
            stored.status, 200,
            "the snippet {key} must be stored first, got {} {}",
            stored.status, stored.body
        );
    }
}

/// The `PhysicalResourceId` the stack reports for `logical_id`, or `None` when it reports
/// no such resource.
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

/// One row of the splice table: the snippets to store, the template that includes them, and
/// the buckets the SPLICED template must have provisioned.
struct Spliced<'a> {
    case: &'a str,
    stack: &'a str,
    snippets: &'a [(&'a str, &'a str)],
    template: String,
    /// `(logical id, bucket name)` the stack must report and Amazon S3 must hold.
    buckets: &'a [(&'a str, &'a str)],
    /// The buckets whose versioning must be enabled, which is a member the template
    /// declares BESIDE the intrinsic: the splice contributes members, it does not displace
    /// them.
    versioned: &'a [&'a str],
}

#[tokio::test]
async fn an_included_snippet_is_spliced_into_the_template_before_the_resources_are_created() {
    let rows: &[Spliced<'_>] = &[
        Spliced {
            case: "a JSON snippet included as the whole of a resource's properties",
            stack: "include-json",
            snippets: &[("snippets/name.json", r#"{"BucketName":"include-json-bucket"}"#)],
            template: json!({
                "Resources": {
                    "Data": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {"Fn::Transform": include(&location("snippets/name.json"))}
                    }
                }
            })
            .to_string(),
            buckets: &[("Data", "include-json-bucket")],
            versioned: &[],
        },
        Spliced {
            case: "a YAML snippet included beside properties the template declares",
            stack: "include-yaml",
            snippets: &[(
                "snippets/name.yaml",
                "BucketName: include-yaml-bucket\n",
            )],
            template: concat!(
                "Resources:\n",
                "  Data:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      VersioningConfiguration:\n",
                "        Status: Enabled\n",
                "      Fn::Transform:\n",
                "        Name: AWS::Include\n",
                "        Parameters:\n",
                "          Location: s3://include-snippets/snippets/name.yaml\n",
            )
            .to_string(),
            buckets: &[("Data", "include-yaml-bucket")],
            versioned: &["include-yaml-bucket"],
        },
        Spliced {
            case: "a snippet included into the Resources section, carrying a resource of \
                   its own",
            stack: "include-resource",
            snippets: &[(
                "snippets/resource.json",
                r#"{"Extra":{"Type":"AWS::S3::Bucket",
                    "Properties":{"BucketName":"include-extra-bucket"}}}"#,
            )],
            template: json!({
                "Resources": {
                    "Base": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {"BucketName": "include-base-bucket"}
                    },
                    "Fn::Transform": include(&location("snippets/resource.json"))
                }
            })
            .to_string(),
            buckets: &[
                ("Base", "include-base-bucket"),
                ("Extra", "include-extra-bucket"),
            ],
            versioned: &[],
        },
        Spliced {
            case: "two resources including the same snippet, each with its own name \
                   declared beside it",
            stack: "include-shared",
            snippets: &[(
                "snippets/versioning.json",
                r#"{"VersioningConfiguration":{"Status":"Enabled"}}"#,
            )],
            template: json!({
                "Resources": {
                    "One": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "include-shared-one",
                            "Fn::Transform": include(&location("snippets/versioning.json"))
                        }
                    },
                    "Two": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "include-shared-two",
                            "Fn::Transform": include(&location("snippets/versioning.json"))
                        }
                    }
                }
            })
            .to_string(),
            buckets: &[
                ("One", "include-shared-one"),
                ("Two", "include-shared-two"),
            ],
            versioned: &["include-shared-one", "include-shared-two"],
        },
        Spliced {
            case: "a member the template declares itself is kept over the snippet's",
            stack: "include-declared-wins",
            snippets: &[(
                "snippets/name.json",
                r#"{"BucketName":"include-snippet-name"}"#,
            )],
            template: json!({
                "Resources": {
                    "Data": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "include-declared-name",
                            "Fn::Transform": include(&location("snippets/name.json"))
                        }
                    }
                }
            })
            .to_string(),
            buckets: &[("Data", "include-declared-name")],
            versioned: &[],
        },
    ];

    for row in rows {
        let router = router();
        seed_snippets(&router, row.snippets).await;

        let (status, created) =
            create_stack(&router, row.stack, &row.template, &acknowledging_the_macro()).await;
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
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must reach CREATE_COMPLETE, got {described}",
            row.case
        );

        for (logical_id, bucket) in row.buckets {
            let reported = physical_id(&router, row.stack, logical_id).await;
            assert_eq!(
                reported.as_deref(),
                Some(*bucket),
                "case {:?}: the stack must report {bucket} for {logical_id}, got \
                 {reported:?}",
                row.case
            );
            assert!(
                bucket_exists(&router, bucket).await,
                "case {:?}: Amazon S3 must hold the bucket {bucket} the spliced template \
                 declares",
                row.case
            );
        }
        for bucket in row.versioned {
            let read = s3(&router, "GET", &format!("/{bucket}?versioning"), b"").await;
            assert!(
                read.status == 200 && read.body.contains("Enabled"),
                "case {:?}: the versioning the template declares beside the intrinsic must \
                 be in force on {bucket}, got {} {}",
                row.case,
                read.status,
                read.body
            );
        }

        // The processed template is the one that deployed, and it carries no intrinsic: the
        // submitted body still does, which is the stage a client asks for by name.
        for (stage, carries_the_intrinsic) in
            [("Processed", false), ("Original", true)]
        {
            let (status, body) = call(
                &router,
                "GetTemplate",
                &[
                    member("StackName", row.stack),
                    member("TemplateStage", stage),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: GetTemplate({stage}) must answer 200, got {status} {body}",
                row.case
            );
            assert_eq!(
                body.contains("Fn::Transform"),
                carries_the_intrinsic,
                "case {:?}: the {stage} template must{} carry the intrinsic, got {body}",
                row.case,
                if carries_the_intrinsic { "" } else { " not" }
            );
        }
    }
}

/// One row of the failure table: what about the include cannot be carried out, and the
/// stack-level reason that failure is reported with.
struct Refused<'a> {
    case: &'a str,
    stack: &'a str,
    snippets: &'a [(&'a str, &'a str)],
    template: String,
    /// The text the stack's reason must carry.
    reason: &'a str,
    /// The bucket the declaration would have created, which nothing may have created.
    never_created: &'a str,
}

#[tokio::test]
async fn an_include_that_cannot_be_read_fails_the_stack_and_provisions_nothing() {
    let rows: &[Refused<'_>] = &[
        Refused {
            case: "the key the Location names is not in the bucket",
            stack: "include-absent-key",
            // A bucket holding ONE snippet, and a template naming another key in it.
            snippets: &[("snippets/present.yaml", "BucketName: unused\n")],
            template: json!({
                "Resources": {
                    "Data": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "include-absent-key-bucket",
                            "Fn::Transform": include(&location("snippets/absent.yaml"))
                        }
                    }
                }
            })
            .to_string(),
            reason: "Transform AWS::Include failed with: S3 key [snippets/absent.yaml] \
                     does not exist.",
            never_created: "include-absent-key-bucket",
        },
        Refused {
            case: "the bucket the Location names does not exist",
            stack: "include-absent-bucket",
            snippets: &[],
            template: json!({
                "Resources": {
                    "Data": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "include-absent-bucket-bucket",
                            "Fn::Transform": include("s3://no-such-include-bucket/a.json")
                        }
                    }
                }
            })
            .to_string(),
            reason: "Transform AWS::Include failed with: S3 bucket \
                     [no-such-include-bucket] does not exist.",
            never_created: "include-absent-bucket-bucket",
        },
        Refused {
            case: "the Location is not an Amazon S3 URI at all",
            stack: "include-bad-location",
            snippets: &[],
            template: json!({
                "Resources": {
                    "Data": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "include-bad-location-bucket",
                            "Fn::Transform": include("https://example.test/snippet.yaml")
                        }
                    }
                }
            })
            .to_string(),
            reason: "is not an Amazon S3 URI",
            never_created: "include-bad-location-bucket",
        },
        Refused {
            case: "the object is no set of template members, beside members that are",
            stack: "include-scalar-snippet",
            snippets: &[("snippets/scalar.txt", "just some text\n")],
            template: json!({
                "Resources": {
                    "Data": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "include-scalar-bucket",
                            "Fn::Transform": include(&location("snippets/scalar.txt"))
                        }
                    }
                }
            })
            .to_string(),
            reason: "is not a set of template members",
            never_created: "include-scalar-bucket",
        },
        Refused {
            case: "the directive carries no Location to read",
            stack: "include-no-location",
            snippets: &[],
            template: json!({
                "Resources": {
                    "Data": {
                        "Type": "AWS::S3::Bucket",
                        "Properties": {
                            "BucketName": "include-no-location-bucket",
                            "Fn::Transform": {"Name": "AWS::Include", "Parameters": {}}
                        }
                    }
                }
            })
            .to_string(),
            reason: "Parameters.Location must be the Amazon S3 URI",
            never_created: "include-no-location-bucket",
        },
    ];

    for row in rows {
        let router = router();
        seed_snippets(&router, row.snippets).await;

        // "After the call completes successfully, the stack creation starts": the template
        // is accepted and the stack id answered — the transform's failure is the STACK's.
        let (status, created) =
            create_stack(&router, row.stack, &row.template, &acknowledging_the_macro()).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateStack must answer 200, got {status} {created}",
            row.case
        );
        let stack_id = xml_text(&created, "StackId").unwrap_or_default();
        assert!(
            stack_id.contains(row.stack),
            "case {:?}: CreateStack must answer the stack id, got {created}",
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
        let reported = xml_text(&described, "StackStatusReason").unwrap_or_default();
        assert!(
            reported.contains(row.reason),
            "case {:?}: expected the reason to carry {:?}, got {reported:?}",
            row.case,
            row.reason
        );
        // The failure is the TRANSFORM's, not a resource's: nothing was asked of Amazon S3
        // for the resource, so no refusal of it can be what the stack reports.
        assert!(
            !reported.contains("failed to create"),
            "case {:?}: the reason must name the transform rather than a resource, got \
             {reported:?}",
            row.case
        );

        // The same reason is in the stack's event log, which is where a client that polled
        // the create reads it.
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
        assert!(
            reasons.iter().any(|reason| reason.contains(row.reason)),
            "case {:?}: expected an event carrying {:?}, got {reasons:?}",
            row.case,
            row.reason
        );
        // ...and it is an event reason of its OWN, verbatim: processing the template is
        // what failed, so nothing was ever asked of the service that owns the resource and
        // no resource event could carry the diagnostic — the stack's own failed event is
        // where a client polling the create reads what the template could not do. The
        // rollback that failure opens is the SEPARATE event that says the rollback was
        // asked for.
        assert!(
            reasons.contains(&reported),
            "case {:?}: expected an event whose reason is exactly {reported:?}, got \
             {reasons:?}",
            row.case
        );
        let on_the_rollback = rollback_requested(&reported);
        assert!(
            reasons.contains(&on_the_rollback),
            "case {:?}: expected an event whose reason is exactly {on_the_rollback:?}, got \
             {reasons:?}",
            row.case
        );

        // Nothing was provisioned: the transform fails before provisioning, so the
        // resource exists in no service and the stack owns none.
        assert!(
            !bucket_exists(&router, row.never_created).await,
            "case {:?}: nothing may have created {}",
            row.case,
            row.never_created
        );
        assert_eq!(
            physical_id(&router, row.stack, "Data").await,
            None,
            "case {:?}: the stack must own no resource, got one for Data",
            row.case
        );
    }
}

/// An UPDATE whose template cannot be processed changes nothing: the stack keeps the
/// template and the resources it has, and the operation fails with the transform's reason —
/// the same rule as a create's, at the other operation that submits a template body.
#[tokio::test]
async fn an_update_whose_include_cannot_be_read_leaves_the_stack_as_it_was() {
    let router = router();
    seed_snippets(&router, &[("snippets/name.json", r#"{"BucketName":"update-included"}"#)])
        .await;

    let deployed = json!({
        "Resources": {
            "Data": {
                "Type": "AWS::S3::Bucket",
                "Properties": {"Fn::Transform": include(&location("snippets/name.json"))}
            }
        }
    })
    .to_string();
    let (status, created) =
        create_stack(&router, "include-updated", &deployed, &acknowledging_the_macro()).await;
    assert_eq!(
        status, 200,
        "CreateStack must answer 200, got {status} {created}"
    );
    wait_for_the_transition().await;
    assert!(
        bucket_exists(&router, "update-included").await,
        "the create must have provisioned the bucket the snippet names"
    );

    // The same stack, updated with a template whose snippet is not there.
    let broken = json!({
        "Resources": {
            "Data": {
                "Type": "AWS::S3::Bucket",
                "Properties": {"Fn::Transform": include(&location("snippets/gone.json"))}
            }
        }
    })
    .to_string();
    let (status, updated) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "include-updated"),
            member("TemplateBody", &broken),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateStack must answer 200 — the update starts, and the stack reports the \
         outcome — got {status} {updated}"
    );
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, "include-updated").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::UPDATE_FAILED),
        "the update must fail, got {described}"
    );
    let reason = xml_text(&described, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Transform AWS::Include failed with: S3 key [snippets/gone.json] \
                         does not exist."),
        "expected the transform's own reason, got {reason:?}"
    );
    // Nothing changed: the resource the stack had is still there and still its.
    assert!(
        bucket_exists(&router, "update-included").await,
        "the bucket the stack already had must survive the failed update"
    );
    assert_eq!(
        physical_id(&router, "include-updated", "Data").await.as_deref(),
        Some("update-included"),
        "the stack must still report the resource it has, got {described}"
    );
}

/// The capability acknowledgement is held to the PROCESSED template: a resource an included
/// snippet CONTRIBUTES is one the stack would deploy, so a template whose only IAM resource
/// arrives through an include is refused exactly as one that declares it inline is
/// ("you must explicitly acknowledge this by specifying one of these capabilities").
#[tokio::test]
async fn an_include_that_contributes_an_iam_resource_is_held_to_the_acknowledgement() {
    let router = router();
    let role = json!({
        "SnippetRole": {
            "Type": "AWS::IAM::Role",
            "Properties": {
                "RoleName": "include-snippet-role",
                "AssumeRolePolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {"Service": "ec2.amazonaws.com"},
                        "Action": "sts:AssumeRole"
                    }]
                }
            }
        }
    })
    .to_string();
    seed_snippets(&router, &[("snippets/role.json", role.as_str())]).await;

    let template = json!({
        "Resources": {"Fn::Transform": include(&location("snippets/role.json"))}
    })
    .to_string();

    // The macro acknowledged, the IAM resource the snippet contributes NOT: the refusal is
    // the IAM gate's, weighed over the template the include expanded to.
    let (status, refused) =
        create_stack(&router, "include-iam", &template, &acknowledging_the_macro()).await;
    assert_eq!(
        status, 400,
        "a request not acknowledging the IAM resource must be refused, got {status} {refused}"
    );
    assert!(
        refused.contains("InsufficientCapabilitiesException"),
        "expected the modelled refusal, got {refused}"
    );
    let (status, absent) = iam(&router, "GetRole", &[("RoleName", "include-snippet-role")]).await;
    assert_eq!(
        status, 404,
        "the refused request must have created no role, got {status} {absent}"
    );

    // The same template, acknowledged: the snippet's resource is the stack's and IAM holds
    // it.
    let (status, created) = create_stack(
        &router,
        "include-iam",
        &template,
        // Both acknowledgements, each its own member of the list: the macro the template
        // invokes and the named IAM resource its snippet contributes.
        &[
            member("Capabilities.member.1", CAPABILITY_AUTO_EXPAND),
            member("Capabilities.member.2", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "an acknowledged request must be accepted, got {status} {created}"
    );
    wait_for_the_transition().await;
    let (_, described) = describe_stack(&router, "include-iam").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "the acknowledged stack must complete, got {described}"
    );
    let (status, found) = iam(&router, "GetRole", &[("RoleName", "include-snippet-role")]).await;
    assert_eq!(
        status, 200,
        "IAM must hold the role the snippet contributed, got {status} {found}"
    );
}

/// A template naming BOTH kinds of macro reports BOTH expansions at the processed stage: the
/// include only the deploying operation could read (it reaches Amazon S3) and the SAM
/// transform a read expands itself. "CloudFormation processes the template fragments" of
/// every macro it is given, so one that is processed early does not hide the other.
#[tokio::test]
async fn a_template_naming_both_transforms_reports_both_expansions() {
    let router = router();
    seed_snippets(
        &router,
        &[(
            "snippets/table.json",
            r#"{"TableName":"include-sam-table"}"#,
        )],
    )
    .await;
    let template = json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": {
            "Table": {
                "Type": "AWS::Serverless::SimpleTable",
                "Properties": {"Fn::Transform": include(&location("snippets/table.json"))}
            }
        }
    })
    .to_string();

    // The template declares the SAM transform, and "if your stack template contains one or
    // more macros, and you choose to create a stack directly from the processed template,
    // without first reviewing the resulting changes in a change set, you must acknowledge
    // this capability. This includes the AWS::Include and AWS::Serverless transforms\".
    let (status, created) = create_stack(
        &router,
        "include-sam",
        &template,
        &[member("Capabilities.member.1", CAPABILITY_AUTO_EXPAND)],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateStack must answer 200, got {status} {created}"
    );
    wait_for_the_transition().await;

    let (status, body) = call(
        &router,
        "GetTemplate",
        &[
            member("StackName", "include-sam"),
            member("TemplateStage", "Processed"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "GetTemplate(Processed) must answer 200, got {status} {body}"
    );
    for (case, fragment, present) in [
        ("the include", "Fn::Transform", false),
        ("the SAM input type", "AWS::Serverless::SimpleTable", false),
        ("the type the SAM transform produces", "AWS::DynamoDB::Table", true),
        ("the member the snippet contributed", "include-sam-table", true),
    ] {
        assert_eq!(
            body.contains(fragment),
            present,
            "case {case:?}: expected {fragment:?} to be {}present in the processed \
             template, got {body}",
            if present { "" } else { "ab" }
        );
    }
    // And the stack deploys what both expansions produced.
    assert_eq!(
        physical_id(&router, "include-sam", "Table").await.as_deref(),
        Some("include-sam-table"),
        "the stack must report the table the snippet named"
    );
}
