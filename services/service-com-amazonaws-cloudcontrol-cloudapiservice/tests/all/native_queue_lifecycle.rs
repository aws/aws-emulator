// SPDX-License-Identifier: CC0-1.0

//! A queue's whole life, seen through Cloud Control API.
//!
//! The rule every case here holds the service to: **Cloud Control API is a façade over the
//! resource type's own provider rather than an index of what it created, so a resource of a
//! type another service of this emulator owns resolves and is enumerated exactly while THAT
//! service holds it — "you can use this action to return information about an existing
//! resource in your account and Amazon Web Services Region, whether those resources were
//! provisioned using Cloud Control API" (`GetResource`) — whichever API created it, and once
//! the owning service no longer holds it, whichever API removed it, every reference to it is
//! refused with the modelled `ResourceNotFoundException` (HTTP 404) carrying the owning
//! handler's not-found verdict for the type and the identifier the request carried.**
//!
//! The rows differ along what the rule generalises over: which API the queue came into being
//! through, which API removed it, which of the references the type's schema declares
//! addressed each call, and whether the queue is a standard or a FIFO one.
//!
//! `native_resource_resolution` holds the same rule over the `AWS::Logs::*` types, so what
//! the two suites assert together is where a resource LIVES rather than anything about one
//! owning service, and `sqs_queue_read_through` holds the model such a queue reads as.

use serde_json::{json, Value};

use super::emulator::{code, message, properties_of, text, Emulator};
use super::refusal_wording::{assert_handler_not_found, generic_not_found};
// The url and ARN shape of this type, and its `TypeName`, are the ones
// `sqs_queue_read_through` already states — one spelling for both suites, so they cannot
// drift apart: the url Amazon SQS assigns
// (`https://<endpoint>/<accountId>/<QueueName>`, the schema's `primaryIdentifier`
// `/properties/QueueUrl`) and the ARN its own builder mints (`/properties/Arn`).
use super::sqs_queue_read_through::{queue_arn, queue_url, QUEUE};

/// One of the references a resource of this type answers to.
///
/// Catalog source: the `AWS::SQS::Queue` resource provider schema, whose `primaryIdentifier`
/// is `/properties/QueueUrl` and whose `additionalIdentifiers` are `/properties/Arn` and
/// `/properties/QueueName` — "you can specify the primary identifier, or any secondary
/// identifier defined for the resource type in its resource schema"
/// (`GetResource$Identifier`).
#[derive(Clone, Copy)]
enum Reference {
    /// The type's primary identifier: the url Amazon SQS assigned the queue.
    Url,
    /// The queue's ARN, one of the type's additional identifiers.
    Arn,
    /// The queue's name, the other one.
    Name,
}

impl Reference {
    /// How this reference spells the queue called `name`.
    fn of(self, name: &str) -> String {
        match self {
            Self::Url => queue_url(name),
            Self::Arn => queue_arn(name),
            Self::Name => name.to_string(),
        }
    }

    /// How a case names this reference.
    fn label(self) -> &'static str {
        match self {
            Self::Url => "url",
            Self::Arn => "ARN",
            Self::Name => "name",
        }
    }
}

/// Every reference of the type, so each case asserts the rule under all of them rather than
/// under the one the recorded request happened to carry.
const REFERENCES: [Reference; 3] = [Reference::Url, Reference::Arn, Reference::Name];

/// Which API a queue came into being — or stopped existing — through.
#[derive(Clone, Copy)]
enum Api {
    /// The OWNING service's own API: `sqs:CreateQueue` and `sqs:DeleteQueue`, with nothing
    /// of the queue ever named to Cloud Control API.
    Owning,
    /// Cloud Control API's own resource operations.
    CloudControl,
}

/// One row of the table: a queue, the APIs its life runs through, and the reference a Cloud
/// Control delete addresses it by.
struct Row {
    /// What the row is about, named in every assertion of it.
    case: &'static str,
    /// The API the queue is created through.
    created_through: Api,
    /// The API the queue is removed through.
    removed_through: Api,
    /// The queue's name.
    name: &'static str,
    /// Whether the row's queue is a FIFO queue.
    fifo: bool,
    /// The reference a delete through Cloud Control API addresses the queue by.
    removed_by: Reference,
}

/// Create the row's queue through `api`, answering the url Amazon SQS assigned it.
async fn create_through(emulator: &Emulator, api: Api, name: &str, fifo: bool) -> String {
    match api {
        Api::Owning => {
            let mut request = json!({ "QueueName": name });
            if fifo {
                // On that service's own wire an attribute value is text.
                request["Attributes"] = json!({ "FifoQueue": "true" });
            }
            text(&emulator.sqs_ok("CreateQueue", request).await, "QueueUrl")
        }
        Api::CloudControl => {
            let mut desired = json!({ "QueueName": name });
            if fifo {
                // In the type's schema the same property is a boolean.
                desired["FifoQueue"] = json!(true);
            }
            emulator.provision(QUEUE, &desired).await
        }
    }
}

/// Remove the queue at `url` through `api` — addressing it, through Cloud Control API, by
/// `reference`.
async fn remove_through(emulator: &Emulator, api: Api, url: &str, reference: &str) {
    match api {
        Api::Owning => {
            emulator
                .sqs_ok("DeleteQueue", json!({ "QueueUrl": url }))
                .await;
        }
        Api::CloudControl => {
            emulator.delete_and_settle(QUEUE, reference).await;
        }
    }
}

