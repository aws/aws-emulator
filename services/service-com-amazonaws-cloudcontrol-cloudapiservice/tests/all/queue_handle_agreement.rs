// SPDX-License-Identifier: CC0-1.0

//! The handle Cloud Control API reports for a queue is the handle Amazon SQS hands a client.
//!
//! The rule every case here holds the service to: **for a resource type whose primary
//! identifier is an ADDRESS the owning service assigns rather than a name the caller chose
//! (`AWS::SQS::Queue`, whose `primaryIdentifier` is `/properties/QueueUrl`), every identifier
//! Cloud Control API reports for a resource — the `ProgressEvent.Identifier` of a create, the
//! `ResourceDescription.Identifier` of a read, the identifiers of a listing and the identifier
//! property of the model each carries — is character for character the address the OWNING
//! service answers the same caller with for that resource (`sqs:GetQueueUrl`), whichever API
//! created it; and the resource reads through with that service's own values and is enumerated
//! for exactly as long as that service holds it.**
//!
//! Why both halves are one rule: the identifier is the only handle a Cloud Control client is
//! handed back, and it is the handle it sends to the owning service's own operations, so an
//! identifier that is not the address that service announces names a resource neither API can
//! join to the other's.
//!
//! The rows differ along what the rule generalises over: which API the queue came into being
//! through, whether it is a standard or a FIFO queue, which attributes its create asked for
//! (none, a count at each published bound of its range, a count inside it, a tag set), and
//! which of the references the type's schema declares addressed the read.
//!
//! Nothing here spells a queue url: every expected handle is read back from the owning
//! service, so a case cannot agree with a url this suite composed while disagreeing with the
//! one a client is really handed.

use serde_json::{json, Map, Value};

use super::emulator::{code, properties_of, text, Emulator};
// The type's `TypeName` and the two readers of the OWNING service's own values are the ones
// `sqs_queue_read_through` already states — one spelling for both suites.
use super::sqs_queue_read_through::{native_attribute, native_tags, QUEUE};

/// Which API a queue comes into being through.
#[derive(Clone, Copy)]
enum CreatedThrough {
    /// The OWNING service's own API: `sqs:CreateQueue`, with nothing of the queue ever named
    /// to Cloud Control API.
    OwningService,
    /// Cloud Control API's own `CreateResource`.
    CloudControl,
}

/// One of the references a resource of this type answers to.
///
/// Catalog source: the `AWS::SQS::Queue` resource provider schema, whose `primaryIdentifier`
/// is `/properties/QueueUrl` and whose `additionalIdentifiers` are `/properties/Arn` and
/// `/properties/QueueName` — "you can specify the primary identifier, or any secondary
/// identifier defined for the resource type in its resource schema"
/// (`GetResource$Identifier`).
#[derive(Clone, Copy)]
enum Reference {
    /// The type's primary identifier: the url the owning service announced.
    Url,
    /// The queue's ARN, as that service reports it.
    Arn,
    /// The queue's name.
    Name,
}

impl Reference {
    /// How a case names this reference.
    fn label(self) -> &'static str {
        match self {
            Self::Url => "url",
            Self::Arn => "ARN",
            Self::Name => "name",
        }
    }
}

/// Every reference of the type, so a read is asserted under all of them rather than under the
/// one a recorded request happened to carry.
const REFERENCES: [Reference; 3] = [Reference::Url, Reference::Arn, Reference::Name];

/// One row of the table: a queue, the API it comes into being through, and what its create
/// asks for.
struct Row {
    /// What the row is about, named in every assertion of it.
    case: &'static str,
    /// The API the queue is created through.
    created_through: CreatedThrough,
    /// The queue's name.
    name: &'static str,
    /// Whether the row's queue is a FIFO queue.
    fifo: bool,
    /// The visibility timeout, in seconds, the create asks for — `None` for a create that
    /// asks for none and leaves the attribute at the documented default.
    ///
    /// Catalog source: the `VisibilityTimeout` entry of the `Attributes` documentation of
    /// `CreateQueue` — "An integer from 0 to 43,200 (12 hours). Default: 30".
    visibility_timeout: Option<i64>,
    /// The tags the queue carries, as `(Key, Value)` pairs.
    tags: &'static [(&'static str, &'static str)],
}

