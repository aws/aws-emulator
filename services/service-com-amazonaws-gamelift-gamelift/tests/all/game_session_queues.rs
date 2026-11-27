// SPDX-License-Identifier: CC0-1.0

//! The game session queue contract, over the wire: `CreateGameSessionQueue` persists the
//! placement queue the request configures, decides the members the request left unset,
//! and `DescribeGameSessionQueues` serves that one stored record back — selected by name
//! or ARN, and paged.
//!
//! The harness (`GameLift`, `REGION`, `gamelift_arn`, `ec2_fleet`, `create_fleet`) is the
//! one `hosting_loop` already drives the production dispatch path with.

use serde_json::{json, Value};

use super::hosting_loop::{create_fleet, ec2_fleet, gamelift_arn, GameLift};

/// The documented bounds of a queue's `TimeoutInSeconds`
/// (`smithy_query doc shape=CreateGameSessionQueueInput member=TimeoutInSeconds`: "The
/// minimum value is 10 and the maximum value is 600."), and the value a queue created
/// without the parameter carries.
pub(crate) const MIN_TIMEOUT: i64 = 10;
/// See [`MIN_TIMEOUT`].
pub(crate) const MAX_TIMEOUT: i64 = 600;

/// Every `PriorityType` the model declares (`smithy_query shape=PriorityType`), so the
/// prioritization rows below are drawn from the modelled catalog rather than from one
/// example.
pub(crate) const PRIORITY_TYPES: [&str; 4] = ["LATENCY", "COST", "DESTINATION", "LOCATION"];

/// Every `LogDeliveryDestinationType` the model declares (`smithy_query
/// shape=LogDeliveryDestinationType`), so the log-destination rows below are drawn from
/// the modelled catalog rather than from one example. `FIREHOSE` is the one that must
/// name the resource it delivers to
/// (`smithy_query doc shape=LogDeliveryDestination member=DestinationArn`: "This value is
/// required for FIREHOSE destinations. For CLOUDWATCH and S3, you can omit this value").
pub(crate) const LOG_DESTINATION_TYPES: [&str; 3] = ["CLOUDWATCH", "S3", "FIREHOSE"];

/// How many destinations one queue's log configuration may deliver to, read from the
/// constant the handlers enforce — the modelled length of
/// `GameSessionQueueLogConfiguration$DeliveryDestinations` and its doc: "You can specify
/// up to 3 destinations."
pub(crate) const MAX_LOG_DESTINATIONS: usize =
    service_com_amazonaws_gamelift_gamelift::state::MAX_QUEUE_LOG_DELIVERY_DESTINATIONS;

/// A log delivery destination of type `log_destination`, delivering to the resource
/// `arn_suffix` names when one is given and to a resource of its own type when none is.
pub(crate) fn log_destination(log_destination: &str, arn_suffix: Option<&str>) -> Value {
    match arn_suffix {
        Some(suffix) => json!({
            "LogDestination": log_destination,
            "DestinationArn": format!("arn:aws:logs:us-west-2:123456789012:{suffix}"),
        }),
        None => json!({"LogDestination": log_destination}),
    }
}

/// One destination of every modelled type ([`LOG_DESTINATION_TYPES`]), which is also the
/// most destinations one queue's log configuration may hold
/// ([`MAX_LOG_DESTINATIONS`]) — the `FIREHOSE` one naming its delivery stream, as that
/// type requires, and the other two leaving the resource to the service.
pub(crate) fn every_log_destination() -> Value {
    json!(LOG_DESTINATION_TYPES
        .iter()
        .map(|destination_type| {
            let arn_suffix = (*destination_type == "FIREHOSE").then_some("stream/placements");
            log_destination(destination_type, arn_suffix)
        })
        .collect::<Vec<_>>())
}

/// The ARN a queue named `name` is assigned:
/// `arn:aws:gamelift:<region>:<account>:gamesessionqueue/<queue name>`.
pub(crate) fn queue_arn(name: &str) -> String {
    gamelift_arn(&format!("gamesessionqueue/{name}"))
}

/// The queues `DescribeGameSessionQueues` reports for `request`.
pub(crate) async fn describe_queues(gamelift: &GameLift, request: Value) -> Value {
    let described = gamelift.ok("DescribeGameSessionQueues", request).await;
    described["GameSessionQueues"].clone()
}

