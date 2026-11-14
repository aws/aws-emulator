// SPDX-License-Identifier: CC0-1.0

//! Deleting the GameLift resources whose `Delete*` operation names ONE record, over the
//! wire: a build, a game session queue, a custom location, a FlexMatch matchmaking
//! configuration and a FlexMatch rule set.
//!
//! The one rule every row below asserts: a successful `Delete*` takes the named record OUT
//! of the store it was read from, so every later read of that record answers the modelled
//! not-found refusal (or, where the read reports a collection, no longer carries it), the
//! records the request did not name are untouched, and the account's collection of that
//! resource is one shorter. An identifier that resolves to no stored record is refused and
//! removes nothing.
//!
//! `alias_deletion.rs` and `script_deletion.rs` assert the same rule for the two resources
//! whose deletes were already implemented; this suite covers the rest of the family.
//!
//! The harness (`GameLift`, `error_code`, `error_message`, `gamelift_arn`, `REGION`) is the
//! one `hosting_loop.rs` defines, and the queue helpers are the ones
//! `game_session_queues.rs` already drives `CreateGameSessionQueue` with.

use serde_json::{json, Value};

use crate::game_session_queues::{destinations, latency_policies, queue_arn, MAX_TIMEOUT};
use crate::hosting_loop::{error_code, error_message, gamelift_arn, GameLift, REGION};

/// Every `OperatingSystem` a build may declare, from the model's own enum
/// (`smithy_query shape=OperatingSystem`), so the build rows are drawn from the modelled
/// catalog rather than from one example.
const OPERATING_SYSTEMS: [&str; 6] = [
    "WINDOWS_2012",
    "AMAZON_LINUX",
    "AMAZON_LINUX_2",
    "WINDOWS_2016",
    "AMAZON_LINUX_2023",
    "WINDOWS_2022",
];

/// The ARN a build with `build_id` is assigned: `Build$BuildArn`'s "the resource ID matches
/// the `BuildId` value".
fn build_arn(build_id: &str) -> String {
    gamelift_arn(&format!("build/{build_id}"))
}

/// The ARN a custom location named `name` is assigned: `LocationModel$LocationArn`'s
/// modelled pattern is `^arn:.*:location\/custom-\S+$`, whose resource is the location's
/// own name.
fn location_arn(name: &str) -> String {
    gamelift_arn(&format!("location/{name}"))
}

/// The ARN a rule set named `name` is assigned: `MatchmakingRuleSet$RuleSetArn`'s doc —
/// "Format is `arn:aws:gamelift:<region>::matchmakingruleset/<ruleset name>`".
fn rule_set_arn(name: &str) -> String {
    gamelift_arn(&format!("matchmakingruleset/{name}"))
}

/// The ARN a matchmaking configuration named `name` is assigned:
/// `MatchmakingConfiguration$ConfigurationArn`'s doc — "Format is
/// `arn:aws:gamelift:<region>::matchmakingconfiguration/<matchmaking configuration name>`".
fn configuration_arn(name: &str) -> String {
    gamelift_arn(&format!("matchmakingconfiguration/{name}"))
}

/// A rule set body that conforms to the FlexMatch rule language: the version it is written
/// in, and the teams a match is built from.
fn conformant_body(name: &str) -> String {
    json!({
        "name": name,
        "ruleLanguageVersion": "1.0",
        "teams": [{"name": "solo", "minPlayers": 1, "maxPlayers": 2}],
    })
    .to_string()
}

/// `count` tags, each carrying a distinct key, as `Tag` models them.
fn tag_list(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"Key": format!("key-{index}"), "Value": format!("value-{index}")}))
            .collect(),
    )
}

/// Assert `operation` naming `request` is refused with HTTP 400, the wire error code `code`
/// and exactly `expected` as its `Message`.
pub(crate) async fn assert_refused(
    gamelift: &GameLift,
    operation: &str,
    request: Value,
    code: &str,
    expected: &str,
    case: &str,
) {
    let (status, body) = gamelift.call(operation, &request).await;
    let got = format!("{}|{}", error_code(&body), error_message(&body));
    let wanted = format!("{code}|{expected}");
    assert_eq!(
        (status, got.as_str()),
        (400, wanted.as_str()),
        "case {case:?} on {operation}: expected HTTP 400 with {wanted:?}, got HTTP {status} with \
         {got:?}"
    );
}

// ---------------------------------------------------------------------------------------
// DeleteBuild
// ---------------------------------------------------------------------------------------

