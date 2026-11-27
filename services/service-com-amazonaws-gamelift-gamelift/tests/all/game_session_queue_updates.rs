// SPDX-License-Identifier: CC0-1.0

//! `UpdateGameSessionQueue`, over the wire: the settings a request supplies are what the
//! stored placement queue carries afterwards, the ones it omits keep the values the queue
//! already had, a configuration structure supplied empty is removed, and the response is
//! the same stored record `DescribeGameSessionQueues` serves back.
//!
//! The harness (`GameLift`) and the queue helpers (`queue_arn`, `destinations`,
//! `latency_policies`, `describe_queues`) are the ones `game_session_queues` already
//! drives the create and describe paths with.

use serde_json::{json, Value};

use super::game_session_queues::{
    describe_queues, destinations, every_log_destination, latency_policies, log_destination,
    queue_arn, LOG_DESTINATION_TYPES, MAX_LOG_DESTINATIONS, MAX_TIMEOUT, MIN_TIMEOUT,
    PRIORITY_TYPES,
};
use super::hosting_loop::GameLift;

/// The bounds this emulator puts on the two queue list members the model leaves
/// unbounded, read from the constants the handlers enforce.
const MAX_DESTINATIONS: usize =
    service_com_amazonaws_gamelift_gamelift::state::MAX_QUEUE_DESTINATIONS;
/// See [`MAX_DESTINATIONS`].
const MAX_LATENCY_POLICIES: usize =
    service_com_amazonaws_gamelift_gamelift::state::MAX_QUEUE_PLAYER_LATENCY_POLICIES;

/// The configuration every queue below starts from: every member of the modelled
/// `GameSessionQueue` set to something, so a row that supplies a member proves the
/// REPLACEMENT and a row that omits one proves it was left alone.
fn base_request() -> Value {
    json!({
        "TimeoutInSeconds": 120,
        "Destinations": destinations(2),
        "PlayerLatencyPolicies": latency_policies(2),
        "FilterConfiguration": {"AllowedLocations": ["us-west-2", "eu-central-1"]},
        "PriorityConfiguration": {
            "PriorityOrder": ["LATENCY", "LOCATION"],
            "LocationOrder": ["us-west-2", "eu-central-1"],
        },
        "CustomEventData": "base-event-data",
        "NotificationTarget": "arn:aws:sns:us-west-2:123456789012:base-topic",
    })
}

/// Create a queue named `name` from [`base_request`] and answer the record the create
/// reported.
async fn create_base_queue(gamelift: &GameLift, name: &str) -> Value {
    let mut request = base_request();
    request["Name"] = json!(name);
    let created = gamelift.ok("CreateGameSessionQueue", request).await;
    created["GameSessionQueue"].clone()
}

/// `stored` with `changes` applied: a member mapped to a value replaces it, and a member
/// mapped to `null` is one the resulting queue must NOT report at all.
fn with_changes(stored: &Value, changes: &Value) -> Value {
    let mut expected = stored.clone();
    let members = expected
        .as_object_mut()
        .expect("a GameSessionQueue is a JSON object");
    for (member, value) in changes
        .as_object()
        .expect("the expected changes are a JSON object")
    {
        if value.is_null() {
            members.remove(member);
        } else {
            members.insert(member.clone(), value.clone());
        }
    }
    expected
}

