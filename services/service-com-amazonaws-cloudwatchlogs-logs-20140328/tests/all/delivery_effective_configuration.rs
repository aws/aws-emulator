// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the EFFECTIVE configuration of a vended-log delivery: what
//! `CreateDelivery`, `GetDelivery` and `DescribeDeliveries` report for `recordFields`,
//! `fieldDelimiter` and `s3DeliveryConfiguration`, whether a `recordFields` selection is
//! allowed at all, and how the delivery enumerations word a `nextToken` they cannot resume
//! from.
//!
//! The rules under test:
//!
//! * A delivery delivers under ONE whole configuration, so every member of it is resolved when
//!   the delivery is created: a member the request states is the caller's, and a member it
//!   leaves unstated takes the default the configuration template that governs the delivery
//!   publishes — the template for the log type the delivery's SOURCE sends, from the kind of
//!   resource it sends them from, to the kind of destination the delivery pairs it with. The
//!   two members that apply to every delivery are resolved even where that template publishes
//!   no default for them: a delivery that states no delimiter joins its fields with none, and
//!   its OBJECT LAYOUT — the suffix path and the Hive-compatible prefix structure — is the one
//!   its SENDER publishes for delivering those logs to an S3 bucket, whatever kind of
//!   destination it actually pairs with, because a layout is published per log type. So each of
//!   the three is reported by every reader, for every delivery.
//! * A `recordFields` selection is the caller leaving fields OUT of the record its source
//!   sends, so a log source whose every published field is mandatory supports no selection:
//!   `CreateDelivery` and `UpdateDeliveryConfiguration` both refuse one with
//!   `ValidationException`, and a source that publishes a field a delivery may drop takes one.
//! * The delivery enumerations refuse a `nextToken` they cannot resume from with their own
//!   fixed sentence.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a response
//! a handler does not actually compute cannot pass.
//!
//! The tables span what the rules generalise over rather than one request: five log sources of
//! five different services, each paired with a different kind of delivery destination, so the
//! governing template differs per row in its service, its log type, the kind of resource
//! sending the logs and the kind of destination receiving them; the expectation for each row is
//! READ OFF `DescribeConfigurationTemplates` rather than written here, because the rule is that
//! the delivery's defaults are that template's.

use serde_json::{json, Map, Value};

use crate::harness::{Logs, REGION};

/// The operations under test, plus the ones a round trip goes through.
const CREATE: &str = "CreateDelivery";
const GET: &str = "GetDelivery";
const UPDATE: &str = "UpdateDeliveryConfiguration";
const DESCRIBE: &str = "DescribeDeliveries";
const DESCRIBE_SOURCES: &str = "DescribeDeliverySources";
const DESCRIBE_DESTINATIONS: &str = "DescribeDeliveryDestinations";
const TEMPLATES: &str = "DescribeConfigurationTemplates";

/// The modelled error code every refusal below carries.
const VALIDATION: &str = "ValidationException";

/// The whole message a `recordFields` selection a delivery source does not support is refused
/// with: fixed text for that class, naming neither the source nor any field.
const SELECTION_UNSUPPORTED: &str =
    "Record fields selection is not supported by this Delivery Source.";

/// The whole message the delivery enumerations refuse an unusable `nextToken` with: fixed text
/// for that class, carrying neither the token nor the operation.
const INVALID_NEXT_TOKEN: &str = "Invalid Next Token.";

/// One log source paired with one delivery destination: a row of the table below.
#[derive(Debug)]
struct Pairing {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// The `deliverySourceName` the source is created under.
    source: &'static str,
    /// `PutDeliverySource$resourceArn` — "The ARN of the Amazon Web Services resource that is
    /// generating and sending logs".
    resource_arn: String,
    /// `PutDeliverySource$logType`, one the resource's service sends.
    log_type: &'static str,
    /// The `ConfigurationTemplate$service` and `$resourceType` the governing template is keyed
    /// by: the ARN namespace of the sending resource, and the kind of resource it is.
    service: &'static str,
    resource_type: &'static str,
    /// The `deliveryDestinationName` the destination is created under, and the resource it
    /// represents — a LOG GROUP of this account for the CloudWatch Logs row, which is created
    /// first.
    destination: &'static str,
    destination_resource: String,
    /// The `DeliveryDestinationType` that destination resource implies, which is the fourth
    /// value the governing template is keyed by.
    destination_type: &'static str,
}

