// SPDX-License-Identifier: CC0-1.0

//! Cloud Control API over the queues Amazon SQS holds.
//!
//! The rule every case here holds the service to: **a resource of a type another service
//! of this emulator owns is resolved, enumerated, patched and removed in THAT service's
//! own state, whichever API created it — "you can use this action to return information
//! about an existing resource in your account and Amazon Web Services Region, whether
//! those resources were provisioned using Cloud Control API" (`GetResource`) — so an
//! `AWS::SQS::Queue` reads as the queue Amazon SQS holds, a write through Cloud Control
//! API is a write to that queue, and an identifier Amazon SQS holds no queue for is
//! refused with the modelled `ResourceNotFoundException` (HTTP 404) by the synchronous
//! operations, and failed with the handler's `NotFound` verdict by the asynchronous
//! delete.**
//!
//! The rows differ along what the rule generalises over: which API the queue came into
//! being through, which attributes it carries and in which form (a count, a flag, a
//! document, a tag set), which property a patch changes, which operation addresses an
//! identifier nothing answers to, and \u2014 for a count \u2014 each published bound of its range and
//! a value past it.
//!
//! `native_resource_resolution` holds the same rule for the two `AWS::Logs::*` types, so
//! what is asserted across the two suites is where a resource LIVES rather than anything
//! about one owning service.

use serde_json::{json, Map, Value};

use super::emulator::{
    code, descriptions, message, progress, properties_of, text, Emulator, SIGNING_REGION,
    STATUS_FAILED, STATUS_SUCCESS,
};

/// The type whose resources live in the Amazon SQS service's own store.
pub(crate) const QUEUE: &str = "AWS::SQS::Queue";

/// The url of the queue called `name`, as Amazon SQS names one:
/// `https://<endpoint>/<accountId>/<QueueName>`. This is the type's primary identifier
/// (`primaryIdentifier: ["/properties/QueueUrl"]`).
pub(crate) fn queue_url(name: &str) -> String {
    format!(
        "https://sqs.{SIGNING_REGION}.amazonaws.com/{}/{name}",
        engine::emulator_account_id()
    )
}

/// The ARN of that queue, built by the core's own ARN builder so a case pins the queue
/// Cloud Control API resolved to the one Amazon SQS holds rather than to a string this
/// suite spells.
pub(crate) fn queue_arn(name: &str) -> String {
    engine::emulator_arn("sqs", SIGNING_REGION, name)
}

/// The model a read must report for the queue called `name`, with `overrides` applied.
///
/// A queue carries the documented default of every attribute that applies to it, so that
/// is what a read reports, in the form the type's schema declares each property in.
/// Catalog source: the defaults in the `Attributes` documentation of `CreateQueue` —
/// `DelaySeconds` "Default: 0", `MaximumMessageSize` "Default: 1,048,576 bytes (1 MiB)",
/// `MessageRetentionPeriod` "Default: 345,600 (4 days)", `ReceiveMessageWaitTimeSeconds`
/// "Default: 0", `VisibilityTimeout` "Default: 30" — plus `SqsManagedSseEnabled`, which a
/// queue nothing put on a KMS key is on.
fn expected_model(name: &str, overrides: &[(&str, Value)]) -> Value {
    let mut model = json!({
        "QueueName": name,
        "QueueUrl": queue_url(name),
        "Arn": queue_arn(name),
        "DelaySeconds": 0,
        "MaximumMessageSize": 1_048_576,
        "MessageRetentionPeriod": 345_600,
        "ReceiveMessageWaitTimeSeconds": 0,
        "VisibilityTimeout": 30,
        "SqsManagedSseEnabled": true,
    });
    for (property, value) in overrides {
        model[*property] = value.clone();
    }
    model
}

/// One `Tags` entry, as the type's schema carries a tag: a `{ Key, Value }` pair.
fn tag(key: &str, value: &str) -> Value {
    json!({ "Key": key, "Value": value })
}

/// Create a queue through the OWNING service's own API — `sqs:CreateQueue` — and answer
/// the url it reports. Nothing of such a queue is ever named to Cloud Control API.
async fn create_native(emulator: &Emulator, name: &str, attributes: &Value) -> String {
    let mut request = Map::new();
    request.insert("QueueName".to_string(), json!(name));
    if attributes.as_object().is_some_and(|map| !map.is_empty()) {
        request.insert("Attributes".to_string(), attributes.clone());
    }
    let created = emulator
        .sqs_ok("CreateQueue", Value::Object(request))
        .await;
    text(&created, "QueueUrl")
}

