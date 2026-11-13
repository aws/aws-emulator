// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch Logs resources a stack declares INSIDE a log group: the subscription
//! filter that forwards the group's matching events, and what a deployment does to a child
//! whose group changes or whose name another group already uses.
//!
//! RULE under test: a resource type CloudFormation deploys is created in the service that
//! OWNS it through that service's own public API, and a resource that lives INSIDE a parent
//! is identified there by its parent together with its own name — so
//!
//! * an `AWS::Logs::SubscriptionFilter` is really put on the group it names, with the
//!   pattern, the destination and the role the declaration states;
//! * a declaration that names another group REPLACES the child (it is made in the group now
//!   declared and taken out of the one it was in), rather than failing the update;
//! * two children of the same name under DIFFERENT groups are two resources, so one stack
//!   can declare both — including the stack that declares a data-protected group beside a
//!   policy-free one, whose protected group then masks what it ingests.
//!
//! Every case drives the real request path for both services through one router — the
//! CloudFormation calls over the awsQuery wire, the CloudWatch Logs calls over its awsJson1_1
//! wire — so what is asserted is what a client that deploys the template and then reads the
//! resources back observes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text, ACCOUNT_ID, REGION,
};

/// The awsJson version Amazon CloudWatch Logs's model declares (`awsJson1_1`), and the
/// service shape its `X-Amz-Target` names an operation under (`Logs_20140328`).
const LOGS_JSON: &str = "application/x-amz-json-1.1";
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";

