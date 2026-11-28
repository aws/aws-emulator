// SPDX-License-Identifier: CC0-1.0

//! HOW the CloudWatch Logs grant reaches a function's execution role, and what a synchronous
//! invocation then leaves in the function's log group.
//!
//! RULE: a successful synchronous `Invoke` of a function whose execution role's POLICY SET
//! allows `logs:CreateLogGroup`, `logs:CreateLogStream` and `logs:PutLogEvents` creates the
//! function's log group if it is absent — the one its `LoggingConfig$LogGroup` names, else the
//! default `/aws/lambda/<function name>` — opens a stream of that invocation in it, and writes
//! that run's log into it as one event per line, the `START RequestId: <id> Version: <version>`
//! frame, every line the handler wrote, the `END RequestId: <id>` frame and the
//! `REPORT RequestId: <id>` line that states what the run spent; the events are readable by the
//! time the call is answered. The decision is made on the policy SET in force on the role, so it
//! does not depend on which document carries the grant: an inline policy, the AWS-managed
//! `AWSLambdaBasicExecutionRole` attached to the role, or a customer-managed policy attached to
//! it each carry it, and a role whose only attached document grants nothing on CloudWatch Logs
//! carries it from nowhere, which leaves the group uncreated and the invocation answered all
//! the same.
//!
//! What the neighbouring suites already pin, and this one does not repeat:
//! [`crate::invocation_log_delivery`] walks what is written and which of the three writes an
//! INLINE policy allows, and [`crate::invocation_log_group_search`] walks the group-wide search
//! that finds it. What only this suite states is that the CARRIER of the grant is not part of
//! the rule — the case a client creating a role the way AWS's own console and every quickstart
//! does (`CreateRole`, then `AttachRolePolicy` with the managed basic-execution policy) depends
//! on — and that a synchronous invocation's events are in the group when its call returns,
//! without a poll.
//!
//! Every row drives real HTTP through the production `MultiServiceRouter` with Lambda,
//! CloudWatch Logs and IAM mounted on ONE engine, reusing that suite's harness so the two
//! cannot drift about what a delivery is. The rows differ along what the rule generalises over:
//! which document carries the grant (an attached AWS-managed policy, that policy beside an
//! inline policy about another service, an attached customer-managed policy, and an attached
//! AWS-managed policy carrying no logs grant at all), which RUNTIME ran (python and node), how
//! many lines the handler wrote (one, three), and which group the log goes to (the default, and
//! one the function's `LoggingConfig` names).
//!
//! Every test here RUNS handlers, so each takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::{allow, element, given_role, iam};
use crate::invocation_log_delivery::{
    create_function, delivered, emulator, events, groups, invocations_log, invoke, record_text,
    streams, Created, LOGS, LOG_WRITES, NODE_RUNTIME, PYTHON_RUNTIME,
};
// The reader a client polls a delivered log with, owned by the suite that first needed it
// rather than written out a second time here.
use crate::invoke_log_delivery::filtered;

/// The AWS-managed policy a Lambda execution role is created with by AWS's own console and by
/// every quickstart: it grants exactly the three log writes a function's runtime makes. Source:
/// the policy of that name in IAM's own managed-policy catalog, at the `/service-role/` path.
const BASIC_EXECUTION_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole";

/// ANOTHER AWS-managed policy, attachable to the same role and granting nothing on CloudWatch
/// Logs: a role carrying only it is a role with a policy set, so what it leaves behind is an
/// implicit deny about the log writes rather than an empty set. Source: the policy of that name
/// in the same catalog, at the `/` path.
const UNRELATED_MANAGED_POLICY: &str = "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess";

/// The fixed text of the framing the runtime writes around a run's own output, and the fixed
/// field names of its closing report. Source: the lines Lambda writes for one invocation —
/// `START RequestId: <id> Version: <version>`, `END RequestId: <id>` and `REPORT RequestId:
/// <id>` with the run's spend as tab-separated named fields.
const START_PREFIX: &str = "START RequestId: ";
const END_PREFIX: &str = "END RequestId: ";
const REPORT_PREFIX: &str = "REPORT RequestId: ";
const VERSION_FIELD: &str = " Version: ";
// The report's fields this suite reads are the ones the runtime writes today. AWS closes the
// line with a `Max Memory Used:` field too, which is NOT part of what this suite states: the
// open issue `20261107T035448Z-lambda-logs-3-net-new-causes-loggingconfig-loggroup.md` claims
// that field as a cause of its own, so its absence here is that issue's and not this rule's.
const REPORT_FIELDS: &[&str] = &["Duration:", "Billed Duration:", "Memory Size:"];

/// The version an invocation that names none resolves to and reports as the one that executed.
const UNPUBLISHED_VERSION: &str = "$LATEST";