/// An update replaces every setting it supplies, leaves every setting it omits as stored,
/// and removes a configuration structure supplied as an empty set.
///
/// The rows differ along what the rule generalises over: which members the request
/// supplies, the SIZE of each list member (none, one, several), the timeout at each
/// documented bound, which modelled `PriorityType` values the prioritization names, and
/// whether a supplied configuration structure carries anything.
#[tokio::test]
async fn an_update_replaces_what_it_supplies_and_leaves_the_rest_as_stored() {
    struct Case {
        label: &'static str,
        /// The update, without its `Name` — each row updates its own queue.
        update: Value,
        /// What the resulting queue must report differently from the created one; a
        /// member mapped to `null` must not be reported at all.
        changes: Value,
    }

    let cases = vec![
        Case {
            label: "nothing but the required name",
            update: json!({}),
            changes: json!({}),
        },
        Case {
            label: "the timeout at the documented minimum",
            update: json!({"TimeoutInSeconds": MIN_TIMEOUT}),
            changes: json!({"TimeoutInSeconds": MIN_TIMEOUT}),
        },
        Case {
            label: "the timeout at the documented maximum",
            update: json!({"TimeoutInSeconds": MAX_TIMEOUT}),
            changes: json!({"TimeoutInSeconds": MAX_TIMEOUT}),
        },
        Case {
            label: "a complete destination list longer than the stored one",
            update: json!({"Destinations": destinations(3)}),
            changes: json!({"Destinations": destinations(3)}),
        },
        Case {
            label: "a complete destination list shorter than the stored one",
            update: json!({"Destinations": destinations(1)}),
            changes: json!({"Destinations": destinations(1)}),
        },
        Case {
            label: "an empty destination list, which leaves the queue with none",
            update: json!({"Destinations": []}),
            changes: json!({"Destinations": []}),
        },
        Case {
            label: "a destination list at the bound on a queue's destinations",
            update: json!({"Destinations": destinations(MAX_DESTINATIONS)}),
            changes: json!({"Destinations": destinations(MAX_DESTINATIONS)}),
        },
        Case {
            label: "a complete policy collection of one policy",
            update: json!({"PlayerLatencyPolicies": latency_policies(1)}),
            changes: json!({"PlayerLatencyPolicies": latency_policies(1)}),
        },
        Case {
            label: "an empty policy collection, which leaves the queue with none",
            update: json!({"PlayerLatencyPolicies": []}),
            changes: json!({"PlayerLatencyPolicies": []}),
        },
        Case {
            label: "a policy collection at the bound on a queue's latency policies",
            update: json!({"PlayerLatencyPolicies": latency_policies(MAX_LATENCY_POLICIES)}),
            changes: json!({"PlayerLatencyPolicies": latency_policies(MAX_LATENCY_POLICIES)}),
        },
        Case {
            label: "a replacement filter configuration naming one location",
            update: json!({"FilterConfiguration": {"AllowedLocations": ["ap-south-1"]}}),
            changes: json!({"FilterConfiguration": {"AllowedLocations": ["ap-south-1"]}}),
        },
        Case {
            label: "an empty filter configuration, which removes the existing one",
            update: json!({"FilterConfiguration": {}}),
            changes: json!({"FilterConfiguration": null}),
        },
        Case {
            label: "a replacement prioritization naming one type the base did not",
            update: json!({"PriorityConfiguration": {"PriorityOrder": ["COST"]}}),
            changes: json!({"PriorityConfiguration": {"PriorityOrder": ["COST"]}}),
        },
        Case {
            label: "a replacement prioritization naming every modelled priority type",
            update: json!({
                "PriorityConfiguration": {
                    "PriorityOrder": PRIORITY_TYPES,
                    "LocationOrder": ["ap-south-1"],
                },
            }),
            changes: json!({
                "PriorityConfiguration": {
                    "PriorityOrder": PRIORITY_TYPES,
                    "LocationOrder": ["ap-south-1"],
                },
            }),
        },
        Case {
            label: "an empty prioritization, which removes the existing one",
            update: json!({"PriorityConfiguration": {}}),
            changes: json!({"PriorityConfiguration": null}),
        },
        Case {
            label: "placement notification and custom event data",
            update: json!({
                "NotificationTarget": "arn:aws:sns:us-west-2:123456789012:moved-topic",
                "CustomEventData": "moved-event-data",
            }),
            changes: json!({
                "NotificationTarget": "arn:aws:sns:us-west-2:123456789012:moved-topic",
                "CustomEventData": "moved-event-data",
            }),
        },
        Case {
            label: "every member at once",
            update: json!({
                "TimeoutInSeconds": 300,
                "Destinations": destinations(1),
                "PlayerLatencyPolicies": latency_policies(3),
                "FilterConfiguration": {"AllowedLocations": ["ca-central-1"]},
                "PriorityConfiguration": {"PriorityOrder": ["DESTINATION"]},
                "CustomEventData": "",
                "NotificationTarget": "arn:aws:sns:us-west-2:123456789012:everything",
            }),
            changes: json!({
                "TimeoutInSeconds": 300,
                "Destinations": destinations(1),
                "PlayerLatencyPolicies": latency_policies(3),
                "FilterConfiguration": {"AllowedLocations": ["ca-central-1"]},
                "PriorityConfiguration": {"PriorityOrder": ["DESTINATION"]},
                "CustomEventData": "",
                "NotificationTarget": "arn:aws:sns:us-west-2:123456789012:everything",
            }),
        },
    ];

    let gamelift = GameLift::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let name = format!("update-row-{index}");
        let stored = create_base_queue(&gamelift, &name).await;
        let expected = with_changes(&stored, &case.changes);

        // The queue is selected by NAME on the first update and by the ARN the create
        // minted on the repeat below: either form of `GameSessionQueueNameOrArn` names the
        // same queue.
        let mut request = case.update.clone();
        request["Name"] = json!(&name);
        let updated = gamelift.ok("UpdateGameSessionQueue", request.clone()).await;
        assert_eq!(
            updated["GameSessionQueue"], expected,
            "case {label:?}: the update must report the stored queue, got {}",
            updated["GameSessionQueue"]
        );

        // The describe path serves the SAME stored record, so the update persisted.
        let described = describe_queues(&gamelift, json!({"Names": [&name]})).await;
        assert_eq!(
            described,
            json!([expected.clone()]),
            "case {label:?}: the describe must report the updated record, got {described}"
        );

        // The same settings applied a second time, selecting the queue by its ARN, leave
        // the queue exactly as the first update left it.
        let mut repeat = case.update.clone();
        repeat["Name"] = json!(queue_arn(&name));
        let repeated = gamelift.ok("UpdateGameSessionQueue", repeat).await;
        assert_eq!(
            repeated["GameSessionQueue"], expected,
            "case {label:?}: repeating the update by ARN must report the same record, got {}",
            repeated["GameSessionQueue"]
        );
    }
}

