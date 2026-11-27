// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetDeliveryDestinationPolicy` as a REPORT: what it answers, where
//! it looks for the resource it names, and what it leaves behind.
//!
//! The rule under test: `GetDeliveryDestinationPolicy` answers from stored state — the
//! delivery destination policy the last assignment stored on the delivery destination the
//! request names, and no `policy` member at all for a destination none has been assigned to
//! — resolving that name in the delivery-destination namespace of this account EXACTLY, so a
//! name the account holds no delivery destination under is refused as a missing resource
//! however many other resources of this service carry it; and, being a report, it writes
//! nothing, so repeating it and interleaving it with other reads leaves every caller
//! afterwards — itself included — seeing exactly the state the reads started from.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire, and a response that is not computed
//! from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: destinations of
//! several `DeliveryDestinationType`s, names at both published bounds of
//! `DeliveryDestinationName`, destinations carrying a policy and destinations carrying none
//! in the same store, policy documents of several forms and character sets, a name held by
//! another kind of resource of this service (a delivery source, a log group and a logical
//! destination) and near-miss spellings of a held name.
//!
//! `tests/all/delivery_destination_policies.rs` is the assignment side of the same state:
//! that a policy is stored and reported as the document the assignment read, replaced by the
//! next assignment, and refused when it is not an IAM policy document.

use serde_json::{json, Value};

use crate::harness::{Logs, REGION};

/// The operation under test, plus the operations a round trip and the namespace cases go
/// through.
const GET_POLICY: &str = "GetDeliveryDestinationPolicy";
const PUT_POLICY: &str = "PutDeliveryDestinationPolicy";
const PUT_DESTINATION: &str = "PutDeliveryDestination";
const GET_DESTINATION: &str = "GetDeliveryDestination";
const PUT_SOURCE: &str = "PutDeliverySource";
const GET_SOURCE: &str = "GetDeliverySource";
const PUT_LOGICAL_DESTINATION: &str = "PutDestination";
const CREATE_LOG_GROUP: &str = "CreateLogGroup";

/// The refusal this operation models for a name the account holds no delivery destination
/// under.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// Destination resources of three vendors a delivery destination can represent.
const BUCKET_ARN: &str = "arn:aws:s3:::vended-logs-bucket";
const LOG_GROUP_ARN: &str = "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/vendedlogs/one:*";
const FIREHOSE_ARN: &str = "arn:aws:firehose:eu-west-2:123456789012:deliverystream/vended-logs";

/// The `DeliveryDestinationType` of a trace destination, the one kind created from its type
/// alone. Source: the model's `DeliveryDestinationType` enum.
const TRACE: &str = "XRAY";

/// The published maximum length of `DeliveryDestinationName`. Source: the model's
/// `DeliveryDestinationName` shape (`@length` 1..60, `@pattern ^[\w-]*$`), which
/// `GetDeliveryDestinationPolicyRequest$deliveryDestinationName` targets.
const NAME_MAX_LENGTH: usize = 60;

/// One delivery destination in the store: how it is created, and the policy (if any) assigned
/// to it.
#[derive(Debug)]
struct Row {
    /// What this row varies, named in every assertion.
    label: &'static str,
    /// `name` (`DeliveryDestinationName`, 1..60, `^[\w-]*$`).
    name: String,
    /// The `destinationResourceArn` this destination represents, or `None` for the trace
    /// destination, which is created from its type alone.
    resource_arn: Option<&'static str>,
    /// The `deliveryDestinationPolicy` assigned to it, or `None` for a destination none has
    /// been assigned to.
    policy: Option<String>,
}

impl Row {
    /// The `PutDeliveryDestination` request that creates this destination.
    fn put_body(&self) -> Value {
        match self.resource_arn {
            Some(resource_arn) => json!({
                "name": self.name,
                "deliveryDestinationConfiguration": {"destinationResourceArn": resource_arn},
            }),
            None => json!({"name": self.name, "deliveryDestinationType": TRACE}),
        }
    }
}

/// Every delivery destination this account holds in this Region, named by the ARN this
/// service builds for one — the resource a delivery destination policy grants access to, so
/// no account id is written here.
fn delivery_destinations() -> String {
    ::engine::emulator_arn("logs", REGION, "delivery-destination:*")
}