/// Which document carries the logs grant onto the execution role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carrier {
    /// `AttachRolePolicy` with [`BASIC_EXECUTION_POLICY`].
    AwsManaged,
    /// The same attachment, beside a `PutRolePolicy` inline document about another service.
    AwsManagedBesideInline,
    /// `CreatePolicy` with the three log writes, then `AttachRolePolicy` with what it minted.
    CustomerManaged,
    /// `AttachRolePolicy` with [`UNRELATED_MANAGED_POLICY`]: a policy set carrying no log write.
    NoLogsGrant,
}

/// One row: which document carries the grant, what the function runs, and what the run then
/// leaves in its log group.
struct Row {
    case: &'static str,
    /// The document the grant arrives on.
    carrier: Carrier,
    /// The handler's source.
    source: &'static str,
    /// The runtime that runs it.
    runtime: &'static str,
    /// The lines the HANDLER itself writes, each of which has to be one event of its own.
    written: &'static [&'static str],
    /// The group the function's configuration names, or `None` to take the default.
    log_group: Option<&'static str>,
    /// A `filterPattern` term every one of `written` carries and no framing line does, as a
    /// quoted term — the reader a client polls with.
    marker: &'static str,
    /// Whether the run's events have to be in that group once the call is answered.
    delivers: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the managed basic-execution policy attached, and nothing else",
            carrier: Carrier::AwsManaged,
            source: "def lambda_handler(event, context):\n    print('carrier-aws-managed')\n    \
                     return {'ok': True}\n",
            runtime: PYTHON_RUNTIME,
            written: &["carrier-aws-managed"],
            log_group: None,
            marker: "\"carrier-aws-managed\"",
            delivers: true,
        },
        Row {
            case: "that policy attached beside an inline policy about another service",
            carrier: Carrier::AwsManagedBesideInline,
            source: "exports.handler = async () => {\n    for (let index = 0; index < 3; index \
                     += 1) {\n        console.log('carrier-beside-inline-' + index);\n    }\n   \
                     return { ok: true };\n};\n",
            runtime: NODE_RUNTIME,
            written: &[
                "carrier-beside-inline-0",
                "carrier-beside-inline-1",
                "carrier-beside-inline-2",
            ],
            log_group: None,
            marker: "\"carrier-beside-inline-\"",
            delivers: true,
        },
        Row {
            case: "a customer-managed policy carrying the three writes, attached",
            carrier: Carrier::CustomerManaged,
            source: "exports.handler = async () => {\n    \
                     console.log('carrier-customer-managed');\n    return { ok: true };\n};\n",
            runtime: NODE_RUNTIME,
            written: &["carrier-customer-managed"],
            log_group: Some("/aws/vendedlogs/lambda-grant-carrier"),
            marker: "\"carrier-customer-managed\"",
            delivers: true,
        },
        Row {
            case: "an attached managed policy that grants nothing on CloudWatch Logs",
            carrier: Carrier::NoLogsGrant,
            source: "exports.handler = async () => {\n    console.log('carrier-no-grant');\n    \
                     return { ok: true };\n};\n",
            runtime: NODE_RUNTIME,
            written: &["carrier-no-grant"],
            log_group: None,
            marker: "\"carrier-no-grant\"",
            delivers: false,
        },
    ]
}

