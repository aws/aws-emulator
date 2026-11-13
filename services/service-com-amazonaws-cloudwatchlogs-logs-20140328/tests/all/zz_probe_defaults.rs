// SPDX-License-Identifier: CC0-1.0

//! Probes of the ROWS and the published DEFAULTS of the vended-log configuration-template
//! catalogue, from the client's side: which (service, resource type, log type) rows a caller can
//! find, in which order they are published, and that a delivery source can be created for each
//! log type a row publishes.
//!
//! The rules under test:
//!
//! * The catalogue holds a row per kind of resource a service sends logs from, for every service
//!   the `logType` member's own catalogue names
//!   (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`), and each
//!   row publishes the log types that kind of resource sends — the same catalogue that decides
//!   which `logType` a `PutDeliverySource` over such a resource may name, so a row a client can
//!   find is a delivery it can create.
//! * Every template a row publishes names the sender's own vended-logs delivery action and the
//!   record fields a delivery of those logs carries when it names none.
//! * The templates are published in ONE order for every caller, and for a service that sends one
//!   log type from two kinds of resource the newer kind comes first, so a client that takes the
//!   first template of the slice binds its delivery to the resource the service vends from
//!   today.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire.
//!
//! The tables span what the rules generalise over rather than one slice: services that send one,
//! two and three log types from one kind of resource, and services that send one log type from
//! two kinds of resource.

use serde_json::{json, Map, Value};

use crate::harness::{Logs, REGION};

/// The operations under test.
const DESCRIBE: &str = "DescribeConfigurationTemplates";
const PUT_SOURCE: &str = "PutDeliverySource";

/// The largest page the model admits (`DescribeLimit` is `range 1..50`), so a filtered
/// enumeration below is read in a single call.
const MAX_LIMIT: usize = 50;

/// The templates one slice of the catalogue publishes, in the order it publishes them.
async fn templates(logs: &Logs, filter: &Value) -> Vec<Value> {
    let answer = logs.ok(DESCRIBE, filter).await;
    answer["configurationTemplates"]
        .as_array()
        .unwrap_or_else(|| panic!("case {filter}: a templates page, got {answer}"))
        .clone()
}

/// A template's `defaultDeliveryConfigValues`.
fn defaults(template: &Value, case: &str) -> Map<String, Value> {
    template
        .get("defaultDeliveryConfigValues")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case}: a template publishes its default values, got {template}")
        })
}