/// The two statements the assignment operation's documentation describes — "One statement
/// enables general logs delivery, and the other allows delivery to the chosen destination" —
/// as one IAM policy document: each grants an account the delivery actions on the delivery
/// resources it names.
fn two_statement_policy(destination_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "AllowLogDeliveryActions",
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::210987654321:root"},
                "Action": ["logs:PutDeliverySource", "logs:CreateDelivery"],
                "Resource": "*",
            },
            {
                "Sid": "AllowDeliveryToDestination",
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::210987654321:root"},
                "Action": "logs:CreateDelivery",
                "Resource": destination_arn,
            },
        ],
    })
    .to_string()
}

/// Destinations spanning the vendors, the trace type, both published bounds of
/// `DeliveryDestinationName`, and policies of several forms beside destinations holding none
/// — all in ONE store, so a report that answers from anything but the destination it names
/// cannot pass.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "an S3 bucket destination holding the two documented statements",
            name: "vended-logs-bucket".to_string(),
            resource_arn: Some(BUCKET_ARN),
            policy: Some(two_statement_policy(&delivery_destinations())),
        },
        Row {
            label: "a log group destination holding no policy",
            name: "vended_logs_group".to_string(),
            resource_arn: Some(LOG_GROUP_ARN),
            policy: None,
        },
        Row {
            label: "a Firehose destination holding two statements with no Sid",
            name: "vended-logs-stream".to_string(),
            resource_arn: Some(FIREHOSE_ARN),
            policy: Some(sid_policy("stream")),
        },
        Row {
            label: "a trace destination holding no policy",
            name: "vended-traces".to_string(),
            resource_arn: None,
            policy: None,
        },
        Row {
            label: "a name at the minimum length, holding a non-ASCII policy",
            name: "d".to_string(),
            resource_arn: Some(BUCKET_ARN),
            policy: Some(sid_policy(
                "lieferung-\u{00fc}ber-gr\u{00f6}\u{00df}e-\u{65e5}\u{672c}",
            )),
        },
        Row {
            label: "a name at the maximum length, holding statements spelled as lists",
            name: "n".repeat(NAME_MAX_LENGTH),
            resource_arn: Some(FIREHOSE_ARN),
            policy: Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": ["*", "arn:aws:iam::210987654321:root"],
                            "Action": ["logs:GetDeliveryDestination", "logs:GetDelivery"],
                            "Resource": [delivery_destinations(), "*"],
                        },
                        {
                            "Effect": "Allow",
                            "Principal": ["*", "arn:aws:iam::210987654321:root"],
                            "Action": ["logs:CreateDelivery", "logs:PutDeliverySource"],
                            "Resource": [delivery_destinations(), "*"],
                        },
                    ],
                })
                .to_string(),
            ),
        },
    ]
}

/// The two statements a delivery destination policy declares, the first carrying `sid`: each
/// grants an account one of the delivery actions on this account's delivery destinations — the
/// elements a delivery destination policy's statements state.
fn sid_policy(sid: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": sid,
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::210987654321:root"},
                "Action": "logs:GetDeliveryDestinationPolicy",
                "Resource": delivery_destinations(),
            },
            {
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::210987654321:root"},
                "Action": "logs:CreateDelivery",
                "Resource": delivery_destinations(),
            },
        ],
    })
    .to_string()
}

/// The `policy.deliveryDestinationPolicy` a response carries, or `None` when the reported
/// `policy` structure carries none — which is what a destination no policy has been assigned
/// to reports. The structure itself is always there, which is asserted here for every read.
fn reported_policy(operation: &str, body: &Value, response: &Value) -> Option<String> {
    let policy = response.get("policy").unwrap_or_else(|| {
        panic!("{operation} {body}: the response carries the policy structure, got {response}")
    });
    assert!(
        policy.is_object(),
        "{operation} {body}: policy is the modelled structure, got {response}"
    );
    Some(
        policy
            .get("deliveryDestinationPolicy")?
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "{operation} {body}: a reported policy carries deliveryDestinationPolicy, \
                     got {response}"
                )
            })
            .to_string(),
    )
}