/// The rows: both creating APIs, both queue kinds, every form the create's attributes take.
const ROWS: [Row; 6] = [
    Row {
        case: "created over the owning service's own API with nothing beyond its identity",
        created_through: CreatedThrough::OwningService,
        name: "handle-native-plain",
        fifo: false,
        visibility_timeout: None,
        tags: &[],
    },
    Row {
        case: "created natively with a visibility timeout at the bottom of its range",
        created_through: CreatedThrough::OwningService,
        name: "handle-native-floor",
        fifo: false,
        visibility_timeout: Some(0),
        tags: &[("owner", "platform")],
    },
    Row {
        case: "created through Cloud Control API with a visibility timeout inside its range",
        created_through: CreatedThrough::CloudControl,
        name: "handle-cloudcontrol-inside",
        fifo: false,
        visibility_timeout: Some(90),
        tags: &[("owner", "platform"), ("tier", "gold")],
    },
    Row {
        case: "created through Cloud Control API with a visibility timeout at the top of its \
               range",
        created_through: CreatedThrough::CloudControl,
        name: "handle-cloudcontrol-ceiling",
        fifo: false,
        visibility_timeout: Some(43_200),
        tags: &[],
    },
    Row {
        case: "a FIFO queue created over the owning service's own API",
        created_through: CreatedThrough::OwningService,
        name: "handle-native-ordered.fifo",
        fifo: true,
        visibility_timeout: Some(120),
        tags: &[],
    },
    Row {
        case: "a FIFO queue created through Cloud Control API",
        created_through: CreatedThrough::CloudControl,
        name: "handle-cloudcontrol-ordered.fifo",
        fifo: true,
        visibility_timeout: None,
        tags: &[],
    },
];

/// Create the row's queue, answering the handle the API that created it reported for it: the
/// `QueueUrl` of a native create, the `ProgressEvent.Identifier` of a Cloud Control one.
async fn create(emulator: &Emulator, row: &Row) -> String {
    match row.created_through {
        CreatedThrough::OwningService => {
            // On the owning service's own wire an attribute value is text, and tags are a map.
            let mut attributes = Map::new();
            if row.fifo {
                attributes.insert("FifoQueue".to_string(), json!("true"));
            }
            if let Some(seconds) = row.visibility_timeout {
                attributes.insert("VisibilityTimeout".to_string(), json!(seconds.to_string()));
            }
            let mut request = Map::new();
            request.insert("QueueName".to_string(), json!(row.name));
            if !attributes.is_empty() {
                request.insert("Attributes".to_string(), Value::Object(attributes));
            }
            if !row.tags.is_empty() {
                let carried: Map<String, Value> = row
                    .tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect();
                // On that service's own wire the member is `tags`, a map
                // (`CreateQueueRequest$tags`).
                request.insert("tags".to_string(), Value::Object(carried));
            }
            text(
                &emulator
                    .sqs_ok("CreateQueue", Value::Object(request))
                    .await,
                "QueueUrl",
            )
        }
        CreatedThrough::CloudControl => {
            // In the type's schema the same values are a boolean, a number and a list of
            // `{ Key, Value }` pairs.
            let mut desired = Map::new();
            desired.insert("QueueName".to_string(), json!(row.name));
            if row.fifo {
                desired.insert("FifoQueue".to_string(), json!(true));
            }
            if let Some(seconds) = row.visibility_timeout {
                desired.insert("VisibilityTimeout".to_string(), json!(seconds));
            }
            if !row.tags.is_empty() {
                let carried: Vec<Value> = row
                    .tags
                    .iter()
                    .map(|(key, value)| json!({ "Key": key, "Value": value }))
                    .collect();
                desired.insert("Tags".to_string(), Value::Array(carried));
            }
            emulator.provision(QUEUE, &Value::Object(desired)).await
        }
    }
}

/// The url the OWNING service's own API hands a client for the queue called `name` — the
/// address a client that never went through Cloud Control API holds.
async fn announced_url(emulator: &Emulator, name: &str) -> String {
    text(
        &emulator
            .sqs_ok("GetQueueUrl", json!({ "QueueName": name }))
            .await,
        "QueueUrl",
    )
}

/// The tag set a read reports, in key order, so two renderings of one tag set compare equal.
fn tags_in_key_order(model: &Value) -> Vec<Value> {
    let mut carried: Vec<Value> = model["Tags"].as_array().cloned().unwrap_or_default();
    carried.sort_by_key(|entry| text(entry, "Key"));
    carried
}

