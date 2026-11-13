// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DEFAULTS a vended-log delivery takes from the configuration-template
//! catalogue: the record fields it delivers and the object layout it writes under.
//!
//! The rules under test:
//!
//! * A delivery's `recordFields` are the ones the template that governs it publishes as its
//!   default — "the default record fields that will be delivered when a list of record fields is
//!   not provided in a CreateDelivery" — so a create that names none reports the whole published
//!   list, in the published order, and every reader reports the same.
//! * A delivery's OBJECT LAYOUT — `s3DeliveryConfiguration`'s suffix path and its
//!   Hive-compatible prefix structure — is the one its SENDER publishes. A layout is published
//!   per LOG TYPE (`S3DeliveryConfiguration$suffixPath`: "the valid variables to use in the
//!   suffix path vary by log type", found through `DescribeConfigurationTemplates`), so it is a
//!   fact about the logs being delivered rather than about the kind of destination a delivery
//!   happens to pair them with: a delivery to a log group or to a delivery stream reports the
//!   same layout as one to a bucket, instead of losing it.
//! * A member the create STATES is the caller's, and it displaces only itself: the rest of the
//!   layout is still the published default.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a value a
//! handler does not actually compute from the store cannot pass.
//!
//! The tables span what the rules generalise over rather than one request: senders whose
//! published layout partitions delivered objects by Region and hour, by the sending resource, by
//! the day, and not at all; record schemas of 1, 7, 21 and 34 published fields, with and without
//! fields that are allowed without being delivered; and each of them delivered to a destination
//! that writes objects, to one that stores records and to one that receives a stream — the
//! expectation for every row READ OFF `DescribeConfigurationTemplates`, because the rule is that
//! a delivery's defaults are what the catalogue publishes for its logs.

use serde_json::{json, Map, Value};

use crate::harness::{Logs, REGION};

/// The operations under test, plus the ones a round trip goes through.
const CREATE: &str = "CreateDelivery";
const GET: &str = "GetDelivery";
const TEMPLATES: &str = "DescribeConfigurationTemplates";
const PUT_SOURCE: &str = "PutDeliverySource";
const PUT_DESTINATION: &str = "PutDeliveryDestination";

/// The `DeliveryDestinationType` an object layout is published for — the one kind of destination
/// a delivery writes OBJECTS to — as the model's enum spells it
/// (`smithy_query(query="shape",shape="DeliveryDestinationType")`).
const OBJECT_DESTINATION_TYPE: &str = "S3";

/// One sender: a kind of resource, the log type it sends, and the ARN a delivery source over it
/// carries. Each ARN is of the form its own service documents, and each log type is one that
/// service sends (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`).
#[derive(Debug)]
struct Sender {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// The `deliverySourceName` the source is created under.
    source: &'static str,
    /// `PutDeliverySource$resourceArn` — "The ARN of the Amazon Web Services resource that is
    /// generating and sending logs".
    resource_arn: String,
    /// `PutDeliverySource$logType`.
    log_type: &'static str,
    /// The `ConfigurationTemplate$service` and `$resourceType` the sender's templates are keyed
    /// by: the ARN namespace of the sending resource, and the kind of resource it is.
    service: &'static str,
    resource_type: &'static str,
}

/// Four senders whose published object layouts differ in kind, with record schemas of four
/// different shapes and sizes.
fn senders() -> Vec<Sender> {
    vec![
        // An AgentCore runtime: a 21-field record — the vended-log envelope plus the
        // OpenTelemetry log record — two of whose fields are allowed without being delivered.
        Sender {
            label: "AgentCore runtime application logs",
            source: "agentcore-runtime-logs",
            resource_arn: format!(
                "arn:aws:bedrock-agentcore:{REGION}:123456789012:runtime/my_runtime-abc123"
            ),
            log_type: "APPLICATION_LOGS",
            service: "bedrock-agentcore",
            resource_type: "runtime",
        },
        // An API Gateway stage: a nine-field envelope, and a published layout that partitions by
        // the sending resource rather than by time.
        Sender {
            label: "API Gateway stage execution logs",
            source: "apigateway-stage-logs",
            resource_arn: format!("arn:aws:apigateway:{REGION}::/restapis/a1b2c3/stages/prod"),
            log_type: "EXECUTION_LOGS",
            service: "apigateway",
            resource_type: "stages",
        },
        // A load balancer: a 34-field access-log line, every field mandatory, and a layout that
        // partitions by the day.
        Sender {
            label: "Application Load Balancer access logs",
            source: "alb-access-logs",
            resource_arn: format!(
                "arn:aws:elasticloadbalancing:{REGION}:123456789012:loadbalancer/app/vended/50dc"
            ),
            log_type: "ALB_ACCESS_LOGS",
            service: "elasticloadbalancing",
            resource_type: "loadbalancer",
        },
        // Security Hub findings: a single mandatory field, and a sender that publishes no object
        // prefix of its own at all.
        Sender {
            label: "Security Hub findings from a hubv2 resource",
            source: "securityhub-findings-v2",
            resource_arn: format!("arn:aws:securityhub:{REGION}:123456789012:hubv2/*"),
            log_type: "SECURITY_FINDING_LOGS",
            service: "securityhub",
            resource_type: "hubv2",
        },
    ]
}