/// The policy the service reports for `name`, through a wire `GetDeliveryDestinationPolicy`.
async fn read_policy(logs: &Logs, name: &str) -> Option<String> {
    let body = json!({"deliveryDestinationName": name});
    let response = logs.ok(GET_POLICY, &body).await;
    reported_policy(GET_POLICY, &body, &response)
}

/// Assign one policy over the wire.
async fn assign_policy(logs: &Logs, name: &str, document: &str) {
    let body = json!({"deliveryDestinationName": name, "deliveryDestinationPolicy": document});
    logs.ok(PUT_POLICY, &body).await;
}

/// The destination record the service reports for `name` — the OTHER reader of this
/// destination's state, so a read that wrote something shows up here too.
async fn destination_record(logs: &Logs, name: &str) -> Value {
    let body = json!({"name": name});
    logs.ok(GET_DESTINATION, &body)
        .await
        .get("deliveryDestination")
        .cloned()
        .unwrap_or_else(|| panic!("{GET_DESTINATION} {body}: the destination must be reported"))
}

/// Every destination in the store, created and given the policy its row names.
async fn store(logs: &Logs, rows: &[Row]) {
    for row in rows {
        logs.ok(PUT_DESTINATION, &row.put_body()).await;
        if let Some(document) = &row.policy {
            assign_policy(logs, &row.name, document).await;
        }
    }
}

/// Each destination's stored policy is the one reported for it — the assigned contents, or no
/// `policy` member at all where none was assigned — and repeating the reads, interleaved
/// across destinations and with a refused read, changes nothing any later caller sees.
#[tokio::test]
async fn a_read_reports_stored_state_and_writes_nothing() {
    let logs = Logs::new();
    let rows = rows();
    store(&logs, &rows).await;

    // The other reader's view of each destination, before any policy is read.
    let mut before = Vec::new();
    for row in &rows {
        before.push(destination_record(&logs, &row.name).await);
    }

    // Three rounds of reads, every round walking every destination, so each read follows
    // reads of OTHER destinations and reads of the same one.
    for round in 1..=3 {
        for row in &rows {
            let case = (row.label, round);
            let reported = read_policy(&logs, &row.name).await;
            assert_eq!(
                reported,
                row.policy,
                "case {case:?}: expected the stored policy {:?}, got {reported:?}",
                row.policy
            );
        }
        // A read of a name the account holds no destination under, in among them: it is
        // refused, and it is not allowed to leave a destination behind either.
        let code = logs
            .err(GET_POLICY, &json!({"deliveryDestinationName": "never_held"}))
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {round:?}: a name the account does not hold is refused with {NOT_FOUND}, \
             got {code}"
        );
    }

    // Nothing the reads touched moved: the policies still report the same, and so does the
    // other reader of the same destinations.
    for (row, snapshot) in rows.iter().zip(&before) {
        let reported = read_policy(&logs, &row.name).await;
        assert_eq!(
            reported, row.policy,
            "case {:?}: the policy after the reads is the stored one, got {reported:?}",
            row.label
        );
        let after = destination_record(&logs, &row.name).await;
        assert_eq!(
            &after, snapshot,
            "case {:?}: the reads left the destination unchanged, got {after}",
            row.label
        );
    }

    // The refused reads persisted nothing: a destination created under that name afterwards
    // holds no policy.
    logs.ok(PUT_DESTINATION, &json!({"name": "never_held", "deliveryDestinationType": TRACE}))
        .await;
    let reported = read_policy(&logs, "never_held").await;
    assert_eq!(
        reported, None,
        "case \"a destination created after the refused reads\": expected no policy, \
         got {reported:?}"
    );
}