/// Five pairings, each with a different sending service, log type, resource kind and kind of
/// destination. Each resource ARN is of the form its own service documents, and each log type
/// is one that service sends (the `logType` member's own catalogue,
/// `smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`).
fn pairings() -> Vec<Pairing> {
    vec![
        Pairing {
            label: "Security Hub findings from a hubv2 resource to an S3 bucket",
            source: "securityhub-findings-v2",
            resource_arn: format!("arn:aws:securityhub:{REGION}:123456789012:hubv2/*"),
            log_type: "SECURITY_FINDING_LOGS",
            service: "securityhub",
            resource_type: "hubv2",
            destination: "findings-bucket",
            destination_resource: "arn:aws:s3:::vended-logs-findings".to_string(),
            destination_type: "S3",
        },
        Pairing {
            label: "Security Hub findings from a hub resource to a log group",
            source: "securityhub-findings",
            resource_arn: format!("arn:aws:securityhub:{REGION}:123456789012:hub/*"),
            log_type: "SECURITY_FINDING_LOGS",
            service: "securityhub",
            resource_type: "hub",
            destination: "findings-group",
            destination_resource: String::new(),
            destination_type: "CWL",
        },
        Pairing {
            label: "Insights query logs from a log group to a Firehose delivery stream",
            source: "insights-queries",
            resource_arn: String::new(),
            log_type: "INSIGHTS_QUERY_LOGS",
            service: "logs",
            resource_type: "log-group",
            destination: "queries-stream",
            destination_resource: format!(
                "arn:aws:firehose:{REGION}:123456789012:deliverystream/vended-logs"
            ),
            destination_type: "FH",
        },
        Pairing {
            label: "ALB access logs from a load balancer to an S3 bucket",
            source: "alb-access-logs",
            resource_arn: format!(
                "arn:aws:elasticloadbalancing:{REGION}:123456789012:loadbalancer/app/vended/50dc"
            ),
            log_type: "ALB_ACCESS_LOGS",
            service: "elasticloadbalancing",
            resource_type: "loadbalancer",
            destination: "alb-bucket",
            destination_resource: "arn:aws:s3:::vended-logs-alb".to_string(),
            destination_type: "S3",
        },
        Pairing {
            label: "API Gateway execution logs from a stage to a log group",
            source: "apigateway-execution",
            resource_arn: format!("arn:aws:apigateway:{REGION}::/restapis/a1b2c3/stages/prod"),
            log_type: "EXECUTION_LOGS",
            service: "apigateway",
            resource_type: "stages",
            destination: "execution-group",
            destination_resource: String::new(),
            destination_type: "CWL",
        },
    ]
}