/// An update touches only the queue it names: every other stored queue is left as it was.
#[tokio::test]
async fn an_update_leaves_every_other_stored_queue_as_it_was() {
    let gamelift = GameLift::new();
    let updated_name = "the-updated-queue";
    let bystanders = ["bystander-one", "bystander-two"];
    create_base_queue(&gamelift, updated_name).await;
    let mut before = Vec::new();
    for name in bystanders {
        before.push(create_base_queue(&gamelift, name).await);
    }

    gamelift
        .ok(
            "UpdateGameSessionQueue",
            json!({
                "Name": updated_name,
                "TimeoutInSeconds": MIN_TIMEOUT,
                "Destinations": [],
                "FilterConfiguration": {},
            }),
        )
        .await;

    for (name, stored) in bystanders.iter().zip(before) {
        let described = describe_queues(&gamelift, json!({"Names": [name]})).await;
        assert_eq!(
            described,
            json!([stored.clone()]),
            "case {name:?}: a queue the update did not name must be untouched, got {described}"
        );
    }
}

/// An update that breaks a documented rule, or names a queue that does not exist, is
/// refused with the modelled client error and leaves the stored queue exactly as it was.
#[tokio::test]
async fn an_update_that_is_refused_changes_nothing() {
    struct Case {
        label: &'static str,
        /// The update, without its `Name`.
        update: Value,
        /// The queue identifier the update names, or `None` for the stored queue's name.
        name: Option<String>,
        code: &'static str,
    }

    let stored_name = "the-stored-queue";
    let cases = vec![
        Case {
            label: "a timeout one below the documented minimum",
            update: json!({"TimeoutInSeconds": MIN_TIMEOUT - 1}),
            name: None,
            code: "InvalidRequestException",
        },
        Case {
            label: "a timeout of zero, which the model allows and the service does not",
            update: json!({"TimeoutInSeconds": 0}),
            name: None,
            code: "InvalidRequestException",
        },
        Case {
            label: "a timeout one above the documented maximum",
            update: json!({"TimeoutInSeconds": MAX_TIMEOUT + 1}),
            name: None,
            code: "InvalidRequestException",
        },
        Case {
            label: "a priority type listed twice",
            update: json!({"PriorityConfiguration": {"PriorityOrder": ["COST", "COST"]}}),
            name: None,
            code: "InvalidRequestException",
        },
        Case {
            label: "a location listed twice",
            update: json!({
                "PriorityConfiguration": {
                    "PriorityOrder": ["LOCATION"],
                    "LocationOrder": ["us-west-2", "us-west-2"],
                },
            }),
            name: None,
            code: "InvalidRequestException",
        },
        Case {
            label: "one destination past the bound on a queue's destinations",
            update: json!({"Destinations": destinations(MAX_DESTINATIONS + 1)}),
            name: None,
            code: "InvalidRequestException",
        },
        Case {
            label: "one policy past the bound on a queue's latency policies",
            update: json!({"PlayerLatencyPolicies": latency_policies(MAX_LATENCY_POLICIES + 1)}),
            name: None,
            code: "InvalidRequestException",
        },
        Case {
            label: "a name no stored queue carries",
            update: json!({"TimeoutInSeconds": 300}),
            name: Some("no-such-queue".to_string()),
            code: "InvalidRequestException",
        },
        Case {
            label: "an ARN no stored queue carries",
            update: json!({"TimeoutInSeconds": 300}),
            name: Some(queue_arn("no-such-queue")),
            code: "InvalidRequestException",
        },
    ];

    let gamelift = GameLift::new();
    let stored = create_base_queue(&gamelift, stored_name).await;
    for case in cases {
        let label = case.label;
        let mut request = case.update.clone();
        request["Name"] = json!(case.name.clone().unwrap_or_else(|| stored_name.to_string()));
        let (status, code) = gamelift.err("UpdateGameSessionQueue", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, case.code),
            "case {label:?}: the update must be refused with {}, got {status} {code}",
            case.code
        );

        let described = describe_queues(&gamelift, json!({"Names": [stored_name]})).await;
        assert_eq!(
            described,
            json!([stored.clone()]),
            "case {label:?}: a refused update must leave the queue as it was, got {described}"
        );
    }
}