/// A destination list naming `count` fleet ARNs, in placement-preference order.
pub(crate) fn destinations(count: usize) -> Value {
    json!((0..count)
        .map(|index| json!({"DestinationArn": gamelift_arn(&format!("fleet/fleet-dest-{index}"))}))
        .collect::<Vec<_>>())
}

/// The bounds this emulator puts on the two queue list members the model leaves
/// unbounded, read from the constants the handler enforces.
const MAX_DESTINATIONS: usize =
    service_com_amazonaws_gamelift_gamelift::state::MAX_QUEUE_DESTINATIONS;
/// See [`MAX_DESTINATIONS`].
const MAX_LATENCY_POLICIES: usize =
    service_com_amazonaws_gamelift_gamelift::state::MAX_QUEUE_PLAYER_LATENCY_POLICIES;
/// See [`MAX_DESTINATIONS`].
const MAX_NAMES_PER_DESCRIBE: usize =
    service_com_amazonaws_gamelift_gamelift::state::MAX_QUEUE_NAMES_PER_REQUEST;

/// A latency policy list of `count` policies, each capping latency more loosely than the
/// one before it: "policies are evaluated in order starting with the lowest maximum
/// latency value".
pub(crate) fn latency_policies(count: usize) -> Value {
    json!((0..count)
        .map(|index| json!({
            "MaximumIndividualPlayerLatencyMilliseconds": 50 + 10 * index,
            "PolicyDurationSeconds": 30,
        }))
        .collect::<Vec<_>>())
}