/// Create the role this row's function runs as, with the grant on the document the row names,
/// and answer the ARN IAM persisted for it.
async fn role_carrying(router: &MultiServiceRouter, case: &str, name: &str, row: &Row) -> String {
    let inline = match row.carrier {
        // "beside an inline policy about another service": a document that states a grant of
        // its own, so the role's set is not the attachment alone.
        Carrier::AwsManagedBesideInline => Some(json!([allow("s3", &["GetObject"], "*")])),
        _ => None,
    };
    let role = given_role(router, case, name, inline).await;
    let attached = match row.carrier {
        Carrier::AwsManaged | Carrier::AwsManagedBesideInline => {
            BASIC_EXECUTION_POLICY.to_string()
        }
        Carrier::NoLogsGrant => UNRELATED_MANAGED_POLICY.to_string(),
        Carrier::CustomerManaged => {
            customer_managed_policy(router, case, &format!("{name}-log-writes")).await
        }
    };
    let (status, answered) = iam(
        router,
        "AttachRolePolicy",
        &[("RoleName", name), ("PolicyArn", attached.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected AttachRolePolicy {attached:?} to answer 200, got {status}: \
         {answered}"
    );
    role
}

/// A CUSTOMER-managed policy whose one statement carries the three log writes, answering the ARN
/// IAM minted for it — read off IAM's own answer rather than spelled out here.
async fn customer_managed_policy(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [allow(LOGS, &LOG_WRITES, "*")],
    })
    .to_string();
    let (status, created) = iam(
        router,
        "CreatePolicy",
        &[("PolicyName", name), ("PolicyDocument", &document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreatePolicy {name:?} to answer 200, got {status}: {created}"
    );
    element(&created, "Arn")
}

/// The request id a framing line names: everything after `prefix` up to the first field the
/// line goes on to state.
fn framed_id(line: &str, prefix: &str) -> String {
    line.strip_prefix(prefix)
        .unwrap_or_default()
        .split(['\t', ' '])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// However the grant reaches the execution role, a synchronous invocation's log is in the
/// function's group by the time the call is answered — and a role carrying no log grant at all
/// leaves the group uncreated without failing the invocation.
#[tokio::test]
async fn the_document_carrying_the_logs_grant_does_not_change_what_an_invocation_logs() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let role = role_carrying(&router, case, &format!("log-carrier-{index}"), row).await;
        let name = format!("log-carried-{index}");
        let group = row
            .log_group
            .map_or_else(|| format!("/aws/lambda/{name}"), str::to_string);
        create_function(
            &router,
            &Created {
                case,
                name: &name,
                source: row.source,
                runtime: row.runtime,
                role: &role,
                log_group: row.log_group,
            },
        )
        .await;

        // The group is the first invocation's doing, not the create's.
        let before = groups(&router, case, &group).await;
        assert!(
            before.is_empty(),
            "case {case:?}: expected no log group before the first invocation, got {before:?}"
        );

        // The invocation is answered whatever became of its log, and what it reported as its
        // own log tail is the independent statement of what the delivery had to write.
        let (reported, payload) = invoke(&router, case, &name).await;
        assert_eq!(
            payload["ok"],
            json!(true),
            "case {case:?}: expected the handler's own answer, got {payload}"
        );
        for written in row.written {
            assert!(
                reported.contains(written),
                "case {case:?}: expected the run's own log to carry {written:?}, got {reported:?}"
            );
        }

        let found = groups(&router, case, &group).await;
        if !row.delivers {
            assert!(
                found.is_empty(),
                "case {case:?}: expected a role with no logs grant to leave {group:?} \
                 uncreated, got {found:?}"
            );
            continue;
        }
        assert_eq!(
            found,
            vec![group.clone()],
            "case {case:?}: expected the invocation to create {group:?}, got {found:?}"
        );

        // One invocation, one stream of it — read with no poll: the delivery is made before the
        // synchronous call is answered.
        let held = streams(&router, case, &group).await;
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected one stream for the one invocation, got {held:?}"
        );
        let stream = held[0].clone();
        let (lines, _text) = delivered(&router, case, &group, &stream).await;
        let rebuilt = invocations_log(&lines);
        assert_eq!(
            rebuilt, reported,
            "case {case:?}: expected the group to hold the log the invocation reported, got \
             {rebuilt:?} against {reported:?}"
        );

        // The framing, the handler's own lines between it, and nothing else — opened by the
        // initialization of the execution environment the run built.
        assert_eq!(
            lines.len(),
            row.written.len() + 4,
            "case {case:?}: expected 4 framing lines around {} of the handler's own, got \
             {lines:?}",
            row.written.len()
        );
        assert!(
            lines
                .first()
                .is_some_and(|line| line.starts_with("INIT_START Runtime Version: ")),
            "case {case:?}: expected the log to open on the environment's initialization, got \
             {lines:?}"
        );
        let opening = lines.get(1).cloned().unwrap_or_default();
        assert!(
            opening.starts_with(START_PREFIX)
                && opening.ends_with(&format!("{VERSION_FIELD}{UNPUBLISHED_VERSION}")),
            "case {case:?}: expected the invocation's START line naming the version that \
             executed, got {opening:?}"
        );
        for (position, written) in row.written.iter().enumerate() {
            let actual = lines.get(position + 2).cloned().unwrap_or_default();
            // A node runtime delivers a console line FRAMED, so the text the handler wrote
            // is the last of the line's tab-separated fields.
            let text = actual.rsplit('\t').next().unwrap_or_default();
            assert_eq!(
                text, *written,
                "case {case:?}: expected the handler's line {position} to be {written:?}, got \
                 {actual:?}"
            );
        }
        let closing = lines[lines.len() - 2].clone();
        let report = lines[lines.len() - 1].clone();
        assert!(
            closing.starts_with(END_PREFIX) && report.starts_with(REPORT_PREFIX),
            "case {case:?}: expected the log to close on END and then REPORT, got \
             {closing:?} and {report:?}"
        );
        for field in REPORT_FIELDS {
            assert!(
                report.contains(field),
                "case {case:?}: expected the report to state {field:?}, got {report:?}"
            );
        }

        // Every framing line names the SAME invocation, and the stream names it too — which is
        // what lets a client tie a stream to the call it made.
        let request_id = framed_id(&opening, START_PREFIX);
        let framed = [
            framed_id(&closing, END_PREFIX),
            framed_id(&report, REPORT_PREFIX),
        ];
        assert!(
            !request_id.is_empty() && framed.iter().all(|id| *id == request_id),
            "case {case:?}: expected one invocation id across the framing, got {request_id:?} \
             against {framed:?}"
        );
        let expected_tail = format!("/[{UNPUBLISHED_VERSION}]{}", request_id.replace('-', ""));
        assert!(
            stream.ends_with(&expected_tail),
            "case {case:?}: expected the stream to end on {expected_tail:?}, got {stream:?}"
        );

        // Every event carries an instant of the run, in order.
        let stamps: Vec<i64> = events(&router, case, &group, &stream)
            .await
            .iter()
            .map(|(at, _)| *at)
            .collect();
        assert!(
            stamps.iter().all(|at| *at > 0) && stamps.windows(2).all(|pair| pair[0] <= pair[1]),
            "case {case:?}: expected ordered instants of the run, got {stamps:?}"
        );

        // And the reader a client actually polls finds the handler's own lines, and only those.
        let matched = filtered(&router, case, &group, row.marker).await;
        let texts: Vec<String> = matched
            .iter()
            .map(|line| {
                record_text(line)
                    .rsplit('\t')
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let expected: Vec<String> = row.written.iter().map(|line| (*line).to_string()).collect();
        assert_eq!(
            texts, expected,
            "case {case:?}: expected the search {:?} to return the handler's own lines, got \
             {matched:?}",
            row.marker
        );
    }
}

/// The grant a role carries is read at the INSTANT of the invocation, so attaching the managed
/// basic-execution policy to a role that had no logs grant makes the NEXT invocation of the same
/// function deliver its log — and nothing of the invocation that ran before the attachment
/// appears, because that run's writes were withheld.
#[tokio::test]
async fn attaching_the_managed_policy_makes_the_next_invocation_deliver_its_log() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a logs grant attached between two invocations";
    let router = emulator();
    let role_name = "log-carrier-attached-late";
    let role = given_role(&router, case, role_name, None).await;
    let (status, answered) = iam(
        &router,
        "AttachRolePolicy",
        &[
            ("RoleName", role_name),
            ("PolicyArn", UNRELATED_MANAGED_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the unrelated policy to attach, got {status}: {answered}"
    );

    let name = "log-carried-late";
    let group = format!("/aws/lambda/{name}");
    create_function(
        &router,
        &Created {
            case,
            name,
            source: "exports.handler = async () => {\n    \
                     console.log('carrier-attached-late');\n    return { ok: true };\n};\n",
            runtime: NODE_RUNTIME,
            role: &role,
            log_group: None,
        },
    )
    .await;

    // The first invocation runs under a role with no logs grant, so it leaves nothing.
    invoke(&router, case, name).await;
    let before = groups(&router, case, &group).await;
    assert!(
        before.is_empty(),
        "case {case:?}: expected the first run to leave {group:?} uncreated, got {before:?}"
    );

    let (status, answered) = iam(
        &router,
        "AttachRolePolicy",
        &[
            ("RoleName", role_name),
            ("PolicyArn", BASIC_EXECUTION_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the basic-execution policy to attach, got {status}: {answered}"
    );

    // The next invocation is decided on the policy set in force NOW, so it delivers.
    let (reported, _) = invoke(&router, case, name).await;
    let found = groups(&router, case, &group).await;
    assert_eq!(
        found,
        vec![group.clone()],
        "case {case:?}: expected the invocation after the attachment to create {group:?}, got \
         {found:?}"
    );
    let held = streams(&router, case, &group).await;
    assert_eq!(
        held.len(),
        1,
        "case {case:?}: expected only the delivering run's stream, got {held:?}"
    );
    let (lines, _text) = delivered(&router, case, &group, &held[0]).await;
    let rebuilt = invocations_log(&lines);
    assert_eq!(
        rebuilt, reported,
        "case {case:?}: expected the group to hold the second run's log, got {rebuilt:?} \
         against {reported:?}"
    );
    assert!(
        lines.first().is_some_and(|line| line.starts_with(START_PREFIX)),
        "case {case:?}: expected the delivered log to open on the invocation's own START \
         line, got {lines:?}"
    );
    // The initialization record is the ENVIRONMENT's, written once ahead of the first
    // invocation it serves ([`crate::environment_init_record_opens_the_stream`]): this
    // environment was initialized while the role still carried no logs grant, so what is
    // delivered here is the invocation's own log and not a second initialization.
    assert!(
        !lines
            .iter()
            .any(|line| line.starts_with("INIT_START Runtime Version: ")),
        "case {case:?}: expected no initialization record for a run that paid none, got \
         {lines:?}"
    );
}