/// Create the source and the destination of one pairing, answering the destination's ARN. A
/// CloudWatch Logs destination represents a LOG GROUP, so the group is created first and the
/// source of the log-group row sends ITS logs.
async fn provision(logs: &Logs, pairing: &Pairing) -> String {
    let source_resource = if pairing.resource_arn.is_empty() {
        let group = format!("/vended/{}", pairing.source);
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
        let (_, group_arn) = logs.group_arns(&group).await;
        group_arn
    } else {
        pairing.resource_arn.clone()
    };
    logs.ok(
        "PutDeliverySource",
        &json!({
            "name": pairing.source,
            "resourceArn": source_resource,
            "logType": pairing.log_type,
        }),
    )
    .await;

    let destination_resource = if pairing.destination_resource.is_empty() {
        let group = format!("/vended/destination/{}", pairing.destination);
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
        let (_, group_arn) = logs.group_arns(&group).await;
        group_arn
    } else {
        pairing.destination_resource.clone()
    };
    let answer = logs
        .ok(
            "PutDeliveryDestination",
            &json!({
                "name": pairing.destination,
                "deliveryDestinationConfiguration": {
                    "destinationResourceArn": destination_resource,
                },
            }),
        )
        .await;
    assert_eq!(
        answer["deliveryDestination"]["deliveryDestinationType"].as_str(),
        Some(pairing.destination_type),
        "case {:?}: the destination resource implies the {} destination type, got {answer}",
        pairing.label,
        pairing.destination_type
    );
    answer["deliveryDestination"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a put answers the destination's ARN, got {answer}"))
        .to_string()
}

/// The ONE configuration template the catalogue publishes for a pairing, as
/// `DescribeConfigurationTemplates` reports it — the expectation every row below is read off,
/// because the rule is that a delivery's unstated configuration is this template's default.
async fn governing_template(logs: &Logs, pairing: &Pairing) -> Value {
    let answer = logs
        .ok(
            TEMPLATES,
            &json!({
                "service": pairing.service,
                "logTypes": [pairing.log_type],
                "resourceTypes": [pairing.resource_type],
                "deliveryDestinationTypes": [pairing.destination_type],
            }),
        )
        .await;
    let published = answer["configurationTemplates"]
        .as_array()
        .unwrap_or_else(|| panic!("case {:?}: a templates page, got {answer}", pairing.label))
        .clone();
    assert_eq!(
        published.len(),
        1,
        "case {:?}: the catalogue publishes one template per service, log type, resource type \
         and destination type, got {answer}",
        pairing.label
    );
    published[0].clone()
}

/// The `DeliveryDestinationType` an OBJECT LAYOUT is published for: the one a delivery writes
/// objects under, which is the destination type whose template carries an
/// `s3DeliveryConfiguration`.
const OBJECT_DESTINATION_TYPE: &str = "S3";

/// The object layout the SENDER of a pairing publishes for its logs: the
/// `defaultDeliveryConfigValues.s3DeliveryConfiguration` of its template for delivering them to
/// an S3 BUCKET.
///
/// A layout is published per LOG TYPE — `S3DeliveryConfiguration$suffixPath`'s own variables
/// "vary by log type" — so it is the layout EVERY delivery of those logs reports, whatever kind
/// of destination it pairs with, and it is read off this template rather than off the one that
/// governs the delivery.
async fn published_object_layout(logs: &Logs, pairing: &Pairing) -> Option<Map<String, Value>> {
    let answer = logs
        .ok(
            TEMPLATES,
            &json!({
                "service": pairing.service,
                "logTypes": [pairing.log_type],
                "resourceTypes": [pairing.resource_type],
                "deliveryDestinationTypes": [OBJECT_DESTINATION_TYPE],
            }),
        )
        .await;
    answer["configurationTemplates"][0]
        .get("defaultDeliveryConfigValues")
        .and_then(|defaults| defaults.get("s3DeliveryConfiguration"))
        .and_then(Value::as_object)
        .cloned()
}

/// The `delivery` member of a response that carries one.
fn delivery(answer: &Value, case: &str) -> Value {
    answer
        .get("delivery")
        .filter(|delivery| !delivery.is_null())
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: expected a delivery, got {answer}"))
}

/// The delivery this account holds under `id`, as `GetDelivery` reports it.
async fn fetched(logs: &Logs, id: &str, case: &str) -> Value {
    delivery(&logs.ok(GET, &json!({"id": id})).await, case)
}

/// A create pairing one source with one destination, answering the delivery it reports.
async fn create(logs: &Logs, source: &str, destination_arn: &str, stated: &Value) -> Value {
    let mut body = json!({
        "deliverySourceName": source,
        "deliveryDestinationArn": destination_arn,
    });
    if let Some(members) = stated.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    delivery(&logs.ok(CREATE, &body).await, source)
}

/// The `defaultDeliveryConfigValues` of a template, as a map.
fn defaults(template: &Value, case: &str) -> Map<String, Value> {
    template
        .get("defaultDeliveryConfigValues")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case}: a template publishes its default values, got {template}")
        })
}