/// A refusal names the queue identifier the CALLER supplied, whichever of the two forms
/// `GameSessionQueueNameOrArn` accepts it was.
#[tokio::test]
async fn a_missing_queue_is_reported_with_the_identifier_the_request_named() {
    let gamelift = GameLift::new();
    for identifier in ["absent-queue".to_string(), queue_arn("absent-queue")] {
        let (status, body) = gamelift
            .call(
                "UpdateGameSessionQueue",
                &json!({"Name": &identifier, "TimeoutInSeconds": 300}),
            )
            .await;
        let message = super::hosting_loop::error_message(&body);
        assert_eq!(
            (status, message.as_str()),
            (
                400,
                format!("Queue name {identifier} does not exist").as_str()
            ),
            "case {identifier:?}: the refusal must name the identifier the request supplied, \
             got {status} {message:?}"
        );
    }
}

/// Create a queue named `name` from [`base_request`], delivering its placement insight
/// logs to `carried`, and answer the record the create reported.
async fn create_queue_logging_to(gamelift: &GameLift, name: &str, carried: &[Value]) -> Value {
    let mut request = base_request();
    request["Name"] = json!(name);
    if !carried.is_empty() {
        request["LogConfiguration"] = json!({"DeliveryDestinations": carried});
    }
    let created = gamelift.ok("CreateGameSessionQueue", request).await;
    created["GameSessionQueue"].clone()
}