/// Call one CloudWatch Logs operation through the same router, signed for `logs` the way a
/// client's SDK signs it: the status and the parsed answer document.
pub(crate) async fn logs(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri(format!("https://logs.{REGION}.amazonaws.com/"))
        .header("content-type", LOGS_JSON)
        .header("x-amz-target", format!("{LOGS_TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/logs/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(request.to_string()))
        .expect("a well-formed CloudWatch Logs request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let body = response.into_body().to_vec();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The subscription filters CloudWatch Logs holds on `group` whose name starts with
/// `prefix`, as `DescribeSubscriptionFilters` reports them.
async fn subscription_filters(
    router: &MultiServiceRouter,
    group: &str,
    prefix: &str,
) -> Vec<Value> {
    let (status, document) = logs(
        router,
        "DescribeSubscriptionFilters",
        &json!({"logGroupName": group, "filterNamePrefix": prefix}),
    )
    .await;
    if status != 200 {
        return Vec::new();
    }
    document
        .get("subscriptionFilters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The names of the streams CloudWatch Logs holds in the group `group`, in the order that
/// service reports them.
async fn stream_names(router: &MultiServiceRouter, group: &str) -> Vec<String> {
    let (status, document) = logs(
        router,
        "DescribeLogStreams",
        &json!({"logGroupName": group}),
    )
    .await;
    if status != 200 {
        return Vec::new();
    }
    document
        .get("logStreams")
        .and_then(Value::as_array)
        .map(|streams| {
            streams
                .iter()
                .filter_map(|stream| {
                    stream
                        .get("logStreamName")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether CloudWatch Logs holds a log group under `name`.
pub(crate) async fn group_exists(router: &MultiServiceRouter, name: &str) -> bool {
    let (status, document) = logs(
        router,
        "DescribeLogGroups",
        &json!({"logGroupNamePrefix": name}),
    )
    .await;
    status == 200
        && document
            .get("logGroups")
            .and_then(Value::as_array)
            .is_some_and(|groups| {
                groups.iter().any(|group| {
                    group.get("logGroupName").and_then(Value::as_str) == Some(name)
                })
            })
}

/// Wall-clock milliseconds, the unit every CloudWatch Logs timestamp member carries.
///
/// `PutLogEvents` admits an event against a clock-relative ingestion window, so a case that
/// writes one stamps it from the clock rather than from a fixed instant.
fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the Unix epoch")
            .as_millis(),
    )
    .expect("the epoch milliseconds fit in an i64")
}

/// One output of a `DescribeStacks` response, read inside its own `<member>` block.
fn output(xml: &str, key: &str) -> Option<String> {
    let block = xml_blocks(xml, "Outputs").into_iter().next()?;
    xml_blocks(&block, "member").into_iter().find_map(|entry| {
        (xml_text(&entry, "OutputKey").as_deref() == Some(key))
            .then(|| xml_text(&entry, "OutputValue"))
            .flatten()
    })
}

/// Every resource failure the stack recorded, so a case whose stack did not complete can say
/// WHY.
async fn failure_reasons(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (_, body) = call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    xml_blocks(&body, "member")
        .into_iter()
        .filter_map(|event| xml_text(&event, "ResourceStatusReason"))
        .collect()
}

/// The ARN of the Kinesis data stream a row's filter delivers to, as that service's own
/// `Fn::GetAtt Arn` resolves it.
fn stream_arn(name: &str) -> String {
    format!("arn:aws:kinesis:{REGION}:{ACCOUNT_ID}:stream/{name}")
}

/// The ARN of the role a row's filter delivers under.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:role/{name}")
}

/// One row of the subscription-filter table: the declaration, and what CloudWatch Logs must
/// then report for the filter.
struct FilterRow {
    case: &'static str,
    stack: &'static str,
    group: &'static str,
    /// The `FilterName` the template states, or `None` where CloudFormation names the filter
    /// (the row then reads the name out of the stack's own output).
    filter_name: Option<&'static str>,
    pattern: &'static str,
    /// Whether the declaration asks for the filter to be applied to the TRANSFORMED version
    /// of each event.
    apply_on_transformed: bool,
    /// The `Distribution` the declaration states, if it states one.
    distribution: Option<&'static str>,
}

fn filter_rows() -> Vec<FilterRow> {
    vec![
        FilterRow {
            case: "a named filter with a word pattern and no distribution",
            stack: "logs-subscription-named",
            group: "subscription-named-group",
            filter_name: Some("named-subscription"),
            pattern: "ERROR",
            apply_on_transformed: false,
            distribution: None,
        },
        FilterRow {
            case: "a filter CloudFormation names, matching every event, by random \
                   distribution",
            stack: "logs-subscription-generated",
            group: "subscription-generated-group",
            filter_name: None,
            pattern: "",
            apply_on_transformed: false,
            distribution: Some("Random"),
        },
        FilterRow {
            case: "a JSON-pattern filter applied on the transformed events",
            stack: "logs-subscription-transformed",
            group: "subscription-transformed-group",
            filter_name: Some("transformed-subscription"),
            pattern: "{ $.level = \"WARN\" }",
            apply_on_transformed: true,
            distribution: None,
        },
    ]
}

/// The template one row deploys: the group, the Kinesis stream the filter delivers to, the
/// role it delivers under, and the filter itself.
fn filter_template(row: &FilterRow) -> String {
    let mut filter = json!({
        "LogGroupName": {"Ref": "Group"},
        "FilterPattern": row.pattern,
        "DestinationArn": {"Fn::GetAtt": ["DestStream", "Arn"]},
        "RoleArn": {"Fn::GetAtt": ["CWLRole", "Arn"]}
    });
    if let Some(name) = row.filter_name {
        filter["FilterName"] = json!(name);
    }
    if row.apply_on_transformed {
        filter["ApplyOnTransformedLogs"] = json!(true);
    }
    if let Some(distribution) = row.distribution {
        filter["Distribution"] = json!(distribution);
    }
    json!({
        "Resources": {
            "Group": {
                "Type": "AWS::Logs::LogGroup",
                "Properties": {"LogGroupName": row.group, "RetentionInDays": 1}
            },
            "Stream": {
                "Type": "AWS::Logs::LogStream",
                "Properties": {
                    "LogGroupName": {"Ref": "Group"},
                    "LogStreamName": "subscription-stream"
                }
            },
            "DestStream": {
                "Type": "AWS::Kinesis::Stream",
                "Properties": {"Name": format!("{}-dest", row.group), "ShardCount": 1}
            },
            "CWLRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": format!("{}-role", row.group),
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": "logs.amazonaws.com"},
                            "Action": "sts:AssumeRole"
                        }]
                    },
                    "Policies": [{
                        "PolicyName": "deliver",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": ["kinesis:PutRecord", "kinesis:PutRecords"],
                                "Resource": {"Fn::GetAtt": ["DestStream", "Arn"]}
                            }]
                        }
                    }]
                }
            },
            "Subscription": {
                "Type": "AWS::Logs::SubscriptionFilter",
                "Properties": filter
            }
        },
        "Outputs": {
            "FilterName": {"Value": {"Ref": "Subscription"}},
            "DestinationArn": {"Value": {"Fn::GetAtt": ["DestStream", "Arn"]}}
        }
    })
    .to_string()
}