/// Every service the `logType` catalogue names has its row, and a delivery source may be created
/// for each log type that row publishes.
#[tokio::test]
async fn the_catalogue_publishes_a_row_for_every_service_that_sends_vended_logs() {
    let logs = Logs::new();

    // (service, resource type, the log types that kind of resource sends, a resource ARN of that
    // kind). Source: the `logType` member of `PutDeliverySourceRequest`
    // (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`) — "For
    // Amazon Q, the valid values are EVENT_LOGS and SYNC_JOB_LOGS", "For Amazon Bedrock AgentCore
    // Tools, the valid values are APPLICATION_LOGS, USAGE_LOGS, and TRACES", "For Amazon SES mail
    // manager, the valid values are APPLICATION_LOGS and TRAFFIC_POLICY_DEBUG_LOGS", "For Amazon
    // Web Services Security Hub, the valid value is SECURITY_FINDING_LOGS". Each ARN is of the
    // form its own service documents, including the wildcard ARN that member documents for the
    // log types a hub resource sends account-wide.
    let rows: [(&str, &str, &[&str], String); 4] = [
        (
            "qbusiness",
            "application",
            &["EVENT_LOGS", "SYNC_JOB_LOGS"],
            format!(
                "arn:aws:qbusiness:{REGION}:123456789012:application/\
                 0123abcd-ef45-6789-abcd-ef0123456789"
            ),
        ),
        (
            "bedrock-agentcore",
            "tools",
            &["APPLICATION_LOGS", "USAGE_LOGS", "TRACES"],
            format!("arn:aws:bedrock-agentcore:{REGION}:123456789012:tools/my_tool-abc123"),
        ),
        (
            "ses",
            "mailmanager",
            &["APPLICATION_LOGS", "TRAFFIC_POLICY_DEBUG_LOGS"],
            format!(
                "arn:aws:ses:{REGION}:123456789012:mailmanager-ingress-point/ip-0123456789abcdef"
            ),
        ),
        (
            "securityhub",
            "hub",
            &["SECURITY_FINDING_LOGS"],
            format!("arn:aws:securityhub:{REGION}:123456789012:hub/*"),
        ),
    ];

    for (service, resource_type, log_types, resource_arn) in rows {
        for log_type in log_types {
            let case = format!("{service}|{log_type}|{resource_type}");
            let published = templates(
                &logs,
                &json!({
                    "service": service,
                    "logTypes": [log_type],
                    "resourceTypes": [resource_type],
                    "limit": MAX_LIMIT,
                }),
            )
            .await;
            assert!(
                !published.is_empty(),
                "case {case}: the catalogue publishes templates for this log source, got none"
            );
            let action = format!("{service}:AllowVendedLogDeliveryForResource");
            for template in &published {
                assert_eq!(
                    template["allowedActionForAllowVendedLogsDeliveryForResource"].as_str(),
                    Some(action.as_str()),
                    "case {case}: expected the delivery action {action:?}, got {template}"
                );
                assert!(
                    defaults(template, &case).contains_key("recordFields"),
                    "case {case}: a template publishes the record fields a delivery of these \
                     logs carries by default, got {template}"
                );
            }

            // The row is what makes this log type one such a resource may be a source for.
            let name = format!("{service}-{resource_type}-{log_type}").to_lowercase();
            let answer = logs
                .ok(
                    PUT_SOURCE,
                    &json!({"name": name, "resourceArn": resource_arn, "logType": log_type}),
                )
                .await;
            assert_eq!(
                answer["deliverySource"]["service"].as_str(),
                Some(service),
                "case {case}: the source reports the service that sends its logs, got {answer}"
            );
        }
    }
}

/// One service's templates are published in one order, the newer kind of resource first.
#[tokio::test]
async fn a_services_templates_are_published_in_one_order() {
    let logs = Logs::new();

    // (service, log type, the kinds of resource it sends those logs from, in published order):
    // the service whose two hub resources send the SAME log type — the newer one first — and one
    // whose two kinds of resource are published in the order the catalogue states them.
    let cases: [(&str, &str, &[&str]); 2] = [
        ("securityhub", "SECURITY_FINDING_LOGS", &["hubv2", "hub"]),
        ("events", "ERROR_LOGS", &["event-bus", "subscriber"]),
    ];

    for (service, log_type, expected) in cases {
        let case = format!("{service}|{log_type}");
        let published = templates(
            &logs,
            &json!({"service": service, "logTypes": [log_type], "limit": MAX_LIMIT}),
        )
        .await;
        let mut reported: Vec<String> = Vec::new();
        for template in &published {
            let resource_type = template["resourceType"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a template names its resource type"))
                .to_string();
            if !reported.contains(&resource_type) {
                reported.push(resource_type);
            }
        }
        let expected: Vec<String> = expected.iter().map(|kind| (*kind).to_string()).collect();
        assert_eq!(
            reported, expected,
            "case {case}: expected the resource types published in the order {expected:?}, got \
             {reported:?}"
        );

        // The same order on a second call: the catalogue is service-managed, so reading it does
        // not reorder it.
        let again = templates(
            &logs,
            &json!({"service": service, "logTypes": [log_type], "limit": MAX_LIMIT}),
        )
        .await;
        assert_eq!(
            again, published,
            "case {case}: two calls report the same templates in the same order"
        );
    }
}