/// One kind of delivery destination a sender's logs are delivered to: what the destination
/// represents, and the `DeliveryDestinationType` that resource implies.
struct Destination {
    label: &'static str,
    destination_type: &'static str,
}

/// A destination that writes objects, one that stores records and one that receives a stream —
/// so each sender's layout is read back from a delivery to each kind of destination there is a
/// resource for.
const DESTINATIONS: [Destination; 3] = [
    Destination {
        label: "an S3 bucket",
        destination_type: OBJECT_DESTINATION_TYPE,
    },
    Destination {
        label: "a log group",
        destination_type: "CWL",
    },
    Destination {
        label: "a Firehose delivery stream",
        destination_type: "FH",
    },
];

/// The templates one slice of the catalogue publishes, as `DescribeConfigurationTemplates`
/// reports them.
async fn templates(logs: &Logs, filter: &Value) -> Vec<Value> {
    let answer = logs.ok(TEMPLATES, filter).await;
    answer["configurationTemplates"]
        .as_array()
        .unwrap_or_else(|| panic!("case {filter}: a templates page, got {answer}"))
        .clone()
}

/// The ONE template the catalogue publishes for a sender's logs delivered to
/// `destination_type`.
async fn template(logs: &Logs, sender: &Sender, destination_type: &str) -> Value {
    let filter = json!({
        "service": sender.service,
        "logTypes": [sender.log_type],
        "resourceTypes": [sender.resource_type],
        "deliveryDestinationTypes": [destination_type],
    });
    let published = templates(logs, &filter).await;
    assert_eq!(
        published.len(),
        1,
        "case {:?}: the catalogue publishes one template per service, log type, resource type \
         and destination type, got {published:?}",
        sender.label
    );
    published[0].clone()
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

/// The object layout a sender publishes for its logs: the `s3DeliveryConfiguration` of its
/// template for delivering them to an S3 BUCKET, which is the one kind of destination the
/// catalogue publishes a layout for.
async fn published_object_layout(logs: &Logs, sender: &Sender) -> Map<String, Value> {
    let template = template(logs, sender, OBJECT_DESTINATION_TYPE).await;
    defaults(&template, sender.label)
        .get("s3DeliveryConfiguration")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the template for a bucket publishes an object layout, got {template}",
                sender.label
            )
        })
}

/// The delivery source of a sender, created.
async fn provision_source(logs: &Logs, sender: &Sender) {
    logs.ok(
        PUT_SOURCE,
        &json!({
            "name": sender.source,
            "resourceArn": sender.resource_arn,
            "logType": sender.log_type,
        }),
    )
    .await;
}