/// What the OWNING service reports for one attribute of the queue at `url`, or `None`
/// where it reports no value for it.
async fn native_attribute(emulator: &Emulator, url: &str, attribute: &str) -> Option<String> {
    let reported = emulator
        .sqs_ok(
            "GetQueueAttributes",
            json!({ "QueueUrl": url, "AttributeNames": [attribute] }),
        )
        .await;
    reported["Attributes"][attribute]
        .as_str()
        .map(str::to_string)
}

/// The tags the OWNING service reports for the queue at `url`, as the type's schema
/// carries them.
async fn native_tags(emulator: &Emulator, url: &str) -> Vec<Value> {
    let reported = emulator
        .sqs_ok("ListQueueTags", json!({ "QueueUrl": url }))
        .await;
    let mut tags: Vec<(String, String)> = reported["Tags"]
        .as_object()
        .map(|carried| {
            carried
                .iter()
                .map(|(key, value)| {
                    (key.clone(), value.as_str().unwrap_or_default().to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    tags.sort();
    tags.iter().map(|(key, value)| tag(key, value)).collect()
}

#[tokio::test]
async fn a_queue_the_owning_service_holds_is_the_one_cloud_control_reports() {
    let emulator = Emulator::with_sqs();
    // One queue the rows can point a redrive policy at, created natively like the rest.
    let dead_letter = "read-through-dead-letters";
    create_native(&emulator, dead_letter, &json!({})).await;
    let redrive = json!({
        "deadLetterTargetArn": queue_arn(dead_letter),
        "maxReceiveCount": 5,
    });

    // The rows differ along the attributes the natively created queue carries and the form
    // each of them takes: none beyond its identity, one count, three counts at the
    // published bounds of their ranges, a tag set, the two attributes that make a FIFO
    // queue, and a redrive policy — which the schema declares as an object.
    let rows: [(&str, Value, Value, Vec<Value>); 6] = [
        (
            "no attribute beyond its identity",
            json!({}),
            json!({}),
            Vec::new(),
        ),
        (
            "a visibility timeout",
            json!({ "VisibilityTimeout": "45" }),
            json!({ "VisibilityTimeout": 45 }),
            Vec::new(),
        ),
        (
            // "An integer from 0 to 43,200 (12 hours)", "An integer from 0 to 900 seconds
            // (15 minutes)" and "An integer from 60 seconds (1 minute) to 1,209,600 seconds
            // (14 days)" — the top of the first two ranges and the bottom of the third.
            "counts at the published bounds of their ranges",
            json!({
                "VisibilityTimeout": "43200",
                "DelaySeconds": "900",
                "MessageRetentionPeriod": "60",
            }),
            json!({
                "VisibilityTimeout": 43_200,
                "DelaySeconds": 900,
                "MessageRetentionPeriod": 60,
            }),
            Vec::new(),
        ),
        (
            "a redrive policy, which the schema declares as an object",
            json!({ "RedrivePolicy": redrive.to_string() }),
            json!({ "RedrivePolicy": redrive }),
            Vec::new(),
        ),
        (
            "a tag set",
            json!({}),
            json!({ "Tags": [tag("owner", "platform"), tag("tier", "gold")] }),
            vec![tag("owner", "platform"), tag("tier", "gold")],
        ),
        (
            "the attributes that make a FIFO queue",
            json!({ "FifoQueue": "true", "ContentBasedDeduplication": "true" }),
            json!({ "FifoQueue": true, "ContentBasedDeduplication": true }),
            Vec::new(),
        ),
    ];

    let mut created: Vec<String> = vec![queue_url(dead_letter)];
    for (row, (case, attributes, overrides, tags)) in rows.iter().enumerate() {
        // A FIFO queue's name must end with the `.fifo` suffix, which is the caller's to
        // supply.
        let name = if attributes.get("FifoQueue").is_some() {
            format!("read-through-{row}.fifo")
        } else {
            format!("read-through-{row}")
        };
        let url = create_native(&emulator, &name, attributes).await;
        if !tags.is_empty() {
            let carried: Map<String, Value> = tags
                .iter()
                .map(|entry| (text(entry, "Key"), json!(text(entry, "Value"))))
                .collect();
            emulator
                .sqs_ok(
                    "TagQueue",
                    json!({ "QueueUrl": url, "Tags": Value::Object(carried) }),
                )
                .await;
        }
        created.push(url.clone());

        let (status, body) = emulator.get(QUEUE, &url).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetResource must resolve a queue the owning service holds, got \
             {status} with body {body:?}"
        );
        assert_eq!(
            text(&body, "TypeName"),
            QUEUE,
            "case {case:?}: expected the type the request named, got {body:?}"
        );
        assert_eq!(
            text(&body["ResourceDescription"], "Identifier"),
            url,
            "case {case:?}: expected the queue's own url as the identifier, got {body:?}"
        );
        let overrides: Vec<(&str, Value)> = overrides
            .as_object()
            .map(|model| {
                model
                    .iter()
                    .map(|(property, value)| (property.as_str(), value.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let expected = expected_model(&name, &overrides);
        let reported = properties_of(&body["ResourceDescription"]);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the model the owning service holds, {expected:?}, got \
             {reported:?}"
        );
    }

    // The listing of the type is the owning service's inventory, in identifier order, and
    // it is walked one resource per page so the pages are built from that inventory too.
    created.sort();
    let listed = emulator.walk(QUEUE, Some(1)).await;
    assert_eq!(
        listed, created,
        "ListResources must report every queue the owning service holds, expected \
         {created:?}, got {listed:?}"
    );

    // A well-formed url of this account and Region that names no queue is not part of
    // that inventory, and reads as a resource that cannot be found.
    let phantom = queue_url("read-through-never-created");
    assert!(
        !listed.contains(&phantom),
        "ListResources must not report {phantom:?}, got {listed:?}"
    );
    let (status, body) = emulator.get(QUEUE, &phantom).await;
    assert_eq!(
        status, 404,
        "GetResource for a url naming no queue: expected HTTP 404, got {status} with body \
         {body:?}"
    );
}

#[tokio::test]
async fn a_patch_through_cloud_control_reaches_the_owning_service() {
    let emulator = Emulator::with_sqs();
    let dead_letter = "patched-dead-letters";
    create_native(&emulator, dead_letter, &json!({})).await;
    let redrive = json!({
        "deadLetterTargetArn": queue_arn(dead_letter),
        "maxReceiveCount": 3,
    });

    // The rows differ along the property the patch changes and the form its value takes —
    // a count inside its published range and at each published bound of it, one past that
    // bound, another count, a document and a tag set — and therefore along whether the
    // owning handler takes the model at all. Each row is applied to the queue the row
    // before it left behind, so the model a read reports is the one the whole sequence
    // built.
    let rows: [(&str, &str, Value, bool, Option<String>); 8] = [
        (
            "a count inside its published range",
            "VisibilityTimeout",
            json!(75),
            true,
            Some("75".to_string()),
        ),
        (
            // "Valid values: An integer from 0 to 43,200 (12 hours)."
            "a count at the bottom of its published range",
            "VisibilityTimeout",
            json!(0),
            true,
            Some("0".to_string()),
        ),
        (
            "a count at the top of its published range",
            "VisibilityTimeout",
            json!(43_200),
            true,
            Some("43200".to_string()),
        ),
        (
            "a count one past the top of its published range",
            "VisibilityTimeout",
            json!(43_201),
            false,
            Some("43200".to_string()),
        ),
        (
            // "Valid values: An integer from 0 to 900 seconds (15 minutes)."
            "another count of the same queue",
            "DelaySeconds",
            json!(900),
            true,
            Some("900".to_string()),
        ),
        (
            "a count one past the top of that range",
            "DelaySeconds",
            json!(901),
            false,
            Some("900".to_string()),
        ),
        (
            // The owning service holds this attribute in the canonical rendering of the
            // object it was given, which is what its own reader reports.
            "a document",
            "RedrivePolicy",
            redrive.clone(),
            true,
            Some(redrive.to_string()),
        ),
        (
            "a tag set",
            "Tags",
            json!([tag("owner", "platform")]),
            true,
            None,
        ),
    ];

    let name = "patched-queue";
    let url = create_native(&emulator, name, &json!({})).await;
    // A queue's access policy is an attribute of the queue that this type's schema does NOT
    // declare — `AWS::SQS::QueuePolicy` / `AWS::SQS::QueueInlinePolicy` carry it — so it is
    // written through the owning service, is no property of the model a read reports, and
    // has to survive every patch below exactly as that service holds it.
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": "*" },
            "Action": "sqs:ReceiveMessage",
            "Resource": queue_arn(name),
        }],
    });
    emulator
        .sqs_ok(
            "SetQueueAttributes",
            json!({
                "QueueUrl": url,
                "Attributes": { "Policy": document.to_string() },
            }),
        )
        .await;
    let policy = native_attribute(&emulator, &url, "Policy").await;
    assert!(
        policy.is_some(),
        "the owning service must report the policy it was given, got {policy:?}"
    );
    let mut model = emulator.properties(QUEUE, &url).await;
    assert!(
        model.get("Policy").is_none(),
        "a read must report only the properties this type's schema declares, got {model:?}"
    );
    for (case, property, value, applies, reported) in rows {
        let patch = json!([{ "op": "add", "path": format!("/{property}"), "value": value }]);
        let (status, body) = emulator.update(QUEUE, &url, &patch, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to be accepted, got {status} with body {body:?}"
        );
        let settled = emulator
            .terminal(&text(&progress(&body), "RequestToken"))
            .await;
        let status = text(&settled, "OperationStatus");
        let expected_status = if applies {
            STATUS_SUCCESS
        } else {
            STATUS_FAILED
        };
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected the request to reach {expected_status}, got {settled:?}"
        );
        if applies {
            // The patch is a patch of the WHOLE model, so a read reports the model the
            // patch left behind: the one before it with this property changed.
            model[property] = value.clone();
        }
        let stored = emulator.properties(QUEUE, &url).await;
        assert_eq!(
            stored, model,
            "case {case:?}: expected the model the patch leaves behind, {model:?}, got \
             {stored:?}"
        );
        // And the OWNING service reports the change through its own API, which is what
        // makes it a write to the queue rather than to a record of Cloud Control's own.
        match &reported {
            Some(expected) => {
                let carried = native_attribute(&emulator, &url, property).await;
                assert_eq!(
                    carried.as_deref(),
                    Some(expected.as_str()),
                    "case {case:?}: expected Amazon SQS to report {property} as \
                     {expected:?}, got {carried:?}"
                );
            }
            None => {
                let carried = native_tags(&emulator, &url).await;
                let expected = value.as_array().cloned().unwrap_or_default();
                assert_eq!(
                    carried, expected,
                    "case {case:?}: expected Amazon SQS to report the tags {expected:?}, \
                     got {carried:?}"
                );
            }
        }
        // And an attribute this type's schema does not declare is left exactly as the
        // owning service holds it, whatever this type's write changed.
        let carried = native_attribute(&emulator, &url, "Policy").await;
        assert_eq!(
            carried, policy,
            "case {case:?}: the queue's Policy must survive a write of this type, expected \
             {policy:?}, got {carried:?}"
        );
    }
}

/// One row of the create table: what a desired state names, and what the queue the create
/// provisions has to be.
struct Created {
    /// What this row is about.
    case: &'static str,
    /// The desired state the create carries.
    desired: Value,
    /// The queue's name, where the desired state names one, and `None` where it leaves
    /// the name to the provider.
    name: Option<&'static str>,
    /// The attributes the OWNING service must report for the queue afterwards.
    attributes: Vec<(&'static str, &'static str)>,
}

#[tokio::test]
async fn a_queue_cloud_control_creates_is_a_queue_the_owning_service_holds() {
    let emulator = Emulator::with_sqs();
    // The rows differ along what the desired state names: a queue's name and nothing else,
    // a name with attributes in each form the schema declares, the two attributes that make
    // a FIFO queue, and no name at all — which leaves the name to the provider, as a
    // template that declares none does.
    let rows = [
        Created {
            case: "a name and nothing else",
            desired: json!({ "QueueName": "created-plain" }),
            name: Some("created-plain"),
            attributes: Vec::new(),
        },
        Created {
            case: "a name with counts and a tag set",
            desired: json!({
                "QueueName": "created-configured",
                "VisibilityTimeout": 90,
                "MessageRetentionPeriod": 1_209_600,
                "Tags": [tag("owner", "platform")],
            }),
            name: Some("created-configured"),
            attributes: vec![
                ("VisibilityTimeout", "90"),
                ("MessageRetentionPeriod", "1209600"),
            ],
        },
        Created {
            case: "the attributes that make a FIFO queue",
            desired: json!({
                "QueueName": "created-ordered.fifo",
                "FifoQueue": true,
                "ContentBasedDeduplication": true,
            }),
            name: Some("created-ordered.fifo"),
            attributes: vec![("FifoQueue", "true"), ("ContentBasedDeduplication", "true")],
        },
        Created {
            case: "no name at all",
            desired: json!({ "VisibilityTimeout": 10 }),
            name: None,
            attributes: vec![("VisibilityTimeout", "10")],
        },
    ];

    for Created {
        case,
        desired,
        name,
        attributes,
    } in rows
    {
        let identifier = emulator.provision(QUEUE, &desired).await;
        if let Some(name) = name {
            assert_eq!(
                identifier,
                queue_url(name),
                "case {case:?}: expected the url of the queue the owning service assigns, \
                 got {identifier:?}"
            );
        } else {
            // The provider named the queue, so the identifier is a url of this account and
            // Region whose last segment is that name.
            let prefix = queue_url("");
            assert!(
                identifier.starts_with(&prefix) && identifier.len() > prefix.len(),
                "case {case:?}: expected a queue url under {prefix:?}, got {identifier:?}"
            );
        }
        // The queue EXISTS in the service that owns it: its own reader reports it, under
        // the attributes the desired state asked for.
        for (attribute, expected) in attributes {
            let carried = native_attribute(&emulator, &identifier, attribute).await;
            assert_eq!(
                carried.as_deref(),
                Some(expected),
                "case {case:?}: expected Amazon SQS to report {attribute} as {expected:?}, \
                 got {carried:?}"
            );
        }
        // And it reads back through Cloud Control API under that same identifier.
        let (status, body) = emulator.get(QUEUE, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetResource must resolve the created queue, got {status} with \
             body {body:?}"
        );
        let reported = properties_of(&body["ResourceDescription"]);
        assert_eq!(
            reported["QueueUrl"],
            json!(identifier),
            "case {case:?}: the model must carry the identifier it answers to, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_queue_cloud_control_deletes_is_gone_from_the_owning_service() {
    let emulator = Emulator::with_sqs();
    // The rows differ along which API created the queue: Cloud Control API acts on the one
    // queue either way.
    let rows = [
        ("created natively", false),
        ("created through Cloud Control", true),
    ];
    for (case, through_cloud_control) in rows {
        let name = if through_cloud_control {
            "deleted-through-cloud-control"
        } else {
            "deleted-natively-created"
        };
        let url = if through_cloud_control {
            emulator
                .provision(QUEUE, &json!({ "QueueName": name }))
                .await
        } else {
            create_native(&emulator, name, &json!({})).await
        };
        assert_eq!(
            url,
            queue_url(name),
            "case {case:?}: expected the queue's own url, got {url:?}"
        );

        emulator.delete_and_settle(QUEUE, &url).await;

        // The queue is gone from the service that OWNS it, which answers for it in its own
        // terms: `GetQueueUrl` resolves no queue of that name, and an attribute write on
        // the url is refused the same way.
        for (operation, request) in [
            ("GetQueueUrl", json!({ "QueueName": name })),
            (
                "SetQueueAttributes",
                json!({ "QueueUrl": url, "Attributes": { "VisibilityTimeout": "30" } }),
            ),
        ] {
            let (status, body) = emulator.sqs(operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?} ({operation}): expected HTTP 400, got {status} with body \
                 {body:?}"
            );
            assert!(
                code(&body).contains("QueueDoesNotExist"),
                "case {case:?} ({operation}): expected the queue-does-not-exist refusal, \
                 got {:?}",
                code(&body)
            );
            assert_eq!(
                message(&body),
                "The specified queue does not exist.",
                "case {case:?} ({operation}): unexpected wording"
            );
        }

        // And Cloud Control API reports it as a resource that cannot be found, with none
        // of the type's inventory naming it.
        let (status, body) = emulator.get(QUEUE, &url).await;
        assert_eq!(
            status, 404,
            "case {case:?}: GetResource after the delete: expected HTTP 404, got {status} \
             with body {body:?}"
        );
        let listed = emulator.list(QUEUE, None, None).await;
        let identifiers: Vec<String> = descriptions(&listed)
            .iter()
            .map(|description| text(description, "Identifier"))
            .collect();
        assert!(
            !identifiers.contains(&url),
            "case {case:?}: the deleted queue must not be listed, got {identifiers:?}"
        );
    }
}

/// RULE: a resource of this type is addressed by every reference the type's schema
/// declares — its `primaryIdentifier` `/properties/QueueUrl`, the url the owning service
/// assigned, and its `additionalIdentifiers` `/properties/Arn` and `/properties/QueueName`
/// — each resolving to the one queue it names; whichever reference addressed it, every
/// answer names the resource by the url it answers to.
///
/// The rows differ along the reference form (the assigned url, the ARN, the bare name) and
/// are driven through every resource operation that addresses one: the read, the patch and
/// the delete.
#[tokio::test]
async fn a_queue_is_addressed_by_its_url_and_by_its_name() {
    let emulator = Emulator::with_sqs();
    let patch = json!([{ "op": "add", "path": "/VisibilityTimeout", "value": 45 }]);

    for (case, name, reference_of) in [
        (
            "the url the owning service assigned",
            "addressed-by-url",
            (|name: &str| queue_url(name)) as fn(&str) -> String,
        ),
        (
            "the queue's own ARN",
            "addressed-by-arn",
            (|name: &str| queue_arn(name)) as fn(&str) -> String,
        ),
        (
            "the queue's own name",
            "addressed-by-name",
            |name: &str| name.to_string(),
        ),
    ] {
        let url = create_native(&emulator, name, &json!({})).await;
        let reference = reference_of(name);

        // The read resolves the queue and reports it under its own url.
        let (status, body) = emulator.get(QUEUE, &reference).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetResource must resolve {reference:?}, got {status} with body \
             {body:?}"
        );
        assert_eq!(
            text(&body["ResourceDescription"], "Identifier"),
            url,
            "case {case:?}: expected the resource to be named by its own url, got {body:?}"
        );
        let reported = properties_of(&body["ResourceDescription"]);
        assert_eq!(
            reported["QueueUrl"],
            json!(url),
            "case {case:?}: the model must carry the url it answers to, got {reported:?}"
        );

        // The patch reaches the same queue: the owning service reports the new value.
        let (status, body) = emulator.update(QUEUE, &reference, &patch, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateResource must accept {reference:?}, got {status} with body \
             {body:?}"
        );
        let settled = emulator
            .settled(&text(&progress(&body), "RequestToken"))
            .await;
        assert_eq!(
            text(&settled, "OperationStatus"),
            STATUS_SUCCESS,
            "case {case:?}: the patch must succeed, got {settled:?}"
        );
        let carried = native_attribute(&emulator, &url, "VisibilityTimeout").await;
        assert_eq!(
            carried.as_deref(),
            Some("45"),
            "case {case:?}: expected Amazon SQS to report the patched value, got {carried:?}"
        );

        // And the delete removes it from the owning service, which no longer holds it.
        emulator.delete_and_settle(QUEUE, &reference).await;
        let (status, body) = emulator.get(QUEUE, &url).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the deleted queue must be gone, got {status} with body {body:?}"
        );
    }
}

/// RULE: the guard that admits ONE operation at a time is about the RESOURCE, not about
/// the reference a caller addressed it by: an operation started under one of a queue's two
/// references holds the queue against an operation that arrives under the other, which is
/// the modelled `ConcurrentOperationException` (HTTP 409) naming the request that holds it.
///
/// The rows differ along which reference starts the operation and which one contends for
/// it, and along which operation contends (an update, a delete).
#[tokio::test]
async fn an_operation_started_under_one_reference_holds_the_queue_against_the_other() {
    let emulator = Emulator::with_sqs();
    let patch = json!([{ "op": "add", "path": "/VisibilityTimeout", "value": 45 }]);

    for (case, name, start_url_first) in [
        ("started under the url, contended by the name", "held-by-url", true),
        ("started under the name, contended by the url", "held-by-name", false),
    ] {
        let url = create_native(&emulator, name, &json!({})).await;
        let (starts, contends) = match start_url_first {
            true => (url.clone(), name.to_string()),
            false => (name.to_string(), url.clone()),
        };

        // The patch is accepted and still running: nothing is settled yet.
        let (status, body) = emulator.update(QUEUE, &starts, &patch, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first operation must be accepted, got {status} with body \
             {body:?}"
        );
        let holding = text(&progress(&body), "RequestToken");

        for (operation, (status, body)) in [
            (
                "UpdateResource",
                emulator.update(QUEUE, &contends, &patch, None).await,
            ),
            ("DeleteResource", emulator.delete(QUEUE, &contends).await),
        ] {
            assert_eq!(
                status, 409,
                "case {case:?} ({operation}): expected the contention refusal for \
                 {contends:?}, got {status} with body {body:?}"
            );
            assert!(
                code(&body).contains("ConcurrentOperationException"),
                "case {case:?} ({operation}): expected ConcurrentOperationException, got {:?}",
                code(&body)
            );
            assert!(
                message(&body).contains(&holding),
                "case {case:?} ({operation}): the refusal must name the request that holds \
                 the resource ({holding}), got {:?}",
                message(&body)
            );
        }

        // Once the holding operation has settled, the other reference is served again.
        emulator.settled(&holding).await;
        let (status, body) = emulator.get(QUEUE, &contends).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the settled queue must read back under {contends:?}, got \
             {status} with body {body:?}"
        );
    }
}