/// The policy lives on a delivery DESTINATION, so a name the account holds some OTHER kind of
/// resource under — a delivery source, a log group, a logical destination — is still a name it
/// holds no delivery destination under, and the read is refused; creating the delivery
/// destination is what makes the read succeed, and it then reports that destination's own
/// policy while the namesakes keep their own state.
#[tokio::test]
async fn a_namesake_of_another_resource_is_not_a_delivery_destination() {
    let logs = Logs::new();
    let name = "shared_name";

    // One namesake per other namespace of this service.
    logs.ok(
        PUT_SOURCE,
        &json!({"name": name, "resourceArn": BUCKET_ARN, "logType": "S3_SERVER_ACCESS_LOGS"}),
    )
    .await;
    logs.ok(CREATE_LOG_GROUP, &json!({"logGroupName": name})).await;
    logs.ok(
        PUT_LOGICAL_DESTINATION,
        &json!({
            "destinationName": name,
            "targetArn": "arn:aws:kinesis:eu-west-2:123456789012:stream/vended",
            "roleArn": "arn:aws:iam::000000000000:role/CWLtoKinesisRole",
        }),
    )
    .await;

    let code = logs
        .err(GET_POLICY, &json!({"deliveryDestinationName": name}))
        .await;
    assert_eq!(
        code, NOT_FOUND,
        "case \"a name held only by other kinds of resource\": expected {NOT_FOUND}, got {code}"
    );

    // The delivery destination of that name, created at last: the read succeeds and reports
    // the policy of THAT resource — none until one is assigned.
    logs.ok(
        PUT_DESTINATION,
        &json!({
            "name": name,
            "deliveryDestinationConfiguration": {"destinationResourceArn": BUCKET_ARN},
        }),
    )
    .await;
    let reported = read_policy(&logs, name).await;
    assert_eq!(
        reported, None,
        "case \"the delivery destination, newly created\": expected no policy, got {reported:?}"
    );

    let document = two_statement_policy(&delivery_destinations());
    assign_policy(&logs, name, &document).await;
    let reported = read_policy(&logs, name).await;
    assert_eq!(
        reported.as_deref(),
        Some(document.as_str()),
        "case \"the delivery destination, policy assigned\": expected the assigned policy, \
         got {reported:?}"
    );

    // The namesake in another namespace kept its own state throughout, and carries no policy
    // of this kind.
    let source = logs
        .ok(GET_SOURCE, &json!({"name": name}))
        .await
        .get("deliverySource")
        .cloned()
        .expect("case \"the delivery source namesake\": the source must be reported");
    assert_eq!(
        source.get("logType").and_then(Value::as_str),
        Some("S3_SERVER_ACCESS_LOGS"),
        "case \"the delivery source namesake\": expected its own logType, got {source}"
    );
    assert!(
        source.get("policy").is_none(),
        "case \"the delivery source namesake\": a delivery source carries no delivery \
         destination policy, got {source}"
    );
}

/// The name is matched EXACTLY: a spelling the account holds no destination under is refused
/// however near it is to one it does hold, and the held spelling keeps reporting its policy.
#[tokio::test]
async fn a_name_is_matched_exactly() {
    let logs = Logs::new();
    let held = "vended-logs-bucket";
    logs.ok(
        PUT_DESTINATION,
        &json!({
            "name": held,
            "deliveryDestinationConfiguration": {"destinationResourceArn": BUCKET_ARN},
        }),
    )
    .await;
    let document = two_statement_policy(&delivery_destinations());
    assign_policy(&logs, held, &document).await;

    // Near misses that all satisfy `DeliveryDestinationName` (1..60, `^[\w-]*$`): case,
    // separator, a missing character, an extra character, and a leading separator.
    let near_misses = [
        ("the held name upper-cased", "VENDED-LOGS-BUCKET".to_string()),
        ("dashes written as underscores", "vended_logs_bucket".to_string()),
        ("one character short", held[..held.len() - 1].to_string()),
        ("one character too many", format!("{held}x")),
        ("a leading separator", format!("-{held}")),
    ];

    for (case, name) in &near_misses {
        let code = logs
            .err(GET_POLICY, &json!({"deliveryDestinationName": name}))
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: a spelling the account does not hold is refused with {NOT_FOUND}, \
             got {code}"
        );
    }

    let reported = read_policy(&logs, held).await;
    assert_eq!(
        reported.as_deref(),
        Some(document.as_str()),
        "case \"the held spelling\": expected the assigned policy, got {reported:?}"
    );
}