/// An update COMPOSES the queue's log configuration: the destinations it adds join the
/// ones the queue already delivers to, the ones it removes are dropped, and a queue left
/// delivering nowhere reports no configuration at all.
///
/// The rows differ along what the rule generalises over: whether the queue already
/// delivers anywhere, how many destinations the request adds or removes, which modelled
/// destination type each one is, whether the destination names the resource itself, and
/// whether the request adds, removes, or does both at once.
#[tokio::test]
async fn an_update_composes_the_log_configuration_from_what_it_adds_and_removes() {
    struct Case {
        label: &'static str,
        /// The destinations the queue is created delivering to.
        carried: Value,
        /// The update, without its `Name`.
        update: Value,
        /// The destinations the queue must deliver to afterwards; an empty list is a
        /// queue that must report no log configuration at all.
        expected: Value,
    }

    let cloudwatch = log_destination(LOG_DESTINATION_TYPES[0], None);
    let bucket = log_destination(LOG_DESTINATION_TYPES[1], Some("placement-logs"));
    let stream = log_destination(LOG_DESTINATION_TYPES[2], Some("stream/placements"));

    let cases = vec![
        Case {
            label: "adding one destination to a queue that delivers nowhere",
            carried: json!([]),
            update: json!({"AddLogDeliveryDestinations": [cloudwatch]}),
            expected: json!([cloudwatch]),
        },
        Case {
            label: "adding every modelled destination type at once",
            carried: json!([]),
            update: json!({"AddLogDeliveryDestinations": every_log_destination()}),
            expected: every_log_destination(),
        },
        Case {
            label: "adding a second destination beside the one carried",
            carried: json!([cloudwatch]),
            update: json!({"AddLogDeliveryDestinations": [bucket]}),
            expected: json!([cloudwatch, bucket]),
        },
        Case {
            label: "adding the destination the queue already delivers to",
            carried: json!([cloudwatch]),
            update: json!({"AddLogDeliveryDestinations": [cloudwatch]}),
            expected: json!([cloudwatch]),
        },
        Case {
            label: "filling the configuration to the bound by adding two",
            carried: json!([cloudwatch]),
            update: json!({"AddLogDeliveryDestinations": [bucket, stream]}),
            expected: json!([cloudwatch, bucket, stream]),
        },
        Case {
            label: "removing the one destination carried, which leaves no configuration",
            carried: json!([cloudwatch]),
            update: json!({"RemoveLogDeliveryDestinations": [cloudwatch]}),
            expected: json!([]),
        },
        Case {
            label: "removing one of the destinations carried",
            carried: json!([cloudwatch, bucket]),
            update: json!({"RemoveLogDeliveryDestinations": [cloudwatch]}),
            expected: json!([bucket]),
        },
        Case {
            label: "removing every destination carried",
            carried: every_log_destination(),
            update: json!({"RemoveLogDeliveryDestinations": every_log_destination()}),
            expected: json!([]),
        },
        Case {
            label: "removing a destination the queue does not deliver to",
            carried: json!([cloudwatch]),
            update: json!({"RemoveLogDeliveryDestinations": [stream]}),
            expected: json!([cloudwatch]),
        },
        Case {
            label: "removing one destination and adding another in one request",
            carried: json!([cloudwatch]),
            update: json!({
                "RemoveLogDeliveryDestinations": [cloudwatch],
                "AddLogDeliveryDestinations": [stream],
            }),
            expected: json!([stream]),
        },
        Case {
            label: "a destination both removed and added, which stays",
            carried: json!([cloudwatch]),
            update: json!({
                "RemoveLogDeliveryDestinations": [cloudwatch],
                "AddLogDeliveryDestinations": [cloudwatch],
            }),
            expected: json!([cloudwatch]),
        },
        Case {
            label: "an update that names neither list, leaving the configuration as stored",
            carried: json!([bucket]),
            update: json!({"TimeoutInSeconds": MIN_TIMEOUT}),
            expected: json!([bucket]),
        },
    ];

    let gamelift = GameLift::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let name = format!("log-row-{index}");
        let carried = case
            .carried
            .as_array()
            .expect("the carried destinations are a JSON list")
            .clone();
        let stored = create_queue_logging_to(&gamelift, &name, &carried).await;

        let expected_destinations = case
            .expected
            .as_array()
            .expect("the expected destinations are a JSON list");
        assert!(
            expected_destinations.len() <= MAX_LOG_DESTINATIONS,
            "case {label:?}: the row expects {} destinations, past the bound of \
             {MAX_LOG_DESTINATIONS}",
            expected_destinations.len()
        );
        let changes = if expected_destinations.is_empty() {
            json!({"LogConfiguration": null})
        } else {
            json!({"LogConfiguration": {"DeliveryDestinations": case.expected.clone()}})
        };
        let mut expected = with_changes(&stored, &changes);
        if let Some(timeout) = case.update["TimeoutInSeconds"].as_i64() {
            expected["TimeoutInSeconds"] = json!(timeout);
        }

        let mut request = case.update.clone();
        request["Name"] = json!(&name);
        let updated = gamelift.ok("UpdateGameSessionQueue", request.clone()).await;
        assert_eq!(
            updated["GameSessionQueue"], expected,
            "case {label:?}: the update must report the composed configuration, got {}",
            updated["GameSessionQueue"]
        );

        // The describe path serves the SAME stored record, so the composition persisted.
        let described = describe_queues(&gamelift, json!({"Names": [&name]})).await;
        assert_eq!(
            described,
            json!([expected.clone()]),
            "case {label:?}: the describe must report the composed configuration, got {described}"
        );

        // Applying the same request again composes the same configuration: adding a
        // destination the queue already delivers to, and removing one it does not, both
        // leave the configuration as it is.
        let repeated = gamelift.ok("UpdateGameSessionQueue", request).await;
        assert_eq!(
            repeated["GameSessionQueue"], expected,
            "case {label:?}: repeating the update must report the same configuration, got {}",
            repeated["GameSessionQueue"]
        );
    }
}