#[tokio::test]
async fn an_identifier_no_queue_answers_to_is_refused_as_not_found() {
    let emulator = Emulator::with_sqs();
    // One queue that DOES exist, so every refusal below is about the identifier rather
    // than about an empty account.
    create_native(&emulator, "not-found-control", &json!({})).await;

    // The rows differ along what the identifier is: a well-formed url of this account and
    // Region naming a queue nothing created, one naming a queue of another account, an ARN
    // of another account, a name no queue of this Region carries, and a value that is no
    // reference of this type at all.
    let identifiers = [
        (
            "a well-formed url nothing created",
            queue_url("not-found-phantom"),
        ),
        (
            "a url of another account",
            "https://sqs.us-east-1.amazonaws.com/123456789012/not-found-control".to_string(),
        ),
        (
            "an ARN of another account",
            "arn:aws:sqs:us-east-1:123456789012:not-found-control".to_string(),
        ),
        (
            "the name of a queue nothing created",
            "not-found-by-name".to_string(),
        ),
        ("a value that is no url at all", "not a url".to_string()),
    ];
    let patch = json!([{ "op": "add", "path": "/VisibilityTimeout", "value": 30 }]);
    for (case, identifier) in identifiers {
        // The two SYNCHRONOUS resource operations refuse the identifier outright.
        let answers = [
            ("GetResource", emulator.get(QUEUE, &identifier).await),
            (
                "UpdateResource",
                emulator.update(QUEUE, &identifier, &patch, None).await,
            ),
        ];
        for (operation, (status, body)) in answers {
            let case = format!("{case} ({operation})");
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} with body {body:?}"
            );
            assert!(
                code(&body).contains("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                code(&body)
            );
        }

        // A delete is asynchronous, so the same identifier is ADMITTED and the owning
        // handler's not-found verdict surfaces on the request record it minted.
        let (status, body) = emulator.delete(QUEUE, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?} (DeleteResource): expected HTTP 200, got {status} with body \
             {body:?}"
        );
        let failed = emulator
            .terminal(&text(&progress(&body), "RequestToken"))
            .await;
        assert_eq!(
            text(&failed, "OperationStatus"),
            STATUS_FAILED,
            "case {case:?} (DeleteResource): the delete of an absent queue must fail, got \
             {failed:?}"
        );
        assert_eq!(
            text(&failed, "ErrorCode"),
            "NotFound",
            "case {case:?} (DeleteResource): unexpected handler error code in {failed:?}"
        );
    }
}