/// A declared `AWS::Logs::SubscriptionFilter` is really put on the group it names, with the
/// pattern, the destination and the role its declaration states — and the stack's deletion
/// takes it off the group again.
#[tokio::test]
async fn a_stack_puts_the_subscription_filter_it_declares_on_the_group() {
    for row in filter_rows() {
        let case = row.case;
        let router = router();
        let (status, body) = create_stack(
            &router,
            row.stack,
            &filter_template(&row),
            &[member("Capabilities.member.1", "CAPABILITY_NAMED_IAM")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
        );
        wait_for_the_transition().await;
        // The Kinesis stream a filter delivers to comes up CREATING and is ACTIVE a span
        // later, so the stack that declares one takes a second span to complete.
        wait_for_the_transition().await;

        let (status, body) = describe_stack(&router, row.stack).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete, got {body} with the resource \
             failures {:?}",
            failure_reasons(&router, row.stack).await
        );

        let published = output(&body, "FilterName")
            .unwrap_or_else(|| panic!("case {case:?}: the stack published no filter name"));
        if let Some(declared) = row.filter_name {
            assert_eq!(
                published, declared,
                "case {case:?}: expected the filter's published name to be the declared one"
            );
        }
        let destination = output(&body, "DestinationArn")
            .unwrap_or_else(|| panic!("case {case:?}: the stack published no destination ARN"));
        assert_eq!(
            destination,
            stream_arn(&format!("{}-dest", row.group)),
            "case {case:?}: expected the destination to be the declared stream's ARN"
        );

        let held = subscription_filters(&router, row.group, &published).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected exactly one subscription filter named {published:?} on \
             {:?}, got {held:?}",
            row.group
        );
        let filter = &held[0];
        assert_eq!(
            filter.get("filterPattern").and_then(Value::as_str),
            Some(row.pattern),
            "case {case:?}: expected the declared pattern, got {filter}"
        );
        assert_eq!(
            filter.get("destinationArn").and_then(Value::as_str),
            Some(destination.as_str()),
            "case {case:?}: expected the declared destination, got {filter}"
        );
        assert_eq!(
            filter.get("logGroupName").and_then(Value::as_str),
            Some(row.group),
            "case {case:?}: expected the filter to be on the declared group, got {filter}"
        );
        let expected_role = role_arn(&format!("{}-role", row.group));
        assert_eq!(
            filter.get("roleArn").and_then(Value::as_str),
            Some(expected_role.as_str()),
            "case {case:?}: expected the declared role, got {filter}"
        );
        assert_eq!(
            filter.get("applyOnTransformedLogs").and_then(Value::as_bool),
            Some(row.apply_on_transformed),
            "case {case:?}: expected the declared applyOnTransformedLogs, got {filter}"
        );
        // `Distribution` is optional on the type; the owning service reports the default it
        // documents ("By default, log data is grouped by log stream") for a filter whose
        // declaration states none.
        assert_eq!(
            filter.get("distribution").and_then(Value::as_str),
            Some(row.distribution.unwrap_or("ByLogStream")),
            "case {case:?}: expected the declared distribution, got {filter}"
        );

        // The stack's deletion takes the filter off the group — asked while the group is
        // still there, because a deleted group holds nothing to ask about.
        let (status, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with {body}"
        );
        wait_for_the_transition().await;
        assert!(
            !group_exists(&router, row.group).await,
            "case {case:?}: expected the deleted stack to have released the log group"
        );
    }
}