/// `CreateGameSessionQueue` stores the queue the request configures and reports the
/// stored record; `DescribeGameSessionQueues` serves the same record back, member for
/// member.
///
/// The rows differ along what the rule generalises over: which members the request sets,
/// the SIZE of each list member (none, one, several), the timeout at each documented
/// bound, and which modelled `PriorityType` values the prioritization names.
#[tokio::test]
async fn a_created_queue_is_reported_as_stored_by_create_and_by_describe() {
    struct Case {
        label: &'static str,
        /// The request, without its `Name` — each row is created under its own name.
        request: Value,
        /// The members the created queue must report beyond its name and ARN.
        expected: Value,
    }

    let cases = vec![
        Case {
            label: "nothing but the required name",
            request: json!({}),
            expected: json!({
                "TimeoutInSeconds": MAX_TIMEOUT,
                "Destinations": [],
                "PlayerLatencyPolicies": [],
            }),
        },
        Case {
            label: "one destination",
            request: json!({"Destinations": destinations(1)}),
            expected: json!({
                "TimeoutInSeconds": MAX_TIMEOUT,
                "Destinations": destinations(1),
                "PlayerLatencyPolicies": [],
            }),
        },
        Case {
            label: "three destinations, in preference order",
            request: json!({"Destinations": destinations(3)}),
            expected: json!({
                "TimeoutInSeconds": MAX_TIMEOUT,
                "Destinations": destinations(3),
                "PlayerLatencyPolicies": [],
            }),
        },
        Case {
            label: "an explicitly empty destination list",
            request: json!({"Destinations": []}),
            expected: json!({
                "TimeoutInSeconds": MAX_TIMEOUT,
                "Destinations": [],
                "PlayerLatencyPolicies": [],
            }),
        },
        Case {
            label: "the minimum documented timeout",
            request: json!({"TimeoutInSeconds": MIN_TIMEOUT}),
            expected: json!({
                "TimeoutInSeconds": MIN_TIMEOUT,
                "Destinations": [],
                "PlayerLatencyPolicies": [],
            }),
        },
        Case {
            label: "the maximum documented timeout",
            request: json!({"TimeoutInSeconds": MAX_TIMEOUT}),
            expected: json!({
                "TimeoutInSeconds": MAX_TIMEOUT,
                "Destinations": [],
                "PlayerLatencyPolicies": [],
            }),
        },
        Case {
            label: "a timeout inside the bounds",
            request: json!({"TimeoutInSeconds": 120}),
            expected: json!({
                "TimeoutInSeconds": 120,
                "Destinations": [],
                "PlayerLatencyPolicies": [],
            }),
        },
        Case {
            label: "two player latency policies, relaxing over time",
            request: json!({
                "PlayerLatencyPolicies": [
                    {
                        "MaximumIndividualPlayerLatencyMilliseconds": 100,
                        "PolicyDurationSeconds": 60,
                    },
                    {"MaximumIndividualPlayerLatencyMilliseconds": 200},
                ],
            }),
            expected: json!({
                "TimeoutInSeconds": MAX_TIMEOUT,
                "Destinations": [],
                "PlayerLatencyPolicies": [
                    {
                        "MaximumIndividualPlayerLatencyMilliseconds": 100,
                        "PolicyDurationSeconds": 60,
                    },
                    {"MaximumIndividualPlayerLatencyMilliseconds": 200},
                ],
            }),
        },
        Case {
            label: "a filter configuration naming two locations",
            request: json!({
                "FilterConfiguration": {"AllowedLocations": ["us-west-2", "eu-central-1"]},
            }),
            expected: json!({
                "TimeoutInSeconds": MAX_TIMEOUT,
                "Destinations": [],
                "PlayerLatencyPolicies": [],
                "FilterConfiguration": {"AllowedLocations": ["us-west-2", "eu-central-1"]},
            }),
        },
        Case {
            label: "every modelled priority type, with a location order",
            request: json!({
                "PriorityConfiguration": {
                    "PriorityOrder": PRIORITY_TYPES,
                    "LocationOrder": ["us-west-2", "ap-south-1"],
                },
            }),
            expected: json!({
                "TimeoutInSeconds": MAX_TIMEOUT,
                "Destinations": [],
                "PlayerLatencyPolicies": [],
                "PriorityConfiguration": {
                    "PriorityOrder": PRIORITY_TYPES,
                    "LocationOrder": ["us-west-2", "ap-south-1"],
                },
            }),
        },
        Case {
            label: "a single priority type the issue did not cite",
            request: json!({"PriorityConfiguration": {"PriorityOrder": ["COST"]}}),
            expected: json!({
                "TimeoutInSeconds": MAX_TIMEOUT,
                "Destinations": [],
                "PlayerLatencyPolicies": [],
                "PriorityConfiguration": {"PriorityOrder": ["COST"]},
            }),
        },
        Case {
            label: "placement notification and custom event data",
            request: json!({
                "NotificationTarget": "arn:aws:sns:us-west-2:123456789012:placements",
                "CustomEventData": "queue-event-data",
                "Destinations": destinations(2),
                "TimeoutInSeconds": 300,
                "Tags": [{"Key": "owner", "Value": "liveops"}],
            }),
            expected: json!({
                "TimeoutInSeconds": 300,
                "Destinations": destinations(2),
                "PlayerLatencyPolicies": [],
                "NotificationTarget": "arn:aws:sns:us-west-2:123456789012:placements",
                "CustomEventData": "queue-event-data",
            }),
        },
        Case {
            label: "one log destination the service resolves the resource for",
            request: json!({
                "LogConfiguration": {
                    "DeliveryDestinations": [log_destination(LOG_DESTINATION_TYPES[0], None)],
                },
            }),
            expected: json!({
                "TimeoutInSeconds": MAX_TIMEOUT,
                "Destinations": [],
                "PlayerLatencyPolicies": [],
                "LogConfiguration": {
                    "DeliveryDestinations": [log_destination(LOG_DESTINATION_TYPES[0], None)],
                },
            }),
        },
        Case {
            label: "one log destination naming the resource it delivers to",
            request: json!({
                "LogConfiguration": {
                    "DeliveryDestinations": [
                        log_destination(LOG_DESTINATION_TYPES[1], Some("one-bucket")),
                    ],
                },
            }),
            expected: json!({
                "TimeoutInSeconds": MAX_TIMEOUT,
                "Destinations": [],
                "PlayerLatencyPolicies": [],
                "LogConfiguration": {
                    "DeliveryDestinations": [
                        log_destination(LOG_DESTINATION_TYPES[1], Some("one-bucket")),
                    ],
                },
            }),
        },
        Case {
            label: "every modelled log destination type, at the bound on the configuration",
            request: json!({
                "LogConfiguration": {"DeliveryDestinations": every_log_destination()},
            }),
            expected: json!({
                "TimeoutInSeconds": MAX_TIMEOUT,
                "Destinations": [],
                "PlayerLatencyPolicies": [],
                "LogConfiguration": {"DeliveryDestinations": every_log_destination()},
            }),
        },
    ];
    let gamelift = GameLift::new();
    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        let name = format!("queue-row-{index}");
        let mut request = case.request.clone();
        request["Name"] = json!(&name);

        let mut expected = case.expected.clone();
        expected["Name"] = json!(&name);
        expected["GameSessionQueueArn"] = json!(queue_arn(&name));

        let created = gamelift.ok("CreateGameSessionQueue", request).await;
        assert_eq!(
            created["GameSessionQueue"], expected,
            "case {label:?}: CreateGameSessionQueue must report the stored queue, got {}",
            created["GameSessionQueue"]
        );

        // The describe path serves the SAME stored record, member for member — selected
        // by name and by the ARN the create minted, which name the same queue.
        for selector in [name.clone(), queue_arn(&name)] {
            let described = describe_queues(&gamelift, json!({"Names": [&selector]})).await;
            assert_eq!(
                described,
                json!([expected.clone()]),
                "case {label:?}: a describe naming {selector:?} must report the created record, got {described}"
            );
        }

        // And so does the unfiltered describe, which reports every queue of the account.
        let listed = describe_queues(&gamelift, json!({})).await;
        let from_list = listed
            .as_array()
            .expect("DescribeGameSessionQueues must report a list")
            .iter()
            .find(|queue| queue["Name"] == json!(&name))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            from_list, expected,
            "case {label:?}: the unfiltered describe must report the same record, got {from_list}"
        );
    }

    // Reading the queues changed nothing: the same unfiltered describe, run again,
    // reports exactly the same collection.
    let first = describe_queues(&gamelift, json!({})).await;
    let second = describe_queues(&gamelift, json!({})).await;
    assert_eq!(
        first, second,
        "describing the queues must not change what a later describe reports, got {second}"
    );
    assert_eq!(
        first.as_array().map(Vec::len),
        Some(cases.len()),
        "every created queue must be reported once, got {first}"
    );
}