/// One delivery destination of each kind, created under `name`, answering its ARN. The resource
/// a CloudWatch Logs destination represents is a LOG GROUP of this account, so it is created
/// first.
async fn provision_destination(logs: &Logs, name: &str, destination: &Destination) -> String {
    let resource = match destination.destination_type {
        "CWL" => {
            let group = format!("/vended/destination/{name}");
            logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
                .await;
            let (_arn, group_arn) = logs.group_arns(&group).await;
            group_arn
        }
        "FH" => format!("arn:aws:firehose:{REGION}:123456789012:deliverystream/{name}"),
        _ => format!("arn:aws:s3:::vended-logs-{name}"),
    };
    let answer = logs
        .ok(
            PUT_DESTINATION,
            &json!({
                "name": name,
                "deliveryDestinationConfiguration": {"destinationResourceArn": resource},
            }),
        )
        .await;
    assert_eq!(
        answer["deliveryDestination"]["deliveryDestinationType"].as_str(),
        Some(destination.destination_type),
        "case {:?}: this resource implies the {} destination type, got {answer}",
        destination.label,
        destination.destination_type
    );
    answer["deliveryDestination"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a put answers the destination's ARN, got {answer}"))
        .to_string()
}

/// The `delivery` member of a response that carries one.
fn delivery(answer: &Value, case: &str) -> Value {
    answer
        .get("delivery")
        .filter(|delivery| !delivery.is_null())
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: expected a delivery, got {answer}"))
}

/// A delivery of one sender's logs to one destination, created with NO configuration member at
/// all, so every member of its configuration is a default.
async fn create_unconfigured(logs: &Logs, source: &str, destination_arn: &str, case: &str) -> Value {
    let answer = logs
        .ok(
            CREATE,
            &json!({
                "deliverySourceName": source,
                "deliveryDestinationArn": destination_arn,
            }),
        )
        .await;
    delivery(&answer, case)
}

/// A delivery created with no configuration reports the record fields and the object layout the
/// catalogue publishes for its logs — the layout whatever kind of destination it is pointed at —
/// and both readers report the same.
#[tokio::test]
async fn an_unconfigured_delivery_reports_its_senders_published_defaults() {
    let logs = Logs::new();
    for sender in senders() {
        provision_source(&logs, &sender).await;
        let published_layout = published_object_layout(&logs, &sender).await;

        for destination in &DESTINATIONS {
            let case = format!("{} to {}", sender.label, destination.label);
            let name = format!("{}-{}", sender.source, destination.destination_type);
            let destination_arn = provision_destination(&logs, &name, destination).await;
            let created = create_unconfigured(&logs, sender.source, &destination_arn, &case).await;

            // The record fields are the governing template's published default list, in the
            // published order.
            let governing = template(&logs, &sender, destination.destination_type).await;
            let expected_fields = defaults(&governing, &case).get("recordFields").cloned();
            assert_eq!(
                created.get("recordFields").cloned(),
                expected_fields,
                "case {case}: expected the published default record fields {expected_fields:?}, \
                 got {created}"
            );

            // The object layout is the SENDER's, whatever kind of destination this delivery
            // pairs it with: both of its members, byte for byte as the catalogue publishes them.
            let reported_layout = created
                .get("s3DeliveryConfiguration")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_else(|| {
                    panic!("case {case}: a delivery reports its object layout, got {created}")
                });
            for member in ["suffixPath", "enableHiveCompatiblePath"] {
                assert_eq!(
                    reported_layout.get(member),
                    published_layout.get(member),
                    "case {case}: expected {member} {:?} \u{2014} the value this sender publishes \u{2014} got {:?}",
                    published_layout.get(member),
                    reported_layout.get(member)
                );
            }

            // The round trip: the reader reports the delivery the create reported.
            let id = created["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a created delivery carries an id"))
                .to_string();
            let fetched = delivery(&logs.ok(GET, &json!({"id": id})).await, &case);
            assert_eq!(
                fetched, created,
                "case {case}: {GET} reports the delivery the create reported"
            );
        }
    }
}

/// A suffix path a delivery states is the caller's, and it is the only thing that displaces the
/// published default — on a delivery to any kind of destination.
#[tokio::test]
async fn a_stated_suffix_path_displaces_the_published_default() {
    let logs = Logs::new();
    let sender = senders()
        .into_iter()
        .next()
        .expect("the table names at least one sender");
    provision_source(&logs, &sender).await;
    let published_layout = published_object_layout(&logs, &sender).await;

    // (case, the suffix path the create states): a path written in the variables the sender
    // publishes, one written in none of them, and the longest one the member admits
    // (`S3DeliveryConfiguration$suffixPath` is `@length(min: 1, max: 256)`).
    let stated: [(&str, String); 3] = [
        ("a path in the sender's own variables", "{accountid}/{region}/{yyyy}/".to_string()),
        ("a path with no variable at all", "flat-prefix/".to_string()),
        ("the longest path the member admits", "p".repeat(256)),
    ];

    for (index, (case, suffix_path)) in stated.into_iter().enumerate() {
        let destination = &DESTINATIONS[index % DESTINATIONS.len()];
        let name = format!("stated-{index}");
        let destination_arn = provision_destination(&logs, &name, destination).await;
        let answer = logs
            .ok(
                CREATE,
                &json!({
                    "deliverySourceName": sender.source,
                    "deliveryDestinationArn": destination_arn,
                    "s3DeliveryConfiguration": {"suffixPath": suffix_path},
                }),
            )
            .await;
        let created = delivery(&answer, case);
        let reported = created["s3DeliveryConfiguration"]["suffixPath"]
            .as_str()
            .map(ToString::to_string);
        assert_eq!(
            reported.as_deref(),
            Some(suffix_path.as_str()),
            "case {case}: the stated suffix path is the delivery's, got {created}"
        );
        // The member the request left unstated inside the layout is still the published default.
        assert_eq!(
            created["s3DeliveryConfiguration"].get("enableHiveCompatiblePath"),
            published_layout.get("enableHiveCompatiblePath"),
            "case {case}: the unstated member of the layout is the published default, got {created}"
        );
        let id = created["id"].as_str().expect("a created delivery has an id");
        let fetched = delivery(&logs.ok(GET, &json!({"id": id})).await, case);
        assert_eq!(
            fetched, created,
            "case {case}: {GET} reports the delivery the create reported"
        );
    }
}