/// Two children of the same NAME under different log groups are two resources: one stack can
/// declare both, and each is created in its own group.
#[tokio::test]
async fn one_stack_declares_the_same_child_name_in_two_groups() {
    let router = router();
    let stack = "logs-children-two-groups";
    let template = json!({
        "Resources": {
            "First": {
                "Type": "AWS::Logs::LogGroup",
                "Properties": {"LogGroupName": "two-groups-first", "RetentionInDays": 1}
            },
            "Second": {
                "Type": "AWS::Logs::LogGroup",
                "Properties": {"LogGroupName": "two-groups-second", "RetentionInDays": 1}
            },
            "FirstStream": {
                "Type": "AWS::Logs::LogStream",
                "Properties": {"LogGroupName": {"Ref": "First"}, "LogStreamName": "app"}
            },
            "SecondStream": {
                "Type": "AWS::Logs::LogStream",
                "Properties": {"LogGroupName": {"Ref": "Second"}, "LogStreamName": "app"}
            },
            "FirstFilter": {
                "Type": "AWS::Logs::MetricFilter",
                "Properties": {
                    "FilterName": "shared",
                    "LogGroupName": {"Ref": "First"},
                    "FilterPattern": "ERROR",
                    "MetricTransformations": [{
                        "MetricName": "Errors",
                        "MetricNamespace": "Platform/Logs",
                        "MetricValue": "1"
                    }]
                }
            },
            "SecondFilter": {
                "Type": "AWS::Logs::MetricFilter",
                "Properties": {
                    "FilterName": "shared",
                    "LogGroupName": {"Ref": "Second"},
                    "FilterPattern": "ERROR",
                    "MetricTransformations": [{
                        "MetricName": "Errors",
                        "MetricNamespace": "Platform/Logs",
                        "MetricValue": "1"
                    }]
                }
            }
        }
    })
    .to_string();
    let case = "one stream and one metric filter of the same name in each of two groups";
    let (status, body) = create_stack(&router, stack, &template, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body} with the resource \
         failures {:?}",
        failure_reasons(&router, stack).await
    );
    for group in ["two-groups-first", "two-groups-second"] {
        let held = stream_names(&router, group).await;
        assert_eq!(
            held,
            vec!["app".to_string()],
            "case {case:?}: expected the stream to be in {group:?}, got {held:?}"
        );
        let (status, document) = logs(
            &router,
            "DescribeMetricFilters",
            &json!({"logGroupName": group, "filterNamePrefix": "shared"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeMetricFilters for {group:?}, got \
             {status} with {document}"
        );
        let names: Vec<String> = document
            .get("metricFilters")
            .and_then(Value::as_array)
            .map(|filters| {
                filters
                    .iter()
                    .filter_map(|filter| {
                        filter
                            .get("filterName")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names,
            vec!["shared".to_string()],
            "case {case:?}: expected the filter to be on {group:?}, got {names:?}"
        );
    }
}

/// A stack declaring a group with a `DataProtectionPolicy` beside a policy-free sibling, and
/// a stream of the same name in each, completes — and the policy it declared is in force:
/// `GetDataProtectionPolicy` reports the declared document, the protected group masks what it
/// ingests while the sibling does not, and `unmask` recovers the ingested message.
#[tokio::test]
async fn a_declared_data_protection_policy_masks_what_the_group_ingests() {
    let router = router();
    let stack = "logs-protected-group";
    let (protected, control) = ("protected-group", "control-group");
    let stream = "app";
    let identifiers = ["arn:aws:dataprotection::aws:data-identifier/EmailAddress"];
    let policy = json!({
        "Name": "declared-protection-policy",
        "Version": "2021-06-01",
        "Statement": [
            {
                "Sid": "audit",
                "DataIdentifier": identifiers,
                "Operation": {"Audit": {"FindingsDestination": {}}}
            },
            {
                "Sid": "mask",
                "DataIdentifier": identifiers,
                "Operation": {"Deidentify": {"MaskConfig": {}}}
            }
        ]
    });
    let group = |name: &str, with_policy: bool| {
        let mut properties = json!({
            "LogGroupName": name,
            "RetentionInDays": 1,
            "Tags": [{"Key": "Suite", "Value": "protected-groups"}]
        });
        if with_policy {
            properties["DataProtectionPolicy"] = policy.clone();
        }
        json!({"Type": "AWS::Logs::LogGroup", "Properties": properties})
    };
    let template = json!({
        "Resources": {
            "Protected": group(protected, true),
            "Control": group(control, false),
            "ProtectedStream": {
                "Type": "AWS::Logs::LogStream",
                "Properties": {
                    "LogGroupName": {"Ref": "Protected"},
                    "LogStreamName": stream
                }
            },
            "ControlStream": {
                "Type": "AWS::Logs::LogStream",
                "Properties": {
                    "LogGroupName": {"Ref": "Control"},
                    "LogStreamName": stream
                }
            }
        }
    })
    .to_string();

    let case = "a data-protected group beside a policy-free sibling";
    let (status, body) = create_stack(&router, stack, &template, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body} with the resource \
         failures {:?}",
        failure_reasons(&router, stack).await
    );

    // The policy the declaration carried is the one CloudWatch Logs holds for the group.
    let (status, document) = logs(
        &router,
        "GetDataProtectionPolicy",
        &json!({"logGroupIdentifier": protected}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the protected group to hold a policy, got {status} with \
         {document}"
    );
    let reported: Value = document
        .get("policyDocument")
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(Value::Null);
    assert_eq!(
        reported.get("Name"),
        policy.get("Name"),
        "case {case:?}: expected the declared document, got {document}"
    );

    // An ingested message carrying the named data is served masked by the protected group,
    // verbatim by the sibling, and `unmask` recovers it.
    let ingested = "signup from ada@example.com accepted";
    for name in [protected, control] {
        let (status, document) = logs(
            &router,
            "PutLogEvents",
            &json!({
                "logGroupName": name,
                "logStreamName": stream,
                "logEvents": [{"timestamp": now_ms(), "message": ingested}]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {name:?} to accept the event, got {status} with {document}"
        );
    }
    let read = |name: &'static str, unmask: bool| {
        let router = &router;
        async move {
            let (status, document) = logs(
                router,
                "GetLogEvents",
                &json!({
                    "logGroupName": name,
                    "logStreamName": stream,
                    "unmask": unmask,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {name:?} to be readable, got {status} with {document}"
            );
            document
                .get("events")
                .and_then(Value::as_array)
                .map(|events| {
                    events
                        .iter()
                        .filter_map(|event| {
                            event
                                .get("message")
                                .and_then(Value::as_str)
                                .map(ToString::to_string)
                        })
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default()
        }
    };
    assert_eq!(
        read(protected, false).await,
        vec!["signup from *************** accepted".to_string()],
        "case {case:?}: expected the protected group to mask the matched span"
    );
    assert_eq!(
        read(protected, true).await,
        vec![ingested.to_string()],
        "case {case:?}: expected unmask to recover the ingested message"
    );
    assert_eq!(
        read(control, false).await,
        vec![ingested.to_string()],
        "case {case:?}: expected the policy-free sibling to serve the message verbatim"
    );
}

/// The template of the move case: two groups, and one subscription filter on the group
/// `on_group` of them, delivering to the stream both revisions declare.
fn moved_filter_template(on_group: &str) -> String {
    json!({
        "Resources": {
            "First": {
                "Type": "AWS::Logs::LogGroup",
                "Properties": {"LogGroupName": "moved-filter-first", "RetentionInDays": 1}
            },
            "Second": {
                "Type": "AWS::Logs::LogGroup",
                "Properties": {"LogGroupName": "moved-filter-second", "RetentionInDays": 1}
            },
            "DestStream": {
                "Type": "AWS::Kinesis::Stream",
                "Properties": {"Name": "moved-filter-dest", "ShardCount": 1}
            },
            "CWLRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": "moved-filter-role",
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": "logs.amazonaws.com"},
                            "Action": "sts:AssumeRole"
                        }]
                    },
                    "Policies": [{
                        "PolicyName": "deliver",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": ["kinesis:PutRecord", "kinesis:PutRecords"],
                                "Resource": {"Fn::GetAtt": ["DestStream", "Arn"]}
                            }]
                        }
                    }]
                }
            },
            "Subscription": {
                "Type": "AWS::Logs::SubscriptionFilter",
                "Properties": {
                    "FilterName": "moved-subscription",
                    "LogGroupName": {"Ref": on_group},
                    "FilterPattern": "ERROR",
                    "DestinationArn": {"Fn::GetAtt": ["DestStream", "Arn"]},
                    "RoleArn": {"Fn::GetAtt": ["CWLRole", "Arn"]}
                }
            }
        }
    })
    .to_string()
}

/// A declaration that names ANOTHER log group for a subscription filter replaces it: the
/// filter is put on the group now declared and taken off the one it was on, and the update
/// completes.
#[tokio::test]
async fn pointing_a_subscription_filter_at_another_group_replaces_it() {
    let router = router();
    let stack = "logs-subscription-moved";
    let case = "the first revision, which puts the filter on the first group";
    let (status, body) = create_stack(
        &router,
        stack,
        &moved_filter_template("First"),
        &[member("Capabilities.member.1", "CAPABILITY_NAMED_IAM")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    wait_for_the_transition().await;
    // The Kinesis stream the filter delivers to is ACTIVE a span after it is created.
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body} with the resource \
         failures {:?}",
        failure_reasons(&router, stack).await
    );
    let held = subscription_filters(&router, "moved-filter-first", "moved-subscription").await;
    assert_eq!(
        held.len(),
        1,
        "case {case:?}: expected the filter on the first group, got {held:?}"
    );

    let case = "the revision that points the filter at the second group";
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &moved_filter_template("Second")),
            member("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the move to complete, got {body} with the resource \
         failures {:?}",
        failure_reasons(&router, stack).await
    );
    let held = subscription_filters(&router, "moved-filter-second", "moved-subscription").await;
    assert_eq!(
        held.len(),
        1,
        "case {case:?}: expected the filter on the group now declared, got {held:?}"
    );
    assert_eq!(
        held[0].get("logGroupName").and_then(Value::as_str),
        Some("moved-filter-second"),
        "case {case:?}: expected the filter to name the group now declared, got {:?}",
        held[0]
    );
    let left_behind =
        subscription_filters(&router, "moved-filter-first", "moved-subscription").await;
    assert!(
        left_behind.is_empty(),
        "case {case:?}: expected the filter to have been taken off the group it was on, got \
         {left_behind:?}"
    );
}