/// A delivery created with no configuration member at all delivers under the WHOLE
/// configuration the governing template defaults it to, and every reader reports it.
#[tokio::test]
async fn an_unconfigured_delivery_reports_the_templates_defaults() {
    let logs = Logs::new();
    for pairing in pairings() {
        let case = pairing.label;
        let destination_arn = provision(&logs, &pairing).await;
        let template = governing_template(&logs, &pairing).await;
        let published = defaults(&template, case);

        let created = create(&logs, pairing.source, &destination_arn, &json!({})).await;

        assert_eq!(
            created.get("recordFields"),
            published.get("recordFields"),
            "case {case}: the delivery's record fields are the template's default, got {created}"
        );
        // The delimiter is the template's default where it publishes one, and NO delimiter
        // where it publishes none — a delivery joins its record fields with something either
        // way, so the member is reported for every delivery.
        assert_eq!(
            created.get("fieldDelimiter").and_then(Value::as_str),
            Some(
                published
                    .get("fieldDelimiter")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ),
            "case {case}: the delivery's field delimiter is the template's default, and none \
             where it publishes none, got {created}"
        );
        let stored_layout = created
            .get("s3DeliveryConfiguration")
            .and_then(Value::as_object)
            .unwrap_or_else(|| {
                panic!("case {case}: the delivery reports an object layout, got {created}")
            });
        let published_layout = published_object_layout(&logs, &pairing).await;
        assert_eq!(
            stored_layout.get("suffixPath").and_then(Value::as_str),
            Some(
                published_layout
                    .as_ref()
                    .and_then(|layout| layout.get("suffixPath"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ),
            "case {case}: the delivery's suffix path is the one its SENDER publishes, whatever \
             kind of destination it is delivered to, and no prefix where the sender publishes \
             none, got {created}"
        );
        assert_eq!(
            stored_layout
                .get("enableHiveCompatiblePath")
                .and_then(Value::as_bool),
            Some(
                published_layout
                    .as_ref()
                    .and_then(|layout| layout.get("enableHiveCompatiblePath"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            ),
            "case {case}: the delivery's Hive-compatible prefix setting is the sender's \
             published default, and off where it publishes none, got {created}"
        );

        // The round trip: both readers report the delivery the create reported.
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a created delivery carries an id"))
            .to_string();
        assert_eq!(
            fetched(&logs, &id, case).await,
            created,
            "case {case}: {GET} reports the delivery the create reported"
        );
        let page = logs.ok(DESCRIBE, &json!({})).await;
        assert!(
            page["deliveries"]
                .as_array()
                .is_some_and(|deliveries| deliveries.contains(&created)),
            "case {case}: {DESCRIBE} reports the delivery the create reported, got {page}"
        );
    }
}

/// A member the create STATES is the caller's, and the ones it leaves unstated are still
/// resolved: an object layout restated in part keeps its other member at the default.
#[tokio::test]
async fn a_stated_configuration_member_is_the_callers_and_the_rest_is_resolved() {
    let logs = Logs::new();
    let pairing = Pairing {
        label: "Insights query logs to an S3 bucket",
        source: "stated-configuration",
        resource_arn: String::new(),
        log_type: "INSIGHTS_QUERY_LOGS",
        service: "logs",
        resource_type: "log-group",
        destination: "stated-bucket",
        destination_resource: "arn:aws:s3:::vended-logs-stated".to_string(),
        destination_type: "S3",
    };
    let destination_arn = provision(&logs, &pairing).await;
    let template = governing_template(&logs, &pairing).await;
    let published = defaults(&template, pairing.label);
    let published_suffix_path = published
        .get("s3DeliveryConfiguration")
        .and_then(|layout| layout.get("suffixPath"))
        .cloned()
        .unwrap_or_else(|| json!(""));

    // (case, the members the create states, what the delivery must carry afterwards)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "a delimiter only",
            json!({"fieldDelimiter": "|||||"}),
            json!({
                "fieldDelimiter": "|||||",
                "recordFields": published.get("recordFields").cloned().unwrap_or(Value::Null),
            }),
        ),
        (
            "the empty record-field list only",
            // `RecordFields` is `@length(min: 0)`: the empty list is a list the caller named,
            // which is not the same as naming none.
            json!({"recordFields": []}),
            json!({
                "recordFields": [],
                "fieldDelimiter": published.get("fieldDelimiter").cloned().unwrap_or(Value::Null),
            }),
        ),
        (
            "an object layout restated in part",
            json!({"s3DeliveryConfiguration": {"enableHiveCompatiblePath": true}}),
            json!({
                "s3DeliveryConfiguration": {
                    "enableHiveCompatiblePath": true,
                    "suffixPath": published_suffix_path,
                },
            }),
        ),
    ];

    for (case, stated, expected) in cases {
        // One destination per case, so each case is its own delivery of the same pair.
        let destination = format!("stated-{}", case.replace(' ', "-"));
        let answer = logs
            .ok(
                "PutDeliveryDestination",
                &json!({
                    "name": destination,
                    "deliveryDestinationConfiguration": {
                        "destinationResourceArn": pairing.destination_resource,
                    },
                }),
            )
            .await;
        let arn = answer["deliveryDestination"]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a put answers the destination's ARN"))
            .to_string();
        assert_ne!(
            arn, destination_arn,
            "case {case}: each case is a delivery of its own pair"
        );

        let created = create(&logs, pairing.source, &arn, &stated).await;
        for (member, value) in expected.as_object().expect("the expectation is a map") {
            assert_eq!(
                created.get(member),
                Some(value),
                "case {case}: expected {member} {value}, got {created}"
            );
        }
        let id = created["id"].as_str().expect("a created delivery has an id");
        assert_eq!(
            fetched(&logs, id, case).await,
            created,
            "case {case}: {GET} reports the delivery the create reported"
        );
    }
}

/// A `recordFields` selection is a caller leaving fields OUT, so it is refused exactly for the
/// log sources whose every published field is mandatory — by the create and by the update
/// alike — and taken for the ones that publish a field a delivery may drop.
#[tokio::test]
async fn a_selection_is_refused_exactly_where_no_field_may_be_dropped() {
    let logs = Logs::new();
    for pairing in pairings() {
        let case = pairing.label;
        let destination_arn = provision(&logs, &pairing).await;
        let template = governing_template(&logs, &pairing).await;
        let allowed = template["allowedFields"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: a template publishes its allowed fields"))
            .clone();
        assert!(
            !allowed.is_empty(),
            "case {case}: a template publishes at least one field, got {template}"
        );
        // Whether the SOURCE supports a selection at all: a field a delivery may drop is one
        // the template does not mark mandatory.
        let droppable = allowed
            .iter()
            .any(|field| field["mandatory"].as_bool() != Some(true));
        let selection: Vec<&str> = allowed
            .iter()
            .filter(|field| field["mandatory"].as_bool() == Some(true))
            .filter_map(|field| field["name"].as_str())
            .collect();

        // The create: a selection is refused before the delivery is minted, or stored as the
        // caller gave it.
        let body = json!({
            "deliverySourceName": pairing.source,
            "deliveryDestinationArn": destination_arn,
            "recordFields": selection,
        });
        if droppable {
            let created = delivery(&logs.ok(CREATE, &body).await, case);
            assert_eq!(
                created["recordFields"],
                json!(selection),
                "case {case}: a source that publishes a droppable field takes the selection, \
                 got {created}"
            );
            // And the update takes one too, with the delivery reporting it afterwards.
            let id = created["id"].as_str().expect("a created delivery has an id");
            logs.ok(UPDATE, &json!({"id": id, "recordFields": selection}))
                .await;
            let after = fetched(&logs, id, case).await;
            assert_eq!(
                after["recordFields"],
                json!(selection),
                "case {case}: the update narrowed the delivered fields, got {after}"
            );
            continue;
        }

        let (code, said) = logs.refusal(CREATE, &body).await;
        assert_eq!(
            code, VALIDATION,
            "case {case}: a selection no field may be dropped from is refused with \
             {VALIDATION}, got {code} {said:?}"
        );
        assert_eq!(
            said, SELECTION_UNSUPPORTED,
            "case {case}: expected {SELECTION_UNSUPPORTED:?}, got {said:?}"
        );

        // The update: the delivery is created WITHOUT a selection, which is accepted, and the
        // update that would narrow it is refused and leaves the delivery as it was.
        let created = create(&logs, pairing.source, &destination_arn, &json!({})).await;
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a created delivery carries an id"))
            .to_string();
        // Every selection is refused, however it narrows the record: the full mandatory list,
        // a shorter one, and the empty list.
        let narrower: Vec<&str> = selection.iter().copied().take(1).collect();
        for (form, fields) in [
            ("the whole published record", json!(selection)),
            ("one field of it", json!(narrower)),
            ("the empty list", json!([])),
        ] {
            let (code, said) = logs
                .refusal(UPDATE, &json!({"id": &id, "recordFields": fields}))
                .await;
            assert_eq!(
                code, VALIDATION,
                "case {case}, {form}: expected {VALIDATION}, got {code} {said:?}"
            );
            assert_eq!(
                said, SELECTION_UNSUPPORTED,
                "case {case}, {form}: expected {SELECTION_UNSUPPORTED:?}, got {said:?}"
            );
            assert_eq!(
                fetched(&logs, &id, case).await,
                created,
                "case {case}, {form}: a refused update changes nothing"
            );
        }

        // The members an update MAY change on such a delivery are still changed by one that
        // names no record fields.
        logs.ok(UPDATE, &json!({"id": &id, "fieldDelimiter": ","}))
            .await;
        let after = fetched(&logs, &id, case).await;
        assert_eq!(
            after["fieldDelimiter"].as_str(),
            Some(","),
            "case {case}: an update that names no selection still changes the delimiter, got \
             {after}"
        );
    }
}

/// The delivery enumerations refuse a `nextToken` they cannot resume from with their own fixed
/// sentence, whichever way the token fails to place.
#[tokio::test]
async fn an_unusable_next_token_is_refused_with_the_delivery_wording() {
    let logs = Logs::new();
    let pairing = &pairings()[0];
    let destination_arn = provision(&logs, pairing).await;
    create(&logs, pairing.source, &destination_arn, &json!({})).await;

    // Tokens no enumeration of this service can resume from: text that is not a token at all,
    // one at each end of the size range, and an odd-length hex string.
    let unusable = [
        ("text that is not a token", "not-a-token".to_string()),
        ("a one-character token", "z".to_string()),
        ("an odd-length hex string", "abc".to_string()),
        ("a long run of hex digits", "ab".repeat(64)),
    ];
    // Each delivery enumeration, with the request it pages.
    let enumerations = [
        (DESCRIBE, json!({})),
        (DESCRIBE_SOURCES, json!({})),
        (DESCRIBE_DESTINATIONS, json!({})),
    ];

    for (operation, body) in &enumerations {
        for (form, token) in &unusable {
            let case = format!("{operation}: {form}");
            let mut request = body.clone();
            request["nextToken"] = json!(token);
            let (code, said) = logs.refusal(operation, &request).await;
            assert_eq!(
                code, VALIDATION,
                "case {case}: an unusable token is refused with {VALIDATION}, got {code} \
                 {said:?}"
            );
            assert_eq!(
                said, INVALID_NEXT_TOKEN,
                "case {case}: expected {INVALID_NEXT_TOKEN:?}, got {said:?}"
            );
        }
    }

    // A token this service DID mint, handed back for a different enumeration than the one it
    // was minted for: a page of all deliveries continued against a filtered enumeration.
    let second = Pairing {
        label: "a second pair, so one page leaves another delivery to come",
        source: "second-source",
        resource_arn: format!("arn:aws:securityhub:{REGION}:123456789012:hub/*"),
        log_type: "SECURITY_FINDING_LOGS",
        service: "securityhub",
        resource_type: "hub",
        destination: "second-bucket",
        destination_resource: "arn:aws:s3:::vended-logs-second".to_string(),
        destination_type: "S3",
    };
    let second_arn = provision(&logs, &second).await;
    create(&logs, second.source, &second_arn, &json!({})).await;

    let page = logs.ok(DESCRIBE, &json!({"limit": 1})).await;
    let token = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a page with more to come hands back a token, got {page}"))
        .to_string();
    // The same token resumes the enumeration it was minted for.
    logs.ok(DESCRIBE, &json!({"limit": 1, "nextToken": &token}))
        .await;
    let (code, said) = logs
        .refusal(
            DESCRIBE,
            &json!({
                "limit": 1,
                "nextToken": &token,
                "deliverySourceNames": [pairing.source],
            }),
        )
        .await;
    assert_eq!(
        code, VALIDATION,
        "case a token minted for another enumeration: expected {VALIDATION}, got {code} {said:?}"
    );
    assert_eq!(
        said, INVALID_NEXT_TOKEN,
        "case a token minted for another enumeration: expected {INVALID_NEXT_TOKEN:?}, got \
         {said:?}"
    );
}
