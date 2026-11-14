// SPDX-License-Identifier: CC0-1.0

//! HOW a configuration item identifies the resource a running configuration recorder
//! captured it for, read back through `GetResourceConfigHistory`.
//!
//! The rule pinned here: while a configuration recorder is recording and its recording
//! group covers a resource type, Config captures a configuration item for every resource
//! of that type the OWNING service reports, and the item identifies that resource the way
//! the owning service does — the id that service addresses it by as `resourceId`, its
//! custom name as `resourceName`, that resource's own ARN as `arn` — in the account and
//! Region the recorder records, stamped `ResourceDiscovered` for the first state captured.
//! `GetResourceConfigHistory` then reports that item for that resource key, and refuses a
//! key no configuration item was captured for with `ResourceNotDiscoveredException`.
//!
//! No row calls `PutResourceConfig` or any other Config write naming a resource: every
//! resource below is created through its own service's API only, so every item asserted on
//! is one the recorder observed. The rows span the two services the recorder observes and
//! the shapes a resource takes within them — an id that is a url and an id that is a bare
//! name, a resource created with no configuration, with several attributes, as a FIFO
//! queue, and carrying tags — because the rule is about where each member of the item
//! comes from rather than about one resource.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config_recorded_resource_compliance::{Emulator, ACCOUNT_ID, REGION};

/// The resource types the recorder observes here. Entries of
/// `com.amazonaws.configservice#ResourceType`, from `smithy_query(query="shape",
/// shape="ResourceType")`.
const QUEUE_RESOURCE_TYPE: &str = "AWS::SQS::Queue";
/// See [`QUEUE_RESOURCE_TYPE`].
const BUCKET_RESOURCE_TYPE: &str = "AWS::S3::Bucket";

/// The `ConfigurationItemStatus` of the first item captured for a resource — "the resource
/// was newly discovered" (`com.amazonaws.configservice#ConfigurationItemStatus`, whose
/// values are `OK`, `ResourceDiscovered`, `ResourceNotRecorded`, `ResourceDeleted` and
/// `ResourceDeletedNotRecorded`).
const RESOURCE_DISCOVERED: &str = "ResourceDiscovered";

/// The error `GetResourceConfigHistory` answers for a resource key Config captured no
/// configuration item for.
const NOT_DISCOVERED: &str = "ResourceNotDiscoveredException";

/// How long a poll waits for the recording pass the clock owes. A pass falls due a
/// fraction of a second after the call that makes it due, so this is many passes' worth of
/// slack rather than a span the behaviour depends on: every wait below ends on the
/// CONDITION, and the budget is only there so a wedged emulator fails instead of hanging.
const CONVERGENCE_BUDGET: Duration = Duration::from_secs(15);

/// How long a poll waits between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One resource created through its own service, and how the configuration item Config
/// captures for it must identify it.
struct Row {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    /// The type the resource is recorded under.
    resource_type: &'static str,
    /// The id the owning service addresses the resource by, which is the id Config records
    /// it under.
    resource_id: String,
    /// The resource's custom name, as its own service names it.
    resource_name: String,
    /// The ARN of the resource itself.
    arn: String,
    /// A tag the resource carries in its own service, for the rows that have one.
    tag: Option<(&'static str, &'static str)>,
}

/// One `GetResourceConfigHistory` call: the HTTP status and the parsed body, so a row can
/// assert on the refusal as well as on the report.
async fn history_call(emulator: &Emulator, resource_type: &str, resource_id: &str) -> (u16, Value) {
    emulator
        .call(
            "config",
            "StarlingDoveService",
            "1.1",
            "GetResourceConfigHistory",
            &json!({ "resourceType": resource_type, "resourceId": resource_id }),
        )
        .await
}