#[tokio::test]
async fn every_identifier_cloud_control_reports_is_the_url_amazon_sqs_announces() {
    let emulator = Emulator::with_sqs();

    for row in &ROWS {
        let case = row.case;
        let reported = create(&emulator, row).await;
        let announced = announced_url(&emulator, row.name).await;
        assert_eq!(
            reported, announced,
            "case {case:?}: the handle the create reported and the url the owning service \
             announces must be one string, got {reported:?} and {announced:?}"
        );

        // Every reference the type declares resolves to that one resource, and every answer
        // names it — and the model it carries names it — by the announced url alone.
        let arn = native_attribute(&emulator, &announced, "QueueArn")
            .await
            .unwrap_or_default();
        assert!(
            !arn.is_empty(),
            "case {case:?}: the owning service must report the queue's ARN"
        );
        for reference in REFERENCES {
            let identifier = match reference {
                Reference::Url => announced.clone(),
                Reference::Arn => arn.clone(),
                Reference::Name => row.name.to_string(),
            };
            let case = format!("{case} (read by its {})", reference.label());
            let (status, body) = emulator.get(QUEUE, &identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetResource must resolve a queue the owning service holds, \
                 got {status} with body {body:?}"
            );
            let described = text(&body["ResourceDescription"], "Identifier");
            assert_eq!(
                described, announced,
                "case {case:?}: expected the announced url as the identifier, got {described:?}"
            );
            let model = properties_of(&body["ResourceDescription"]);
            assert_eq!(
                model["QueueUrl"],
                json!(announced),
                "case {case:?}: expected the announced url as the model's QueueUrl, got \
                 {model:?}"
            );
        }

        let listed = emulator.walk(QUEUE, None).await;
        assert!(
            listed.contains(&announced),
            "case {case:?}: ListResources must report the queue under the announced url \
             {announced:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_queue_reads_through_with_the_owning_service_s_values_until_it_removes_it() {
    let emulator = Emulator::with_sqs();
    // One queue no row touches, so every refusal below is about the queue the row removed
    // rather than about an account Cloud Control API reads as empty.
    let untouched = Row {
        case: "the queue no row touches",
        created_through: CreatedThrough::OwningService,
        name: "read-through-untouched",
        fifo: false,
        visibility_timeout: None,
        tags: &[],
    };
    let untouched_url = create(&emulator, &untouched).await;

    for row in &ROWS {
        let case = row.case;
        create(&emulator, row).await;
        let url = announced_url(&emulator, row.name).await;

        // What Cloud Control API reports for the resource is what the OWNING service holds
        // for it, read back from that service rather than from anything this suite chose.
        let model = emulator.properties(QUEUE, &url).await;
        assert_eq!(
            model["QueueName"],
            json!(row.name),
            "case {case:?}: expected the queue's own name, got {model:?}"
        );
        let expected_arn = native_attribute(&emulator, &url, "QueueArn").await;
        assert_eq!(
            model["Arn"].as_str().map(str::to_string),
            expected_arn,
            "case {case:?}: expected the ARN the owning service reports, {expected_arn:?}, \
             got {model:?}"
        );
        let expected_timeout = native_attribute(&emulator, &url, "VisibilityTimeout")
            .await
            .and_then(|text| text.parse::<i64>().ok());
        assert_eq!(
            model["VisibilityTimeout"].as_i64(),
            expected_timeout,
            "case {case:?}: expected the visibility timeout the owning service reports, \
             {expected_timeout:?}, got {model:?}"
        );
        let expected_tags = native_tags(&emulator, &url).await;
        assert_eq!(
            tags_in_key_order(&model),
            expected_tags,
            "case {case:?}: expected the tag set the owning service holds, {expected_tags:?}, \
             got {model:?}"
        );

        let listed = emulator.walk(QUEUE, None).await;
        assert!(
            listed.contains(&url),
            "case {case:?}: ListResources must enumerate the queue at {url:?}, got {listed:?}"
        );

        // Removed through the OWNING service's own API — and only then is the resource one
        // Cloud Control API reports as not found.
        emulator
            .sqs_ok("DeleteQueue", json!({ "QueueUrl": url }))
            .await;
        let (status, body) = emulator.get(QUEUE, &url).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 once the owning service holds the queue no \
             longer, got {status} with body {body:?}"
        );
        let reported_code = code(&body);
        assert!(
            reported_code.contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {reported_code:?}"
        );
        let listed = emulator.walk(QUEUE, None).await;
        assert!(
            !listed.contains(&url),
            "case {case:?}: the removed queue must not be listed, got {listed:?}"
        );
        assert!(
            listed.contains(&untouched_url),
            "case {case:?}: the queue the row did not touch must still be listed, got \
             {listed:?}"
        );
    }
}