/// Every queue url the OWNING service's own reader reports.
async fn native_urls(emulator: &Emulator) -> Vec<String> {
    let listed = emulator.sqs_ok("ListQueues", json!({})).await;
    listed["QueueUrls"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|url| url.as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn a_queue_resolves_exactly_while_amazon_sqs_holds_it() {
    let emulator = Emulator::with_sqs();
    // One queue created over the owning service's own API that no row touches, so every
    // refusal below is about the queue the row removed rather than about an account Cloud
    // Control API reads as empty.
    let untouched = "lifecycle-untouched";
    let untouched_url = create_through(&emulator, Api::Owning, untouched, false).await;

    // The rows differ along which API the queue came into being through, which API removed
    // it, the reference a Cloud Control delete addressed it by, and whether it is a standard
    // or a FIFO queue.
    let rows = [
        Row {
            case: "created and removed through the owning service's own API",
            created_through: Api::Owning,
            removed_through: Api::Owning,
            name: "lifecycle-native-native",
            fifo: false,
            removed_by: Reference::Url,
        },
        Row {
            case: "created natively, removed through Cloud Control API by its name",
            created_through: Api::Owning,
            removed_through: Api::CloudControl,
            name: "lifecycle-native-cloudcontrol",
            fifo: false,
            removed_by: Reference::Name,
        },
        Row {
            case: "created through Cloud Control API, removed natively",
            created_through: Api::CloudControl,
            removed_through: Api::Owning,
            name: "lifecycle-cloudcontrol-native",
            fifo: false,
            removed_by: Reference::Url,
        },
        Row {
            case: "created and removed through Cloud Control API by its url",
            created_through: Api::CloudControl,
            removed_through: Api::CloudControl,
            name: "lifecycle-cloudcontrol-cloudcontrol",
            fifo: false,
            removed_by: Reference::Url,
        },
        Row {
            case: "a FIFO queue created and removed through the owning service's own API",
            created_through: Api::Owning,
            removed_through: Api::Owning,
            name: "lifecycle-ordered.fifo",
            fifo: true,
            removed_by: Reference::Url,
        },
    ];

    for row in rows {
        let Row {
            case,
            created_through,
            removed_through,
            name,
            fifo,
            removed_by,
        } = row;
        let url = create_through(&emulator, created_through, name, fifo).await;
        assert_eq!(
            url,
            queue_url(name),
            "case {case:?}: expected the url the owning service assigns the queue, got \
             {url:?}"
        );
        let held = native_urls(&emulator).await;
        assert!(
            held.contains(&url),
            "case {case:?}: the owning service must hold the created queue, it holds {held:?}"
        );

        // While Amazon SQS holds it, EVERY reference of the type resolves to it, and every
        // answer names it by the one identifier it answers to.
        for reference in REFERENCES {
            let identifier = reference.of(name);
            let case = format!("{case} (read by its {})", reference.label());
            let (status, body) = emulator.get(QUEUE, &identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetResource must resolve a queue the owning service holds, \
                 got {status} with body {body:?}"
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
            let reported = properties_of(&body["ResourceDescription"]);
            assert_eq!(
                reported["QueueName"],
                json!(name),
                "case {case:?}: expected the model of the queue Amazon SQS holds, got \
                 {reported:?}"
            );
            assert_eq!(
                reported["Arn"],
                json!(queue_arn(name)),
                "case {case:?}: expected the ARN the owning service assigns, got {reported:?}"
            );
        }
        let listed = emulator.walk(QUEUE, None).await;
        assert!(
            listed.contains(&url),
            "case {case:?}: ListResources must report a queue the owning service holds, got \
             {listed:?}"
        );

        remove_through(&emulator, removed_through, &url, &removed_by.of(name)).await;

        let held = native_urls(&emulator).await;
        assert!(
            !held.contains(&url),
            "case {case:?}: the owning service must hold the queue no longer, it holds \
             {held:?}"
        );
        // And once it is gone THERE, every reference of it is refused here — with the
        // owning handler's not-found verdict for the identifier the request carried.
        for reference in REFERENCES {
            let identifier = reference.of(name);
            let case = format!("{case} (read by its {} after the delete)", reference.label());
            let (status, body) = emulator.get(QUEUE, &identifier).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} with body {body:?}"
            );
            assert!(
                code(&body).contains("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                code(&body)
            );
            let request_token = assert_handler_not_found(
                &message(&body),
                QUEUE,
                &generic_not_found(QUEUE, &identifier),
                &case,
            );
            assert!(
                !request_token.is_empty(),
                "case {case:?}: the refusal must name the handler invocation that failed"
            );
        }
        let listed = emulator.walk(QUEUE, None).await;
        assert!(
            !listed.contains(&url),
            "case {case:?}: the removed queue must not be listed, got {listed:?}"
        );

        // The delete removed THAT queue: the one no row touches is still the queue Cloud
        // Control API resolves.
        let (status, body) = emulator.get(QUEUE, &untouched_url).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the queue the row did not touch must still resolve, got {status} \
             with body {body:?}"
        );
        let reported: Value = properties_of(&body["ResourceDescription"]);
        assert_eq!(
            reported["QueueName"],
            json!(untouched),
            "case {case:?}: expected the untouched queue's own model, got {reported:?}"
        );
    }
}