/// The configuration items reported for one resource key, newest first — empty while
/// Config has captured none, which is the refusal the read answers until the recorder's
/// pass has seen the resource.
async fn history(case: &str, emulator: &Emulator, row: &Row) -> Vec<Value> {
    let (status, body) = history_call(emulator, row.resource_type, &row.resource_id).await;
    if status == 400 && body["__type"] == json!(NOT_DISCOVERED) {
        return Vec::new();
    }
    assert_eq!(
        status, 200,
        "case {case:?}: GetResourceConfigHistory must answer 200, got {status} with {body}"
    );
    body["configurationItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Read `row`'s history until it holds exactly one item, and answer that item.
///
/// What is waited for is the pass the recorder owes, never a span: the assertion that
/// fails at the deadline names what the read did report.
async fn await_discovery_item(case: &str, emulator: &Emulator, row: &Row) -> Value {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let items = history(case, emulator, row).await;
        if items.len() == 1 {
            return items[0].clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected one configuration item for {} {}, got {items:?}",
            row.resource_type,
            row.resource_id
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Create a queue through Amazon SQS' own API, tag it where the row carries one, and
/// answer the row the configuration item Config captures for it must satisfy.
async fn queue_row(
    emulator: &Emulator,
    case: &'static str,
    name: &str,
    attributes: &Value,
    tag: Option<(&'static str, &'static str)>,
) -> Row {
    let mut request = json!({ "QueueName": name });
    if !attributes.is_null() {
        request["Attributes"] = attributes.clone();
    }
    let created = emulator.sqs("CreateQueue", &request).await;
    let queue_url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
        .to_string();
    if let Some((key, value)) = tag {
        emulator
            .sqs(
                "TagQueue",
                &json!({ "QueueUrl": queue_url, "Tags": { key: value } }),
            )
            .await;
    }
    // The ARN Amazon SQS itself reports for the queue: the owning service states the
    // resource's ARN, so that is the one its configuration item must carry rather than one
    // this test spells out.
    let attributes = emulator
        .sqs(
            "GetQueueAttributes",
            &json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
        )
        .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn, got {attributes}"))
        .to_string();
    Row {
        case,
        resource_type: QUEUE_RESOURCE_TYPE,
        // Amazon SQS addresses a queue by its url, and the queue's name is the last
        // segment of it.
        resource_id: queue_url,
        resource_name: name.to_string(),
        arn,
        tag,
    }
}

/// Create a bucket through S3's own API and answer the row its configuration item must
/// satisfy.
async fn bucket_row(
    emulator: &Emulator,
    case: &'static str,
    name: &str,
    tag: Option<(&'static str, &'static str)>,
) -> Row {
    let tags: Vec<(&str, &str)> = tag.into_iter().collect();
    emulator.create_bucket(name, &tags).await;
    Row {
        case,
        resource_type: BUCKET_RESOURCE_TYPE,
        // S3 addresses a general purpose bucket by its name, which is its id and its
        // custom name both; a bucket name is unique across every account and Region, so
        // the bucket's ARN names neither.
        resource_id: name.to_string(),
        resource_name: name.to_string(),
        arn: format!("arn:aws:s3:::{name}"),
        tag,
    }
}

#[tokio::test]
async fn a_resource_the_recorder_discovers_is_identified_as_its_own_service_identifies_it() {
    let emulator = Emulator::new();
    emulator.recording().await;

    let rows = [
        queue_row(&emulator, "a queue created with no attributes", "plain-q", &Value::Null, None)
            .await,
        queue_row(
            &emulator,
            "a queue created with several attributes",
            "configured-q",
            &json!({ "VisibilityTimeout": "45", "DelaySeconds": "9" }),
            None,
        )
        .await,
        queue_row(
            &emulator,
            "a FIFO queue, whose name carries the suffix only a FIFO queue has",
            "ordered-q.fifo",
            &json!({ "FifoQueue": "true", "ContentBasedDeduplication": "true" }),
            None,
        )
        .await,
        queue_row(
            &emulator,
            "a tagged queue",
            "tagged-q",
            &Value::Null,
            Some(("stage", "prod")),
        )
        .await,
        bucket_row(&emulator, "a bucket, whose id is a bare name", "identity-bucket", None).await,
        bucket_row(
            &emulator,
            "a tagged bucket",
            "tagged-bucket",
            Some(("owner", "platform")),
        )
        .await,
    ];

    for row in &rows {
        let case = row.case;
        let item = await_discovery_item(case, &emulator, row).await;

        assert_eq!(
            item["resourceType"],
            json!(row.resource_type),
            "case {case:?}: the item records the type the resource is recorded under, got {item}"
        );
        assert_eq!(
            item["resourceId"],
            json!(row.resource_id),
            "case {case:?}: the item records the id the owning service addresses the \
             resource by, got {item}"
        );
        assert_eq!(
            item["resourceName"],
            json!(row.resource_name),
            "case {case:?}: the item records the resource's own name, got {item}"
        );
        assert_eq!(
            item["arn"],
            json!(row.arn),
            "case {case:?}: the item records the ARN of the resource itself, got {item}"
        );
        assert_eq!(
            item["awsRegion"],
            json!(REGION),
            "case {case:?}: the item belongs to the Region the recorder records, got {item}"
        );
        assert_eq!(
            item["accountId"],
            json!(ACCOUNT_ID),
            "case {case:?}: the item belongs to the account the recorder records, got {item}"
        );
        assert_eq!(
            item["configurationItemStatus"],
            json!(RESOURCE_DISCOVERED),
            "case {case:?}: the first state captured for a resource is its discovery, got {item}"
        );
        assert!(
            !item["configurationItemCaptureTime"].is_null(),
            "case {case:?}: a captured item reports the instant it was captured, got {item}"
        );
        assert!(
            item["configuration"]
                .as_str()
                .is_some_and(|document| !document.is_empty()),
            "case {case:?}: the item carries the configuration the owning service reports, \
             got {item}"
        );
        match row.tag {
            Some((key, value)) => assert_eq!(
                item["tags"][key],
                json!(value),
                "case {case:?}: the item carries the tags the resource carries in its own \
                 service, got {item}"
            ),
            None => assert!(
                item["tags"].is_null() || item["tags"] == json!({}),
                "case {case:?}: an untagged resource's item carries no tags, got {item}"
            ),
        }

        // The read is a REPORT: reading again answers the same item. Read once rather than
        // waited for, so a second state captured in between is reported as the difference it
        // is instead of being waited out.
        let again = history(case, &emulator, row).await;
        assert_eq!(
            again,
            vec![item.clone()],
            "case {case:?}: a second read must report the same configuration item, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_resource_key_no_item_was_captured_for_is_refused() {
    let emulator = Emulator::new();
    emulator.recording().await;

    // One resource of each observed type exists, so the inventory is not empty: what the
    // rows below differ along is the KEY asked about — a resource of a type the account
    // holds one of but by an id it never held, the id of a resource that exists under
    // ANOTHER type, and a type the account holds nothing of at all.
    let queue = queue_row(&emulator, "the queue that exists", "held-q", &Value::Null, None).await;
    let bucket = bucket_row(&emulator, "the bucket that exists", "held-bucket", None).await;
    await_discovery_item(queue.case, &emulator, &queue).await;
    await_discovery_item(bucket.case, &emulator, &bucket).await;

    let unheld_queue_url = format!("https://sqs.{REGION}.amazonaws.com/{ACCOUNT_ID}/never-created");
    let cases: [(&str, &str, &str); 4] = [
        (
            "a queue url nothing created",
            QUEUE_RESOURCE_TYPE,
            unheld_queue_url.as_str(),
        ),
        (
            "a bucket name nothing created",
            BUCKET_RESOURCE_TYPE,
            "never-created-bucket",
        ),
        (
            "the bucket that exists, asked about under another type",
            QUEUE_RESOURCE_TYPE,
            bucket.resource_id.as_str(),
        ),
        (
            // Another entry of `com.amazonaws.configservice#ResourceType`, for a type no
            // resource of this account exists under.
            "a type the account holds nothing of",
            "AWS::EC2::Volume",
            "vol-00000000000000001",
        ),
    ];

    for (case, resource_type, resource_id) in cases {
        let (status, body) = history_call(&emulator, resource_type, resource_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a key no item was captured for is refused, got {status} with {body}"
        );
        assert_eq!(
            body["__type"],
            json!(NOT_DISCOVERED),
            "case {case:?}: expected {NOT_DISCOVERED}, got {body}"
        );
        assert_eq!(
            body["message"],
            json!(format!(
                "Resource {resource_id} of resourceType:{resource_type} is unknown or has \
                 not been discovered"
            )),
            "case {case:?}: expected the refusal Config words for an undiscovered resource, \
             got {body}"
        );
    }
}