/// The template of the rename case: a named group with one stream in it, and the group's ARN
/// as an output.
fn rename_template(group: &str) -> String {
    json!({
        "Resources": {
            "LogGroup": {
                "Type": "AWS::Logs::LogGroup",
                "Properties": {"LogGroupName": group, "RetentionInDays": 1}
            },
            "LogStream": {
                "Type": "AWS::Logs::LogStream",
                "Properties": {
                    "LogGroupName": {"Ref": "LogGroup"},
                    "LogStreamName": "renamed-stream"
                }
            }
        },
        "Outputs": {
            "GroupName": {"Value": {"Ref": "LogGroup"}},
            "GroupArn": {"Value": {"Fn::GetAtt": ["LogGroup", "Arn"]}}
        }
    })
    .to_string()
}

/// Renaming a declared log group REPLACES the group and the stream inside it: the update
/// completes, the old group is gone, and the replacement group holds the recreated stream
/// with none of the old group's events.
#[tokio::test]
async fn renaming_a_declared_log_group_replaces_the_group_and_its_stream() {
    let router = router();
    let stack = "logs-group-rename";
    let (old_group, new_group) = ("rename-before", "rename-after");
    let case = "the first revision, which creates the group and its stream";
    let (status, body) = create_stack(&router, stack, &rename_template(old_group), &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    assert_eq!(
        output(&body, "GroupName").as_deref(),
        Some(old_group),
        "case {case:?}: expected the stack to publish the declared group name, got {body}"
    );

    // One event in the old group, read back so the replacement can be shown to carry none of
    // them.
    let (status, document) = logs(
        &router,
        "PutLogEvents",
        &json!({
            "logGroupName": old_group,
            "logStreamName": "renamed-stream",
            "logEvents": [{"timestamp": now_ms(), "message": "before the rename"}]
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the old group to accept an event, got {status} with {document}"
    );
    let (_, document) = logs(
        &router,
        "GetLogEvents",
        &json!({"logGroupName": old_group, "logStreamName": "renamed-stream"}),
    )
    .await;
    assert_eq!(
        document["events"][0]["message"],
        json!("before the rename"),
        "case {case:?}: expected the old group to serve the event it took, got {document}"
    );

    let case = "the revision that renames the group";
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &rename_template(new_group)),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with {body}"
    );
    wait_for_the_transition().await;
    let (_, body) = describe_stack(&router, stack).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the rename to complete, got {body} with the resource \
         failures {:?}",
        failure_reasons(&router, stack).await
    );
    assert_eq!(
        output(&body, "GroupName").as_deref(),
        Some(new_group),
        "case {case:?}: expected the stack to publish the replacement group's name, got {body}"
    );
    assert_eq!(
        output(&body, "GroupArn"),
        Some(format!(
            "arn:aws:logs:{REGION}:{ACCOUNT_ID}:log-group:{new_group}:*"
        )),
        "case {case:?}: expected the published ARN to name the replacement group, got {body}"
    );

    assert!(
        !group_exists(&router, old_group).await,
        "case {case:?}: expected the replaced group to have been deleted"
    );
    let (status, document) = logs(
        &router,
        "GetLogEvents",
        &json!({"logGroupName": old_group, "logStreamName": "renamed-stream"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a read of the replaced group to be refused, got {status} \
         with {document}"
    );
    assert_eq!(
        document.get("message").and_then(Value::as_str),
        Some("The specified log group does not exist."),
        "case {case:?}: expected the modelled refusal of a group that does not exist, got \
         {document}"
    );

    let held = stream_names(&router, new_group).await;
    assert_eq!(
        held,
        vec!["renamed-stream".to_string()],
        "case {case:?}: expected the replacement group to hold exactly the recreated stream, \
         got {held:?}"
    );
    let (status, document) = logs(
        &router,
        "GetLogEvents",
        &json!({"logGroupName": new_group, "logStreamName": "renamed-stream"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the replacement stream to be readable, got {status} with \
         {document}"
    );
    assert_eq!(
        document
            .get("events")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "case {case:?}: expected the recreated stream to carry none of the old group's \
         events, got {document}"
    );
    let (status, document) = logs(
        &router,
        "PutLogEvents",
        &json!({
            "logGroupName": new_group,
            "logStreamName": "renamed-stream",
            "logEvents": [{"timestamp": now_ms(), "message": "after the rename"}]
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the replacement group to be writable, got {status} with \
         {document}"
    );
}