/// A create that breaks a documented rule is refused with the modelled client error, and
/// leaves the stored collection as it was.
#[tokio::test]
async fn a_create_that_breaks_a_documented_rule_is_refused() {
    struct Case {
        label: &'static str,
        request: Value,
        code: &'static str,
    }

    let cases = vec![
        Case {
            label: "a timeout one below the documented minimum",
            request: json!({"Name": "below-minimum", "TimeoutInSeconds": MIN_TIMEOUT - 1}),
            code: "InvalidRequestException",
        },
        Case {
            label: "a timeout of zero, which the model allows and the service does not",
            request: json!({"Name": "zero-timeout", "TimeoutInSeconds": 0}),
            code: "InvalidRequestException",
        },
        Case {
            label: "a timeout one above the documented maximum",
            request: json!({"Name": "above-maximum", "TimeoutInSeconds": MAX_TIMEOUT + 1}),
            code: "InvalidRequestException",
        },
        Case {
            label: "a priority type listed twice",
            request: json!({
                "Name": "repeated-priority",
                "PriorityConfiguration": {"PriorityOrder": ["COST", "COST"]},
            }),
            code: "InvalidRequestException",
        },
        Case {
            label: "a location listed twice",
            request: json!({
                "Name": "repeated-location",
                "PriorityConfiguration": {
                    "PriorityOrder": ["LOCATION"],
                    "LocationOrder": ["us-west-2", "us-west-2"],
                },
            }),
            code: "InvalidRequestException",
        },
        Case {
            label: "one destination past the bound on a queue's destinations",
            request: json!({
                "Name": "too-many-destinations",
                "Destinations": destinations(MAX_DESTINATIONS + 1),
            }),
            code: "InvalidRequestException",
        },
        Case {
            label: "one policy past the bound on a queue's latency policies",
            request: json!({
                "Name": "too-many-policies",
                "PlayerLatencyPolicies": latency_policies(MAX_LATENCY_POLICIES + 1),
            }),
            code: "InvalidRequestException",
        },
        Case {
            label: "a log destination of the type that must name its resource, naming none",
            request: json!({
                "Name": "firehose-without-arn",
                "LogConfiguration": {
                    "DeliveryDestinations": [log_destination("FIREHOSE", None)],
                },
            }),
            code: "InvalidRequestException",
        },
        Case {
            label: "two log destinations delivering to the same resource",
            request: json!({
                "Name": "repeated-destination-arn",
                "LogConfiguration": {
                    "DeliveryDestinations": [
                        log_destination(LOG_DESTINATION_TYPES[0], Some("shared")),
                        log_destination(LOG_DESTINATION_TYPES[1], Some("shared")),
                    ],
                },
            }),
            code: "InvalidRequestException",
        },
    ];

    let gamelift = GameLift::new();
    for case in cases {
        let label = case.label;
        let (status, code) = gamelift
            .err("CreateGameSessionQueue", case.request.clone())
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, case.code),
            "case {label:?}: the request must be refused with {}, got {status} {code}",
            case.code
        );
        let listed = describe_queues(&gamelift, json!({})).await;
        assert_eq!(
            listed,
            json!([]),
            "case {label:?}: a refused create must store nothing, got {listed}"
        );
    }
}