/// `DeleteBuild` removes the one build its identifier resolves to: the delete succeeds with
/// an empty body, every later read of that build answers the modelled `NotFoundException`
/// carrying the identifier the request asked for, and the other builds in the store still
/// read back exactly as they did before.
///
/// The rows differ along what the rule generalises over: which identifier form names the
/// build (id or ARN), how many builds the store holds, which of them the request deletes
/// (the only one, the first, a middle one, the last), which modelled `OperatingSystem` it
/// declares, which upload path it took (the service-owned location or the caller's own
/// bucket), how many tags it carries, and whether `UpdateBuild` revised it first.
#[tokio::test]
async fn deleting_a_build_removes_the_one_build_the_identifier_resolves_to() {
    struct Case {
        label: &'static str,
        /// How many builds the store holds when the delete runs.
        store: usize,
        /// The index, within that store, of the build the request deletes.
        target: usize,
        /// Whether the request names the target by its ARN rather than by its id.
        by_arn: bool,
        /// Whether the target's files live in the CALLER's own bucket rather than at the
        /// service-owned location a create without `StorageLocation` is handed.
        caller_bucket: bool,
        /// How many tags the target carries.
        tags: usize,
        /// Whether `UpdateBuild` revised the target before the delete.
        updated: bool,
    }

    let cases = [
        Case {
            label: "the only build in the store, named by its id",
            store: 1,
            target: 0,
            by_arn: false,
            caller_bucket: false,
            tags: 0,
            updated: false,
        },
        Case {
            label: "the only build in the store, named by its ARN",
            store: 1,
            target: 0,
            by_arn: true,
            caller_bucket: false,
            tags: 0,
            updated: false,
        },
        Case {
            label: "the first of three builds, named by its id",
            store: 3,
            target: 0,
            by_arn: false,
            caller_bucket: false,
            tags: 1,
            updated: false,
        },
        Case {
            label: "a middle build of five, named by its ARN",
            store: 5,
            target: 2,
            by_arn: true,
            caller_bucket: true,
            tags: 0,
            updated: false,
        },
        Case {
            label: "the last of four builds, named by its id",
            store: 4,
            target: 3,
            by_arn: false,
            caller_bucket: true,
            tags: 3,
            updated: false,
        },
        Case {
            label: "a build updated before the delete, named by its ARN",
            store: 2,
            target: 1,
            by_arn: true,
            caller_bucket: false,
            tags: 2,
            updated: true,
        },
    ];

    for case in &cases {
        let label = case.label;
        // Each row gets its own store, so what survives the delete is only what this row
        // created.
        let gamelift = GameLift::new();
        let mut builds: Vec<Value> = Vec::new();
        for index in 0..case.store {
            let is_target = index == case.target;
            let mut request = json!({
                "Name": format!("deletable-{index}"),
                "Version": format!("1.{index}"),
                "OperatingSystem": OPERATING_SYSTEMS[index % OPERATING_SYSTEMS.len()],
            });
            if is_target && case.caller_bucket {
                request["StorageLocation"] = json!({
                    "Bucket": "my-own-game-builds",
                    "Key": format!("builds/{index}.zip"),
                    "RoleArn": "arn:aws:iam::123456789012:role/GameLiftBuildAccess",
                });
            }
            if is_target && case.tags > 0 {
                request["Tags"] = tag_list(case.tags);
            }
            let created = gamelift.ok("CreateBuild", request).await;
            builds.push(created["Build"].clone());
        }

        if case.updated {
            let target_id = builds[case.target]["BuildId"]
                .as_str()
                .expect("CreateBuild must report a BuildId")
                .to_string();
            let updated = gamelift
                .ok(
                    "UpdateBuild",
                    json!({"BuildId": &target_id, "Name": "revised-before-deletion"}),
                )
                .await;
            builds[case.target] = updated["Build"].clone();
        }

        let target_id = builds[case.target]["BuildId"]
            .as_str()
            .expect("CreateBuild must report a BuildId")
            .to_string();
        let target_arn = build_arn(&target_id);
        let selector = if case.by_arn {
            target_arn.clone()
        } else {
            target_id.clone()
        };

        // Every build reads back as stored before the delete, so what changes below is the
        // delete's doing and nothing else.
        for (index, build) in builds.iter().enumerate() {
            let id = build["BuildId"].as_str().unwrap_or_default();
            let described = gamelift.ok("DescribeBuild", json!({"BuildId": id})).await;
            assert_eq!(
                &described["Build"], build,
                "case {label:?}: build {index} must read back as created, got {}",
                described["Build"]
            );
        }

        let (status, body) = gamelift
            .call("DeleteBuild", &json!({"BuildId": &selector}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting the stored build must succeed, got HTTP {status} with body \
             {body}"
        );
        assert!(
            body.is_null() || body == json!({}),
            "case {label:?}: DeleteBuild reports no members, got {body}"
        );

        // The deleted record is gone from the one store every read of the build is served
        // from: both identifier forms, every operation that names ONE build, and a second
        // delete.
        for probe in [target_id.clone(), target_arn.clone()] {
            for operation in [
                "DescribeBuild",
                "UpdateBuild",
                "RequestUploadCredentials",
                "DeleteBuild",
            ] {
                let mut request = json!({"BuildId": &probe});
                if operation == "UpdateBuild" {
                    request["Name"] = json!("after-deletion");
                }
                assert_refused(
                    &gamelift,
                    operation,
                    request,
                    "NotFoundException",
                    &format!("Build ({probe}) not found"),
                    label,
                )
                .await;
            }
        }

        // Every other build of the store is untouched — member for member, by id and by
        // ARN.
        for (index, build) in builds.iter().enumerate() {
            if index == case.target {
                continue;
            }
            let survivor_id = build["BuildId"].as_str().unwrap_or_default().to_string();
            for probe in [survivor_id.clone(), build_arn(&survivor_id)] {
                let described = gamelift.ok("DescribeBuild", json!({"BuildId": probe})).await;
                assert_eq!(
                    &described["Build"], build,
                    "case {label:?}: build {index} must survive the delete unchanged, got {}",
                    described["Build"]
                );
            }
        }
    }
}

/// `DeleteBuild` acts on ONE named resource, so an identifier that resolves to no stored
/// build is the modelled `NotFoundException` rather than an empty success — and the build
/// that IS stored is left exactly as it was.
#[tokio::test]
async fn deleting_a_build_that_is_not_stored_is_refused_and_removes_nothing() {
    let gamelift = GameLift::new();
    let stored = gamelift
        .ok(
            "CreateBuild",
            json!({"Name": "the-surviving-build", "OperatingSystem": OPERATING_SYSTEMS[0]}),
        )
        .await["Build"]
        .clone();
    let stored_id = stored["BuildId"]
        .as_str()
        .expect("CreateBuild must report a BuildId")
        .to_string();

    // A build created and then deleted: its identifier is well formed and resolves to
    // nothing, exactly like one that never existed.
    let deleted_id = gamelift
        .ok(
            "CreateBuild",
            json!({"Name": "the-deleted-build", "OperatingSystem": OPERATING_SYSTEMS[1]}),
        )
        .await["Build"]["BuildId"]
        .as_str()
        .expect("CreateBuild must report a BuildId")
        .to_string();
    gamelift
        .ok("DeleteBuild", json!({"BuildId": &deleted_id}))
        .await;

    // The rows differ in the FORM of the identifier (bare id, UUID-shaped id, ARN), in
    // whether the build never existed or was deleted, and in which account and Region the
    // ARN names — each well formed against the modelled `BuildIdOrArn`
    // (`^(build-\S+|arn:.*:build\/build-\S+)$`).
    let cases: Vec<(&str, String)> = vec![
        ("an id no build carries", "build-does-not-exist".to_string()),
        (
            "a UUID-shaped id no build carries",
            "build-00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("an ARN no build carries", build_arn("build-absent-by-arn")),
        ("the id of a deleted build", deleted_id.clone()),
        ("the ARN of a deleted build", build_arn(&deleted_id)),
        (
            "an ARN in another account for a stored id",
            format!("arn:aws:gamelift:{REGION}:999999999999:build/{stored_id}"),
        ),
        (
            "an ARN in another Region for a stored id",
            format!(
                "arn:aws:gamelift:eu-central-1:{}:build/{stored_id}",
                engine::emulator_account_id()
            ),
        ),
    ];

    for (label, selector) in &cases {
        assert_refused(
            &gamelift,
            "DeleteBuild",
            json!({"BuildId": selector}),
            "NotFoundException",
            &format!("Build ({selector}) not found"),
            label,
        )
        .await;
    }

    let described = gamelift
        .ok("DescribeBuild", json!({"BuildId": &stored_id}))
        .await;
    assert_eq!(
        described["Build"], stored,
        "the stored build must survive every refused delete, got {}",
        described["Build"]
    );
}

/// "This operation permanently deletes the build resource and any uploaded build files":
/// the files uploaded to the location this service handed the caller go with the build, so
/// the object store no longer serves them — while an object in the CALLER's own bucket,
/// which this service was only given read access to, is left exactly where it is.
#[tokio::test]
async fn deleting_a_build_removes_the_files_uploaded_to_the_service_owned_location() {
    let gamelift = GameLift::new();

    // Several builds, so the rule holds for each one's own key in the shared bucket rather
    // than for one object alone, and the delete of one leaves the others' files in place.
    let mut uploads: Vec<(String, String, String, Vec<u8>)> = Vec::new();
    for (index, operating_system) in OPERATING_SYSTEMS.iter().enumerate() {
        let created = gamelift
            .ok(
                "CreateBuild",
                json!({"Name": format!("uploaded-{index}"), "OperatingSystem": operating_system}),
            )
            .await;
        let build_id = created["Build"]["BuildId"]
            .as_str()
            .expect("CreateBuild must report a BuildId")
            .to_string();
        let bucket = created["StorageLocation"]["Bucket"]
            .as_str()
            .expect("the create must report a bucket to upload to")
            .to_string();
        let key = created["StorageLocation"]["Key"]
            .as_str()
            .expect("the create must report a key to upload to")
            .to_string();

        // Deterministic bytes standing in for a zipped game server build, opening with the
        // local-file signature a zip archive carries (`PK\x03\x04`).
        let mut body: Vec<u8> = b"PK\x03\x04".to_vec();
        body.extend((0..=index as u8).cycle().take(64 + index));
        let (status, response) = gamelift.s3("PUT", &format!("/{bucket}/{key}"), &body).await;
        assert_eq!(
            status,
            200,
            "case {operating_system:?}: the upload to {bucket}/{key} must be served, got {status} \
             with body {}",
            String::from_utf8_lossy(&response)
        );
        uploads.push((build_id, bucket, key, body));
    }

    // An object the CALLER owns, at the storage location a build of its own names.
    let caller_bucket = "my-own-game-builds";
    let caller_key = "builds/kept.zip";
    let create_bucket = format!(
        "<CreateBucketConfiguration><LocationConstraint>{REGION}</LocationConstraint>\
         </CreateBucketConfiguration>"
    );
    let (status, _) = gamelift
        .s3("PUT", &format!("/{caller_bucket}"), create_bucket.as_bytes())
        .await;
    assert_eq!(
        status, 200,
        "the caller's own bucket must be created, got {status}"
    );
    let (status, _) = gamelift
        .s3(
            "PUT",
            &format!("/{caller_bucket}/{caller_key}"),
            b"PK\x03\x04caller",
        )
        .await;
    assert_eq!(status, 200, "the caller's own object must be stored, got {status}");
    let caller_build = gamelift
        .ok(
            "CreateBuild",
            json!({
                "Name": "caller-owned",
                "OperatingSystem": OPERATING_SYSTEMS[0],
                "StorageLocation": {
                    "Bucket": caller_bucket,
                    "Key": caller_key,
                    "RoleArn": "arn:aws:iam::123456789012:role/GameLiftBuildAccess",
                },
            }),
        )
        .await["Build"]["BuildId"]
        .as_str()
        .expect("CreateBuild must report a BuildId")
        .to_string();

    // Delete the service-owned builds one at a time: each delete takes exactly its own
    // object, and the keys of the builds still stored still serve their bytes.
    for (deleted, (build_id, _, _, _)) in uploads.iter().enumerate() {
        gamelift
            .ok("DeleteBuild", json!({"BuildId": build_id}))
            .await;

        for (index, (_, bucket, key, body)) in uploads.iter().enumerate() {
            let (status, served) = gamelift.s3("GET", &format!("/{bucket}/{key}"), b"").await;
            if index <= deleted {
                assert_eq!(
                    status, 404,
                    "case {index}: the files of a deleted build must no longer be served, got \
                     {status} with {} bytes",
                    served.len()
                );
            } else {
                assert_eq!(
                    (status, served.as_ref()),
                    (200, body.as_slice()),
                    "case {index}: the files of a build still stored must read back as uploaded, \
                     got {status} with {} bytes",
                    served.len()
                );
            }
        }
    }

    // The caller's own object is untouched by its build's delete.
    gamelift
        .ok("DeleteBuild", json!({"BuildId": &caller_build}))
        .await;
    let (status, served) = gamelift
        .s3("GET", &format!("/{caller_bucket}/{caller_key}"), b"")
        .await;
    assert_eq!(
        (status, served.as_ref()),
        (200, b"PK\x03\x04caller".as_slice()),
        "an object in the caller's own bucket must survive the build's delete, got {status} with \
         {} bytes",
        served.len()
    );
}

// ---------------------------------------------------------------------------------------
// DeleteGameSessionQueue
// ---------------------------------------------------------------------------------------

/// `DeleteGameSessionQueue` removes the one queue its name resolves to: the name stops
/// resolving, the queue is gone from `DescribeGameSessionQueues` — both from the unfiltered
/// listing and from a describe that names it — and the other queues are untouched.
///
/// The rows differ along what the rule generalises over: which identifier form names the
/// queue (name or ARN), how many queues the store holds, which of them the request deletes,
/// how many destinations and latency policies it carries, and whether it carries a timeout
/// of its own.
#[tokio::test]
async fn deleting_a_game_session_queue_removes_it_from_the_queue_collection() {
    struct Case {
        label: &'static str,
        store: usize,
        target: usize,
        by_arn: bool,
        /// How many destinations the target queue lists.
        destinations: usize,
        /// How many player latency policies the target queue lists.
        policies: usize,
        /// Whether the target queue sets `TimeoutInSeconds` itself.
        timed: bool,
    }

    let cases = [
        Case {
            label: "the only queue in the store, named by its name",
            store: 1,
            target: 0,
            by_arn: false,
            destinations: 0,
            policies: 0,
            timed: false,
        },
        Case {
            label: "the only queue in the store, named by its ARN",
            store: 1,
            target: 0,
            by_arn: true,
            destinations: 1,
            policies: 0,
            timed: true,
        },
        Case {
            label: "the first of three queues, named by its name",
            store: 3,
            target: 0,
            by_arn: false,
            destinations: 2,
            policies: 1,
            timed: false,
        },
        Case {
            label: "a middle queue of five, named by its ARN",
            store: 5,
            target: 2,
            by_arn: true,
            destinations: 3,
            policies: 2,
            timed: true,
        },
        Case {
            label: "the last of four queues, named by its name",
            store: 4,
            target: 3,
            by_arn: false,
            destinations: 0,
            policies: 3,
            timed: false,
        },
    ];

    for case in &cases {
        let label = case.label;
        let gamelift = GameLift::new();
        let mut queues: Vec<Value> = Vec::new();
        for index in 0..case.store {
            let is_target = index == case.target;
            let name = format!("deletable-queue-{index}");
            let mut request = json!({"Name": &name});
            if is_target {
                if case.destinations > 0 {
                    request["Destinations"] = destinations(case.destinations);
                }
                if case.policies > 0 {
                    request["PlayerLatencyPolicies"] = latency_policies(case.policies);
                }
                if case.timed {
                    request["TimeoutInSeconds"] = json!(MAX_TIMEOUT);
                }
            }
            let created = gamelift.ok("CreateGameSessionQueue", request).await;
            queues.push(created["GameSessionQueue"].clone());
        }

        let target_name = format!("deletable-queue-{}", case.target);
        let selector = if case.by_arn {
            queue_arn(&target_name)
        } else {
            target_name.clone()
        };

        let (status, body) = gamelift
            .call("DeleteGameSessionQueue", &json!({"Name": &selector}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting the stored queue must succeed, got HTTP {status} with body \
             {body}"
        );
        assert!(
            body.is_null() || body == json!({}),
            "case {label:?}: DeleteGameSessionQueue reports no members, got {body}"
        );

        // The unfiltered listing is one queue shorter and no longer carries the deleted
        // record.
        let listed = gamelift
            .ok("DescribeGameSessionQueues", json!({}))
            .await["GameSessionQueues"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            listed.len(),
            case.store - 1,
            "case {label:?}: the listing must be one queue shorter, got {listed:?}"
        );
        for (index, queue) in queues.iter().enumerate() {
            let held = listed.iter().any(|reported| reported == queue);
            assert_eq!(
                held,
                index != case.target,
                "case {label:?}: queue {index} present={held} in the listing {listed:?}"
            );
        }

        // A describe that NAMES the deleted queue reports an empty page rather than the
        // record: "objects are returned only for queues that currently exist in the Region".
        for probe in [target_name.clone(), queue_arn(&target_name)] {
            let described = gamelift
                .ok("DescribeGameSessionQueues", json!({"Names": [&probe]}))
                .await;
            assert_eq!(
                described["GameSessionQueues"],
                json!([]),
                "case {label:?}: a describe naming the deleted {probe:?} must report no queue, got \
                 {described}"
            );
        }

        // Every operation that acts on ONE queue now finds none, by name and by ARN —
        // each refusing with the error class and sentence it words that refusal with.
        for probe in [target_name.clone(), queue_arn(&target_name)] {
            assert_refused(
                &gamelift,
                "UpdateGameSessionQueue",
                json!({"Name": &probe}),
                "InvalidRequestException",
                &format!("Queue name {probe} does not exist"),
                label,
            )
            .await;
            assert_refused(
                &gamelift,
                "DeleteGameSessionQueue",
                json!({"Name": &probe}),
                "NotFoundException",
                &format!("Game session queue {probe} does not exist."),
                label,
            )
            .await;
        }
    }
}

/// `DeleteGameSessionQueue` acts on ONE named resource, so a name that resolves to no
/// stored queue is the modelled `NotFoundException` rather than an empty success — and the
/// queue that IS stored is left exactly as it was.
#[tokio::test]
async fn deleting_a_game_session_queue_that_is_not_stored_is_refused_and_removes_nothing() {
    let gamelift = GameLift::new();
    let stored = gamelift
        .ok(
            "CreateGameSessionQueue",
            json!({"Name": "the-surviving-queue", "Destinations": destinations(2)}),
        )
        .await["GameSessionQueue"]
        .clone();

    gamelift
        .ok("CreateGameSessionQueue", json!({"Name": "the-deleted-queue"}))
        .await;
    gamelift
        .ok(
            "DeleteGameSessionQueue",
            json!({"Name": "the-deleted-queue"}),
        )
        .await;

    // The rows span both forms the modelled `GameSessionQueueNameOrArn` accepts
    // (`^([a-zA-Z0-9-]+|arn:.*:gamesessionqueue\/[a-zA-Z0-9-]+)$`), a name that never
    // existed and one that was deleted, and an ARN in another account and another Region
    // for a name that IS stored.
    let cases: Vec<(&str, String)> = vec![
        ("a name no queue carries", "no-such-queue".to_string()),
        ("an ARN no queue carries", queue_arn("no-such-queue")),
        ("the name of a deleted queue", "the-deleted-queue".to_string()),
        (
            "the ARN of a deleted queue",
            queue_arn("the-deleted-queue"),
        ),
        (
            "an ARN in another account for a stored name",
            format!("arn:aws:gamelift:{REGION}:999999999999:gamesessionqueue/the-surviving-queue"),
        ),
        (
            "an ARN in another Region for a stored name",
            format!(
                "arn:aws:gamelift:eu-central-1:{}:gamesessionqueue/the-surviving-queue",
                engine::emulator_account_id()
            ),
        ),
    ];

    for (label, selector) in &cases {
        assert_refused(
            &gamelift,
            "DeleteGameSessionQueue",
            json!({"Name": selector}),
            "NotFoundException",
            &format!("Game session queue {selector} does not exist."),
            label,
        )
        .await;
    }

    let described = gamelift
        .ok(
            "DescribeGameSessionQueues",
            json!({"Names": ["the-surviving-queue"]}),
        )
        .await;
    assert_eq!(
        described["GameSessionQueues"],
        json!([stored]),
        "the stored queue must survive every refused delete, got {described}"
    );
}

// ---------------------------------------------------------------------------------------
// DeleteLocation
// ---------------------------------------------------------------------------------------

/// `DeleteLocation` removes the one custom location its identifier resolves to: it is gone
/// from `ListLocations`, the other custom locations and the Amazon Web Services locations
/// are untouched, and the name is free for another `CreateLocation`.
///
/// The rows differ along what the rule generalises over: which identifier form names the
/// location (name or ARN), how many custom locations the store holds, which of them the
/// request deletes, the LENGTH of the name at each documented bound of
/// `CustomInputLocationStringModel` (len 8..64), and how many tags it carries.
#[tokio::test]
async fn deleting_a_custom_location_removes_it_from_the_location_listing() {
    struct Case {
        label: &'static str,
        store: usize,
        target: usize,
        by_arn: bool,
        /// The length of the target's name, within the modelled bounds (len 8..64).
        name_length: usize,
        tags: usize,
    }

    let cases = [
        Case {
            label: "the only location in the store, named by its name",
            store: 1,
            target: 0,
            by_arn: false,
            name_length: 8,
            tags: 0,
        },
        Case {
            label: "the only location in the store, named by its ARN",
            store: 1,
            target: 0,
            by_arn: true,
            name_length: 64,
            tags: 2,
        },
        Case {
            label: "the first of three locations, named by its name",
            store: 3,
            target: 0,
            by_arn: false,
            name_length: 20,
            tags: 1,
        },
        Case {
            label: "a middle location of five, named by its ARN",
            store: 5,
            target: 2,
            by_arn: true,
            name_length: 32,
            tags: 0,
        },
        Case {
            label: "the last of four locations, named by its name",
            store: 4,
            target: 3,
            by_arn: false,
            name_length: 9,
            tags: 3,
        },
    ];

    for case in &cases {
        let label = case.label;
        let gamelift = GameLift::new();
        let mut names: Vec<String> = Vec::new();
        for index in 0..case.store {
            let suffix = format!("{index}");
            let name = if index == case.target {
                let filler = case.name_length - "custom-".len() - suffix.len();
                format!("custom-{}{suffix}", "a".repeat(filler))
            } else {
                format!("custom-other-{index}")
            };
            let mut request = json!({"LocationName": &name});
            if index == case.target && case.tags > 0 {
                request["Tags"] = tag_list(case.tags);
            }
            let created = gamelift.ok("CreateLocation", request).await;
            assert_eq!(
                created["Location"],
                json!({"LocationName": &name, "LocationArn": location_arn(&name)}),
                "case {label:?}: the create must report the stored location, got {created}"
            );
            names.push(name);
        }

        let target = names[case.target].clone();
        let selector = if case.by_arn {
            location_arn(&target)
        } else {
            target.clone()
        };

        // The Amazon Web Services locations the store reports, so the delete below is shown
        // to leave them alone.
        let aws_before = gamelift
            .ok("ListLocations", json!({"Filters": ["AWS"], "Limit": 200}))
            .await["Locations"]
            .clone();

        let (status, body) = gamelift
            .call("DeleteLocation", &json!({"LocationName": &selector}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting the stored location must succeed, got HTTP {status} with \
             body {body}"
        );
        assert!(
            body.is_null() || body == json!({}),
            "case {label:?}: DeleteLocation reports no members, got {body}"
        );

        let listed = gamelift
            .ok("ListLocations", json!({"Filters": ["CUSTOM"], "Limit": 200}))
            .await["Locations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let reported: Vec<&str> = listed
            .iter()
            .map(|location| location["LocationName"].as_str().unwrap_or_default())
            .collect();
        assert!(
            !reported.contains(&target.as_str()),
            "case {label:?}: the deleted {target:?} must be gone from the listing, got {reported:?}"
        );
        for (index, name) in names.iter().enumerate() {
            if index == case.target {
                continue;
            }
            assert!(
                reported.contains(&name.as_str()),
                "case {label:?}: location {index} must survive the delete, got {reported:?}"
            );
        }
        let aws_after = gamelift
            .ok("ListLocations", json!({"Filters": ["AWS"], "Limit": 200}))
            .await["Locations"]
            .clone();
        assert_eq!(
            aws_after, aws_before,
            "case {label:?}: deleting a custom location must leave the Amazon Web Services \
             locations alone, got {aws_after}"
        );

        // A second delete, by either form, finds no location; and the name is free again,
        // which a create of it proves.
        for probe in [target.clone(), location_arn(&target)] {
            assert_refused(
                &gamelift,
                "DeleteLocation",
                json!({"LocationName": &probe}),
                "NotFoundException",
                &format!("Location ({probe}) not found"),
                label,
            )
            .await;
        }
        let recreated = gamelift
            .ok("CreateLocation", json!({"LocationName": &target}))
            .await;
        assert_eq!(
            recreated["Location"]["LocationName"],
            json!(&target),
            "case {label:?}: the deleted name must be free for another create, got {recreated}"
        );
    }
}

/// `DeleteLocation` acts on ONE named resource, so an identifier that resolves to no stored
/// custom location is the modelled `NotFoundException` rather than an empty success — and
/// the location that IS stored is left exactly as it was.
#[tokio::test]
async fn deleting_a_custom_location_that_is_not_stored_is_refused_and_removes_nothing() {
    let gamelift = GameLift::new();
    gamelift
        .ok("CreateLocation", json!({"LocationName": "custom-survivor"}))
        .await;
    gamelift
        .ok("CreateLocation", json!({"LocationName": "custom-deleted"}))
        .await;
    gamelift
        .ok("DeleteLocation", json!({"LocationName": "custom-deleted"}))
        .await;

    // The rows span both forms the modelled `CustomLocationNameOrArnModel` accepts
    // (`^(custom-[A-Za-z0-9\-]+|arn:.*:location\/custom-\S+)$`), a name that never existed
    // and one that was deleted, and an ARN in another account and another Region for a name
    // that IS stored.
    let cases: Vec<(&str, String)> = vec![
        (
            "a name no location carries",
            "custom-absent".to_string(),
        ),
        (
            "an ARN no location carries",
            location_arn("custom-absent-by-arn"),
        ),
        ("the name of a deleted location", "custom-deleted".to_string()),
        (
            "the ARN of a deleted location",
            location_arn("custom-deleted"),
        ),
        (
            "an ARN in another account for a stored name",
            format!("arn:aws:gamelift:{REGION}:999999999999:location/custom-survivor"),
        ),
        (
            "an ARN in another Region for a stored name",
            format!(
                "arn:aws:gamelift:eu-central-1:{}:location/custom-survivor",
                engine::emulator_account_id()
            ),
        ),
    ];

    for (label, selector) in &cases {
        assert_refused(
            &gamelift,
            "DeleteLocation",
            json!({"LocationName": selector}),
            "NotFoundException",
            &format!("Location ({selector}) not found"),
            label,
        )
        .await;
    }

    let listed = gamelift
        .ok("ListLocations", json!({"Filters": ["CUSTOM"], "Limit": 200}))
        .await;
    assert_eq!(
        listed["Locations"],
        json!([{
            "LocationName": "custom-survivor",
            "LocationArn": location_arn("custom-survivor"),
        }]),
        "the stored location must survive every refused delete, got {listed}"
    );
}

// ---------------------------------------------------------------------------------------
// DeleteMatchmakingConfiguration and DeleteMatchmakingRuleSet
// ---------------------------------------------------------------------------------------

/// Create a rule set named `{name}-rules` and a matchmaking configuration named `name` on
/// it, and report the stored configuration.
async fn create_matchmaker(gamelift: &GameLift, name: &str, extra: Value) -> Value {
    gamelift
        .ok(
            "CreateMatchmakingRuleSet",
            json!({
                "Name": format!("{name}-rules"),
                "RuleSetBody": conformant_body(name),
            }),
        )
        .await;
    let mut request = json!({
        "Name": name,
        "RuleSetName": format!("{name}-rules"),
        "RequestTimeoutSeconds": 30,
        "AcceptanceRequired": false,
        "FlexMatchMode": "STANDALONE",
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            request[member] = value.clone();
        }
    }
    gamelift
        .ok("CreateMatchmakingConfiguration", request)
        .await["Configuration"]
        .clone()
}

/// `DeleteMatchmakingConfiguration` removes the one configuration its identifier resolves
/// to: it is gone from `DescribeMatchmakingConfigurations`, a describe that NAMES it is
/// refused with `InvalidRequestException`, the other configurations are untouched, and the
/// rule set it matched with is still stored.
///
/// The rows differ along what the rule generalises over: which identifier form names the
/// configuration (name or ARN), how many configurations the store holds, which of them the
/// request deletes, how many tags and game properties it carries, and whether
/// `UpdateMatchmakingConfiguration` revised it first.
#[tokio::test]
async fn deleting_a_matchmaking_configuration_removes_the_one_it_resolves_to() {
    struct Case {
        label: &'static str,
        store: usize,
        target: usize,
        by_arn: bool,
        tags: usize,
        /// Whether the target carries a description and custom event data of its own.
        described: bool,
        /// Whether `UpdateMatchmakingConfiguration` revised the target before the delete.
        updated: bool,
    }

    let cases = [
        Case {
            label: "the only matchmaker in the store, named by its name",
            store: 1,
            target: 0,
            by_arn: false,
            tags: 0,
            described: false,
            updated: false,
        },
        Case {
            label: "the only matchmaker in the store, named by its ARN",
            store: 1,
            target: 0,
            by_arn: true,
            tags: 2,
            described: true,
            updated: false,
        },
        Case {
            label: "the first of three matchmakers, named by its name",
            store: 3,
            target: 0,
            by_arn: false,
            tags: 1,
            described: false,
            updated: false,
        },
        Case {
            label: "a middle matchmaker of four, named by its ARN",
            store: 4,
            target: 2,
            by_arn: true,
            tags: 0,
            described: true,
            updated: true,
        },
        Case {
            label: "the last of three matchmakers, named by its name",
            store: 3,
            target: 2,
            by_arn: false,
            tags: 3,
            described: false,
            updated: true,
        },
    ];

    for case in &cases {
        let label = case.label;
        let gamelift = GameLift::new();
        let mut names: Vec<String> = Vec::new();
        let mut stored: Vec<Value> = Vec::new();
        for index in 0..case.store {
            let name = format!("deletable-matchmaker-{index}");
            let mut extra = json!({});
            if index == case.target {
                if case.tags > 0 {
                    extra["Tags"] = tag_list(case.tags);
                }
                if case.described {
                    extra["Description"] = json!("the matchmaker under test");
                    extra["CustomEventData"] = json!("row-under-test");
                }
            }
            stored.push(create_matchmaker(&gamelift, &name, extra).await);
            names.push(name);
        }

        if case.updated {
            let updated = gamelift
                .ok(
                    "UpdateMatchmakingConfiguration",
                    json!({"Name": &names[case.target], "RequestTimeoutSeconds": 45}),
                )
                .await;
            stored[case.target] = updated["Configuration"].clone();
        }

        let target = names[case.target].clone();
        let selector = if case.by_arn {
            configuration_arn(&target)
        } else {
            target.clone()
        };

        let (status, body) = gamelift
            .call("DeleteMatchmakingConfiguration", &json!({"Name": &selector}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting the stored matchmaker must succeed, got HTTP {status} with \
             body {body}"
        );
        assert!(
            body.is_null() || body == json!({}),
            "case {label:?}: DeleteMatchmakingConfiguration reports no members, got {body}"
        );

        let listed = gamelift
            .ok("DescribeMatchmakingConfigurations", json!({"Limit": 10}))
            .await["Configurations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            listed.len(),
            case.store - 1,
            "case {label:?}: the listing must be one matchmaker shorter, got {listed:?}"
        );
        for (index, configuration) in stored.iter().enumerate() {
            let held = listed.iter().any(|reported| reported == configuration);
            assert_eq!(
                held,
                index != case.target,
                "case {label:?}: matchmaker {index} present={held} in the listing {listed:?}"
            );
        }

        // A describe that NAMES the deleted configuration, and every mutating operation on
        // it, now find none — by name and by ARN. `DescribeMatchmakingConfigurations`
        // models `InvalidRequestException` and not `NotFoundException`, so it is the error
        // class that operation declares.
        for probe in [target.clone(), configuration_arn(&target)] {
            assert_refused(
                &gamelift,
                "DescribeMatchmakingConfigurations",
                json!({"Names": [&probe]}),
                "InvalidRequestException",
                &format!("Configuration not found: {probe}"),
                label,
            )
            .await;
            for operation in [
                "UpdateMatchmakingConfiguration",
                "DeleteMatchmakingConfiguration",
            ] {
                assert_refused(
                    &gamelift,
                    operation,
                    json!({"Name": &probe}),
                    "NotFoundException",
                    &format!("Configuration with name {probe} does not exist"),
                    label,
                )
                .await;
            }
        }

        // The rule set the configuration matched with is NOT touched by the delete: a
        // configuration points at a rule set, it does not own one.
        let rule_sets = gamelift
            .ok(
                "DescribeMatchmakingRuleSets",
                json!({"Names": [format!("{target}-rules")]}),
            )
            .await["RuleSets"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            rule_sets.len(),
            1,
            "case {label:?}: the rule set must survive the configuration's delete, got \
             {rule_sets:?}"
        );
    }
}

/// `DeleteMatchmakingRuleSet` removes the one rule set its identifier resolves to, once no
/// matchmaking configuration uses it: it is gone from `DescribeMatchmakingRuleSets`, a
/// describe that NAMES it is refused, and a configuration can no longer be created against
/// it.
///
/// The rows differ along what the rule generalises over: which identifier form names the
/// rule set (name or ARN), how many rule sets the store holds, and which of them the
/// request deletes.
#[tokio::test]
async fn deleting_a_matchmaking_rule_set_removes_the_one_it_resolves_to() {
    struct Case {
        label: &'static str,
        store: usize,
        target: usize,
        by_arn: bool,
    }

    let cases = [
        Case {
            label: "the only rule set in the store, named by its name",
            store: 1,
            target: 0,
            by_arn: false,
        },
        Case {
            label: "the only rule set in the store, named by its ARN",
            store: 1,
            target: 0,
            by_arn: true,
        },
        Case {
            label: "the first of three rule sets, named by its name",
            store: 3,
            target: 0,
            by_arn: false,
        },
        Case {
            label: "a middle rule set of five, named by its ARN",
            store: 5,
            target: 2,
            by_arn: true,
        },
        Case {
            label: "the last of four rule sets, named by its name",
            store: 4,
            target: 3,
            by_arn: false,
        },
    ];

    for case in &cases {
        let label = case.label;
        let gamelift = GameLift::new();
        let mut names: Vec<String> = Vec::new();
        for index in 0..case.store {
            let name = format!("deletable-rules-{index}");
            gamelift
                .ok(
                    "CreateMatchmakingRuleSet",
                    json!({"Name": &name, "RuleSetBody": conformant_body(&name)}),
                )
                .await;
            names.push(name);
        }

        let target = names[case.target].clone();
        let selector = if case.by_arn {
            rule_set_arn(&target)
        } else {
            target.clone()
        };

        let (status, body) = gamelift
            .call("DeleteMatchmakingRuleSet", &json!({"Name": &selector}))
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: deleting the stored rule set must succeed, got HTTP {status} with \
             body {body}"
        );
        assert!(
            body.is_null() || body == json!({}),
            "case {label:?}: DeleteMatchmakingRuleSet reports no members, got {body}"
        );

        let listed = gamelift
            .ok("DescribeMatchmakingRuleSets", json!({}))
            .await["RuleSets"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let reported: Vec<&str> = listed
            .iter()
            .map(|rule_set| rule_set["RuleSetName"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            reported.len(),
            case.store - 1,
            "case {label:?}: the listing must be one rule set shorter, got {reported:?}"
        );
        for (index, name) in names.iter().enumerate() {
            let held = reported.contains(&name.as_str());
            assert_eq!(
                held,
                index != case.target,
                "case {label:?}: rule set {index} present={held} in the listing {reported:?}"
            );
        }

        // A describe that NAMES the deleted rule set, and a second delete, now find none —
        // by name and by ARN. `DescribeMatchmakingRuleSets` models
        // `InvalidRequestException` and not `NotFoundException`, so it is the error class
        // that operation declares.
        for probe in [target.clone(), rule_set_arn(&target)] {
            assert_refused(
                &gamelift,
                "DescribeMatchmakingRuleSets",
                json!({"Names": [&probe]}),
                "InvalidRequestException",
                &format!("Failed to find rule set: {probe}"),
                label,
            )
            .await;
            // The delete words the same missing rule set its own way, in the
            // `NotFoundException` it declares.
            assert_refused(
                &gamelift,
                "DeleteMatchmakingRuleSet",
                json!({"Name": &probe}),
                "NotFoundException",
                &format!("Matchmaking rule set {probe} not found"),
                label,
            )
            .await;
        }

        // A configuration can no longer be created against the deleted rule set: the name
        // resolves to nothing, so the cross-resource check refuses the create.
        assert_refused(
            &gamelift,
            "CreateMatchmakingConfiguration",
            json!({
                "Name": "after-the-rule-set-went",
                "RuleSetName": &target,
                "RequestTimeoutSeconds": 30,
                "AcceptanceRequired": false,
                "FlexMatchMode": "STANDALONE",
            }),
            "InvalidRequestException",
            &format!("RuleSet with name {target} does not exist"),
            label,
        )
        .await;
    }
}

/// "Rule sets cannot be deleted if they are currently being used by a matchmaking
/// configuration": while a configuration points at the rule set the delete is refused and
/// the rule set stays stored; once the configuration is gone, the same delete succeeds.
#[tokio::test]
async fn a_rule_set_in_use_by_a_configuration_cannot_be_deleted() {
    let gamelift = GameLift::new();

    // Several configurations on ONE rule set, so the refusal is about the rule set being
    // referenced at all rather than about one particular matchmaker.
    create_matchmaker(&gamelift, "shared", json!({})).await;
    for index in 0..2 {
        gamelift
            .ok(
                "CreateMatchmakingConfiguration",
                json!({
                    "Name": format!("shared-user-{index}"),
                    "RuleSetName": "shared-rules",
                    "RequestTimeoutSeconds": 30,
                    "AcceptanceRequired": false,
                    "FlexMatchMode": "STANDALONE",
                }),
            )
            .await;
    }

    // Three configurations use it, then two, then one: the delete is refused each time, by
    // name and by ARN alike, with the one in-use sentence whatever the number still
    // referencing it, and the rule set is still there afterwards.
    let users = ["shared", "shared-user-0", "shared-user-1"];
    for (index, user) in users.iter().enumerate() {
        let still_using = users.len() - index;
        for probe in ["shared-rules".to_string(), rule_set_arn("shared-rules")] {
            assert_refused(
                &gamelift,
                "DeleteMatchmakingRuleSet",
                json!({"Name": &probe}),
                "InvalidRequestException",
                "Matchmaking rule set is still in use, cannot delete",
                &format!("{still_using} configurations still using it"),
            )
            .await;
        }
        let listed = gamelift
            .ok(
                "DescribeMatchmakingRuleSets",
                json!({"Names": ["shared-rules"]}),
            )
            .await;
        assert_eq!(
            listed["RuleSets"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            1,
            "a refused delete must leave the rule set stored, got {listed}"
        );

        gamelift
            .ok("DeleteMatchmakingConfiguration", json!({"Name": user}))
            .await;
    }

    // Nothing uses it now, so the same delete succeeds and the rule set is gone.
    gamelift
        .ok("DeleteMatchmakingRuleSet", json!({"Name": "shared-rules"}))
        .await;
    assert_refused(
        &gamelift,
        "DescribeMatchmakingRuleSets",
        json!({"Names": ["shared-rules"]}),
        "InvalidRequestException",
        "Failed to find rule set: shared-rules",
        "the rule set nothing uses any more",
    )
    .await;
}

/// "A matchmaking configuration cannot be deleted if it is being used in any active
/// matchmaking tickets": while a ticket of the matchmaker is still in process the delete is
/// refused and the configuration stays stored; once the ticket has reached its terminal
/// status, the same delete succeeds.
#[tokio::test]
async fn a_configuration_with_active_tickets_cannot_be_deleted() {
    let gamelift = GameLift::new();
    // `RequestTimeoutSeconds` decides how long a ticket stays in process, so the rows below
    // are a matchmaker whose tickets time out within this test and one whose do not.
    create_matchmaker(
        &gamelift,
        "busy",
        json!({"RequestTimeoutSeconds": 1}),
    )
    .await;

    for index in 0..2 {
        gamelift
            .ok(
                "StartMatchmaking",
                json!({
                    "ConfigurationName": "busy",
                    "Players": [{"PlayerId": format!("player-{index}")}],
                }),
            )
            .await;
    }

    for probe in ["busy".to_string(), configuration_arn("busy")] {
        assert_refused(
            &gamelift,
            "DeleteMatchmakingConfiguration",
            json!({"Name": &probe}),
            "InvalidRequestException",
            "Matchmaking configuration busy is being used in 2 active matchmaking tickets and \
             cannot be deleted.",
            "two tickets still in process",
        )
        .await;
    }
    let listed = gamelift
        .ok(
            "DescribeMatchmakingConfigurations",
            json!({"Names": ["busy"]}),
        )
        .await;
    assert_eq!(
        listed["Configurations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "a refused delete must leave the configuration stored, got {listed}"
    );

    // The tickets reach their terminal status once the configuration's own
    // `RequestTimeoutSeconds` has passed — the clock alone retires them — and then the
    // matchmaker can go.
    std::thread::sleep(std::time::Duration::from_millis(1_250));
    gamelift
        .ok("DeleteMatchmakingConfiguration", json!({"Name": "busy"}))
        .await;
    assert_refused(
        &gamelift,
        "DescribeMatchmakingConfigurations",
        json!({"Names": ["busy"]}),
        "InvalidRequestException",
        "Configuration not found: busy",
        "the configuration whose tickets have timed out",
    )
    .await;
}