/// An update whose composed log configuration breaks a documented rule is refused with
/// the modelled client error, and the queue keeps delivering exactly where it did.
#[tokio::test]
async fn an_update_whose_log_configuration_is_refused_changes_nothing() {
    struct Case {
        label: &'static str,
        /// The destinations the queue is created delivering to.
        carried: Value,
        /// The update, without its `Name`.
        update: Value,
    }

    let cloudwatch = log_destination(LOG_DESTINATION_TYPES[0], None);

    let cases = [
        Case {
            label: "one destination past the bound on the configuration",
            carried: every_log_destination(),
            update: json!({
                "AddLogDeliveryDestinations": [
                    log_destination(LOG_DESTINATION_TYPES[1], Some("one-too-many")),
                ],
            }),
        },
        Case {
            label: "adding a destination of the type that must name its resource, naming none",
            carried: json!([cloudwatch]),
            update: json!({
                "AddLogDeliveryDestinations": [log_destination(LOG_DESTINATION_TYPES[2], None)],
            }),
        },
        Case {
            label: "adding two destinations that deliver to the same resource",
            carried: json!([]),
            update: json!({
                "AddLogDeliveryDestinations": [
                    log_destination(LOG_DESTINATION_TYPES[0], Some("shared")),
                    log_destination(LOG_DESTINATION_TYPES[1], Some("shared")),
                ],
            }),
        },
        Case {
            label: "adding one past the bound while removing a destination it does not carry",
            carried: every_log_destination(),
            update: json!({
                "RemoveLogDeliveryDestinations": [
                    log_destination(LOG_DESTINATION_TYPES[1], Some("never-carried")),
                ],
                "AddLogDeliveryDestinations": [
                    log_destination(LOG_DESTINATION_TYPES[1], Some("one-too-many")),
                ],
            }),
        },
    ];

    let gamelift = GameLift::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let name = format!("refused-log-row-{index}");
        let carried = case
            .carried
            .as_array()
            .expect("the carried destinations are a JSON list")
            .clone();
        let stored = create_queue_logging_to(&gamelift, &name, &carried).await;

        let mut request = case.update.clone();
        request["Name"] = json!(&name);
        let (status, code) = gamelift.err("UpdateGameSessionQueue", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: the update must be refused, got {status} {code}"
        );

        let described = describe_queues(&gamelift, json!({"Names": [&name]})).await;
        assert_eq!(
            described,
            json!([stored.clone()]),
            "case {label:?}: a refused update must leave the queue as it was, got {described}"
        );
    }
}