/// A request exactly AT each caller-sized bound is served, and one entry past it is
/// refused: the bounds are the service's, not the request's.
#[tokio::test]
async fn a_request_at_each_bound_is_served_and_one_past_it_is_refused() {
    let gamelift = GameLift::new();

    // At the bound on a queue's destinations and latency policies.
    let created = gamelift
        .ok(
            "CreateGameSessionQueue",
            json!({
                "Name": "at-the-bounds",
                "Destinations": destinations(MAX_DESTINATIONS),
                "PlayerLatencyPolicies": latency_policies(MAX_LATENCY_POLICIES),
            }),
        )
        .await;
    assert_eq!(
        created["GameSessionQueue"]["Destinations"]
            .as_array()
            .map(Vec::len),
        Some(MAX_DESTINATIONS),
        "a queue at the destination bound must be stored whole, got {}",
        created["GameSessionQueue"]["Destinations"]
    );
    assert_eq!(
        created["GameSessionQueue"]["PlayerLatencyPolicies"]
            .as_array()
            .map(Vec::len),
        Some(MAX_LATENCY_POLICIES),
        "a queue at the latency-policy bound must be stored whole, got {}",
        created["GameSessionQueue"]["PlayerLatencyPolicies"]
    );

    // At the bound on how many queue identifiers one describe may name, and one past it.
    let names: Vec<String> = (0..MAX_NAMES_PER_DESCRIBE)
        .map(|index| format!("named-{index}"))
        .collect();
    let described = describe_queues(&gamelift, json!({"Names": names.clone()})).await;
    assert_eq!(
        described,
        json!([]),
        "a describe naming the maximum number of identifiers must be served, got {described}"
    );
    let mut too_many = names.clone();
    too_many.push("named-one-too-many".to_string());
    let (status, code) = gamelift
        .err("DescribeGameSessionQueues", json!({"Names": too_many}))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "a describe naming more identifiers than the maximum must be refused, got {status} {code}"
    );
}

/// "Queue names must be unique within each Region": a second create under a name that is
/// taken is refused, and the queue that holds the name is untouched.
#[tokio::test]
async fn a_queue_name_cannot_be_taken_twice() {
    let gamelift = GameLift::new();
    let first = gamelift
        .ok(
            "CreateGameSessionQueue",
            json!({
                "Name": "unique-name",
                "TimeoutInSeconds": 120,
                "Destinations": destinations(1),
            }),
        )
        .await;

    let (status, code) = gamelift
        .err(
            "CreateGameSessionQueue",
            json!({"Name": "unique-name", "TimeoutInSeconds": 300}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "a repeated queue name must be refused: got {status} {code}"
    );

    let described = describe_queues(&gamelift, json!({"Names": ["unique-name"]})).await;
    assert_eq!(
        described,
        json!([first["GameSessionQueue"].clone()]),
        "the refused create must leave the queue holding the name untouched, got {described}"
    );

    // A different name is free, so the refusal is about the name and nothing else.
    let other = gamelift
        .ok(
            "CreateGameSessionQueue",
            json!({"Name": "unique-name-2", "TimeoutInSeconds": 300}),
        )
        .await;
    assert_eq!(
        other["GameSessionQueue"]["TimeoutInSeconds"],
        json!(300),
        "a queue under a free name must be created, got {}",
        other["GameSessionQueue"]
    );
}

/// `DescribeGameSessionQueues` pages the collection: a `Limit` bounds the page, the
/// `NextToken` it hands back resumes exactly where that page ended, and the last page
/// carries no token. A token this collection did not issue is refused.
#[tokio::test]
async fn describe_pages_the_collection_and_refuses_a_foreign_token() {
    let gamelift = GameLift::new();
    let names: Vec<String> = (0..5).map(|index| format!("paged-{index}")).collect();
    for name in &names {
        gamelift
            .ok("CreateGameSessionQueue", json!({"Name": name}))
            .await;
    }

    // Every page size from one queue at a time up to more than the collection holds
    // walks the same collection, in the same order, exactly once.
    for page_size in [1_usize, 2, 5, 7] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"Limit": page_size});
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let response = gamelift
                .ok("DescribeGameSessionQueues", request.clone())
                .await;
            let page = response["GameSessionQueues"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page must hold at most Limit queues, got {}",
                page.len()
            );
            walked.extend(
                page.iter()
                    .map(|queue| queue["Name"].as_str().unwrap_or_default().to_string()),
            );
            match response["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, names,
            "case page size {page_size}: the paged walk must report every queue once, in order, got {walked:?}"
        );
    }

    // A token minted for another collection is not an offset into this one.
    let fleet_one = create_fleet(&gamelift, ec2_fleet("token-fleet-one")).await;
    let fleet_two = create_fleet(&gamelift, ec2_fleet("token-fleet-two")).await;
    assert_ne!(
        fleet_one, fleet_two,
        "the two fleets of the token case must be distinct, got {fleet_one} and {fleet_two}"
    );
    let fleet_page = gamelift
        .ok("DescribeFleetAttributes", json!({"Limit": 1}))
        .await;
    let fleet_token = fleet_page["NextToken"]
        .as_str()
        .expect("a paged DescribeFleetAttributes must hand back a token")
        .to_string();

    for token in [fleet_token.as_str(), "not-a-token-this-service-issued"] {
        let (status, code) = gamelift
            .err("DescribeGameSessionQueues", json!({"NextToken": token}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case token {token:?}: a token this collection did not issue must be refused, got {status} {code}"
        );
    }

    // `Limit`'s doc: "You can request up to 50 results."
    let largest = gamelift
        .ok("DescribeGameSessionQueues", json!({"Limit": 50}))
        .await;
    assert_eq!(
        largest["GameSessionQueues"].as_array().map(Vec::len),
        Some(names.len()),
        "a Limit at the documented maximum must be served, got {largest}"
    );
    let (status, code) = gamelift
        .err("DescribeGameSessionQueues", json!({"Limit": 51}))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "a Limit above the documented maximum must be refused, got {status} {code}"
    );
}

/// "Objects are returned only for queues that currently exist in the Region": a name that
/// names no stored queue selects nothing, and the request still succeeds.
#[tokio::test]
async fn a_describe_that_names_no_stored_queue_reports_an_empty_page() {
    let gamelift = GameLift::new();
    gamelift
        .ok("CreateGameSessionQueue", json!({"Name": "the-only-queue"}))
        .await;

    let cases = vec![
        ("a name no queue carries", json!(["no-such-queue"])),
        ("an ARN no queue carries", json!([queue_arn("no-such-queue")])),
        (
            "several names, none of them stored",
            json!(["absent-one", "absent-two"]),
        ),
    ];
    for (label, requested) in cases {
        let described = describe_queues(&gamelift, json!({"Names": requested})).await;
        assert_eq!(
            described,
            json!([]),
            "case {label:?}: a describe that selects no stored queue must report an empty page, got {described}"
        );
    }

    // Naming the same queue twice — by name and by ARN — reports it once: the page holds
    // one object per distinct QUEUE, not one per entry.
    let described = describe_queues(
        &gamelift,
        json!({"Names": ["the-only-queue", queue_arn("the-only-queue"), "absent"]}),
    )
    .await;
    assert_eq!(
        described.as_array().map(Vec::len),
        Some(1),
        "a queue named twice must be reported once, got {described}"
    );
}

/// The account's queue quota bounds the stored collection: once it is reached, the next
/// create answers the modelled `LimitExceededException` and stores nothing.
#[tokio::test]
async fn the_queue_quota_is_the_bound_on_the_stored_collection() {
    let quota = service_com_amazonaws_gamelift_gamelift::state::MAX_GAME_SESSION_QUEUES_PER_ACCOUNT;
    let gamelift = GameLift::new();
    for index in 0..quota {
        gamelift
            .ok(
                "CreateGameSessionQueue",
                json!({"Name": format!("quota-{index}")}),
            )
            .await;
    }

    let (status, code) = gamelift
        .err(
            "CreateGameSessionQueue",
            json!({"Name": "one-queue-too-many"}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededException"),
        "a create past the quota must be refused: got {status} {code}"
    );

    let listed = describe_queues(&gamelift, json!({"Limit": 50})).await;
    assert_eq!(
        listed.as_array().map(Vec::len),
        Some(quota),
        "the refused create must store nothing, got {} queues",
        listed.as_array().map_or(0, Vec::len)
    );
}
