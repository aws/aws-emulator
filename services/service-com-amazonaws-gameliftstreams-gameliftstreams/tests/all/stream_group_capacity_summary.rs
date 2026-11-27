// SPDX-License-Identifier: CC0-1.0

//! The capacity a stream group reports for ITSELF, beside its per-location
//! `LocationStates`.
//!
//! The rule under test: a stream group reports its own capacity as the roll-up of the
//! locations it holds — `DesiredCapacity` is the capacity those locations have most
//! recently requested, `ActiveCapacity` the capacity the service has provisioned
//! across them, and `IdleCapacity` how much of that provisioned capacity is not
//! streaming — and it reports the same three members through every operation that
//! answers with a stream group (`CreateStreamGroup`, `UpdateStreamGroup`,
//! `GetStreamGroup`). The three members are recorded behaviour: this service's
//! published model declares capacity only per location, so they are declared from
//! this crate's own overlay model `smithy-model/local-additions.json` (see
//! `smithy-model/LOCAL-ADDITIONS.md` and the module docs of
//! `src/ops/get_stream_group.rs`).
//!
//! The table spans the class: the rows differ in how many locations one group holds,
//! in which capacity setting each location is configured through (the always-on
//! minimum, the target-idle level, the maximum bound and the deprecated on-demand
//! bound), and therefore in whether what a location requests is provisioned at all.
//! A further test drives the summary while a stream session is streaming — the one
//! thing that moves `IdleCapacity` without any capacity being reconfigured — and
//! through an `UpdateStreamGroup` that scales a location. The last test pins the
//! overlay against the published snapshot, shape by shape: a refreshed snapshot that
//! declares those members itself, drops a member the overlay restates or changes a
//! target, type or value set fails there rather than silently narrowing what the
//! service reports.
//!
//! Every assertion reads a RESPONSE BODY, and every mutation is read back through
//! `GetStreamGroup`, so a handler that answered 2xx without persisting anything fails
//! here.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::application_read_projection::{poll_until, stream_group_fully_active};
use super::stream_group_capacity::{
    group_with_locations, PLACED_SESSION_STATUSES, TERMINATED_SESSION_STATUSES,
};
use super::stream_workflow::{call, emulator, ready_application, text, REGION};

/// The three members a stream group reports for itself.
const SUMMARY_MEMBERS: [&str; 3] = ["DesiredCapacity", "ActiveCapacity", "IdleCapacity"];

/// The published model snapshot this crate is generated from, embedded at compile time
/// so the overlay is checked against the model rather than against a copy of it.
const SNAPSHOT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-gameliftstreams-gameliftstreams.json"
));

/// The overlay model the three stream-group outputs are declared from, embedded for
/// the same reason: what it restates of the snapshot must still match it.
const OVERLAY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/local-additions.json"
));

/// The shapes the overlay restates with the summary members, and the shape those
/// members target.
const OUTPUT_SHAPES: [&str; 3] = [
    "com.amazonaws.gameliftstreams#CreateStreamGroupOutput",
    "com.amazonaws.gameliftstreams#GetStreamGroupOutput",
    "com.amazonaws.gameliftstreams#UpdateStreamGroupOutput",
];
/// The target the three added members carry, which the snapshot declares for the
/// per-location capacities.
const CAPACITY_TARGET: &str = "com.amazonaws.gameliftstreams#CapacityValue";

/// The summary a body is expected to carry: requested, provisioned, idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Summary {
    desired: i64,
    active: i64,
    idle: i64,
}

/// Assert `body` reports exactly `expected` as its own capacity, every member of it a
/// number.
fn assert_summary(body: &Value, expected: Summary, case: &str) {
    for member in SUMMARY_MEMBERS {
        assert!(
            body[member].is_number(),
            "case {case:?}: {member} must be reported as a number, got {body}"
        );
    }
    let got = Summary {
        desired: body["DesiredCapacity"].as_i64().unwrap_or(-1),
        active: body["ActiveCapacity"].as_i64().unwrap_or(-1),
        idle: body["IdleCapacity"].as_i64().unwrap_or(-1),
    };
    assert_eq!(
        got, expected,
        "case {case:?}: expected the group to report {expected:?}, got {got:?} in {body}"
    );
}

/// Read a group back and answer its body.
async fn read(router: &MultiServiceRouter, group_id: &str, case: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/streamgroups/{group_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetStreamGroup 200, got {status} {body}"
    );
    body
}

/// One row: the case, the locations the group is created with, and the summary it
/// reports before and after the clock has provisioned those locations.
struct SummaryCase<'a> {
    case: &'a str,
    configurations: Vec<Value>,
    activating: Summary,
    active: Summary,
}

#[tokio::test]
async fn a_stream_group_reports_the_capacity_of_the_locations_it_holds() {
    let router = emulator();
    let (_, app_arn) = ready_application(&router, "summary-app").await;

    let cases: [SummaryCase; 6] = [
        SummaryCase {
            case: "one location with always-on capacity",
            configurations: vec![json!({"LocationName": REGION, "AlwaysOnCapacity": 2})],
            activating: Summary {
                desired: 2,
                active: 0,
                idle: 0,
            },
            active: Summary {
                desired: 2,
                active: 2,
                idle: 2,
            },
        },
        SummaryCase {
            // A bound alone: the group asks the service to be able to allocate that
            // much, and nothing stands provisioned until a stream asks for it.
            case: "one location with a maximum bound alone",
            configurations: vec![json!({"LocationName": REGION, "MaximumCapacity": 2})],
            activating: Summary {
                desired: 2,
                active: 0,
                idle: 0,
            },
            active: Summary {
                desired: 2,
                active: 0,
                idle: 0,
            },
        },
        SummaryCase {
            case: "one location with the deprecated on-demand bound",
            configurations: vec![json!({"LocationName": REGION, "OnDemandCapacity": 4})],
            activating: Summary {
                desired: 4,
                active: 0,
                idle: 0,
            },
            active: Summary {
                desired: 4,
                active: 0,
                idle: 0,
            },
        },
        SummaryCase {
            case: "one location holding a target idle level under a maximum",
            configurations: vec![json!({
                "LocationName": REGION,
                "MaximumCapacity": 6,
                "TargetIdleCapacity": 2,
            })],
            activating: Summary {
                desired: 6,
                active: 0,
                idle: 0,
            },
            active: Summary {
                desired: 6,
                active: 2,
                idle: 2,
            },
        },
        SummaryCase {
            case: "three locations, each configured differently",
            configurations: vec![
                json!({"LocationName": REGION, "AlwaysOnCapacity": 1}),
                json!({"LocationName": "us-east-1", "MaximumCapacity": 5,
                       "TargetIdleCapacity": 3}),
                json!({"LocationName": "eu-west-1", "MaximumCapacity": 2}),
            ],
            activating: Summary {
                desired: 8,
                active: 0,
                idle: 0,
            },
            active: Summary {
                desired: 8,
                active: 4,
                idle: 4,
            },
        },
        SummaryCase {
            case: "a location that asks for nothing",
            configurations: vec![json!({"LocationName": REGION})],
            activating: Summary {
                desired: 0,
                active: 0,
                idle: 0,
            },
            active: Summary {
                desired: 0,
                active: 0,
                idle: 0,
            },
        },
    ];

    for (index, row) in cases.into_iter().enumerate() {
        let SummaryCase {
            case,
            configurations,
            activating,
            active,
        } = row;
        // The create answers the same projection a read does, over locations that are
        // still being prepared and hold nothing yet. Asserted on the create's own
        // response: a later read can only be made once the clock has provisioned them.
        let (group_id, _, created) = group_with_locations(
            &router,
            &format!("summary-group-{index}"),
            &app_arn,
            &configurations,
        )
        .await;
        assert_summary(&created, activating, case);

        // Once the clock has provisioned the locations — polled for, not waited on —
        // the group reports what they now hold.
        stream_group_fully_active(&router, &group_id, case).await;
        let group = read(&router, &group_id, case).await;
        assert_summary(&group, active, case);

        // Reading a group changes nothing about the capacity a later read sees.
        let again = read(&router, &group_id, case).await;
        assert_summary(&again, active, case);
    }
}

#[tokio::test]
async fn the_summary_follows_a_streaming_session_and_a_capacity_update() {
    let router = emulator();
    let (app_id, app_arn) = ready_application(&router, "summary-session-app").await;
    let (group_id, _, _) = group_with_locations(
        &router,
        "summary-session-group",
        &app_arn,
        &[json!({"LocationName": REGION, "AlwaysOnCapacity": 2})],
    )
    .await;
    // Wait for the CONDITION the summary depends on — the location provisioned —
    // rather than for a fixed time.
    stream_group_fully_active(&router, &group_id, "summary-session-group").await;

    let case = "two always-on streams, none of them streaming";
    assert_summary(
        &read(&router, &group_id, case).await,
        Summary {
            desired: 2,
            active: 2,
            idle: 2,
        },
        case,
    );

    // A session streaming occupies one of the two provisioned streams: the group
    // still has the same capacity requested and provisioned, and half of it idle.
    // "NO_CLIENT_CONNECTION ... will create a session without needing any browser
    // request or Web SDK integration", so the stream is placed and stays placed.
    let (status, session) = call(
        &router,
        "POST",
        &format!("/streamgroups/{group_id}/streamsessions"),
        Some(json!({
            "Protocol": "WebRTC",
            "SignalRequest": "NO_CLIENT_CONNECTION",
            "ApplicationIdentifier": app_id,
            "Locations": [REGION],
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"one session streaming\": expected StartStreamSession 201, got {status} \
         {session}"
    );
    let case = "one session streaming";
    let session_id = text(&session, "/Arn", case)
        .rsplit('/')
        .next()
        .expect("a stream session ARN ends in the session id")
        .to_string();
    let session_path = format!("/streamgroups/{group_id}/streamsessions/{session_id}");
    poll_until(&router, &session_path, case, |body| {
        PLACED_SESSION_STATUSES
            .iter()
            .any(|status| body["Status"] == json!(status))
    })
    .await;
    assert_summary(
        &read(&router, &group_id, case).await,
        Summary {
            desired: 2,
            active: 2,
            idle: 1,
        },
        case,
    );

    // Scaling the location through `UpdateStreamGroup`: the update answers the new
    // summary and a read agrees with it.
    let case = "the location scaled to four always-on streams";
    let (status, updated) = call(
        &router,
        "PATCH",
        &format!("/streamgroups/{group_id}"),
        Some(json!({
            "LocationConfigurations": [{"LocationName": REGION, "AlwaysOnCapacity": 4}],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected UpdateStreamGroup 200, got {status} {updated}"
    );
    assert_eq!(
        text(&updated, "/Id", case),
        group_id,
        "case {case:?}: the update answers the group it addressed, got {updated}"
    );
    // The session terminated gives its stream back, so the whole allocation is idle.
    let (status, terminated) = call(&router, "DELETE", &session_path, None).await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected TerminateStreamSession 204, got {status} {terminated}"
    );
    poll_until(&router, &session_path, case, |body| {
        TERMINATED_SESSION_STATUSES
            .iter()
            .any(|status| body["Status"] == json!(status))
    })
    .await;
    let expected = Summary {
        desired: 4,
        active: 4,
        idle: 4,
    };
    assert_summary(&read(&router, &group_id, case).await, expected, case);
}

/// The `shapes` object of one model document.
fn model_shapes(model: &str) -> Map<String, Value> {
    let parsed: Value = serde_json::from_str(model).expect("a model document must be JSON");
    parsed["shapes"]
        .as_object()
        .expect("a model document must carry a shapes object")
        .clone()
}

/// The members of one shape declaration, or an empty set for a shape declared
/// without any.
fn members_of(declaration: &Value) -> Map<String, Value> {
    declaration["members"]
        .as_object()
        .cloned()
        .unwrap_or_default()
}

/// The wire values of an enum shape, in the order it declares them.
fn enum_values(declaration: &Value) -> Vec<String> {
    members_of(declaration)
        .values()
        .map(|member| {
            member["traits"]["smithy.api#enumValue"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Is this member declaration `@required`?
fn is_required(member: &Value) -> bool {
    !member["traits"]["smithy.api#required"].is_null()
}

#[test]
fn the_overlay_restates_the_snapshot_and_adds_the_capacity_summary() {
    // The overlay is what the three stream-group output shapes are generated from, so
    // everything it restates of the snapshot — a member's target and requiredness, a
    // shape's type, an enum's values — must still BE what the snapshot says, or what
    // goes on the wire would differ from the model without any other test noticing.
    // Constraint traits are deliberately NOT restated (see
    // `smithy-model/LOCAL-ADDITIONS.md`): every request is validated against the
    // snapshot's own declarations on the way in.
    let snapshot = model_shapes(SNAPSHOT);
    let overlay = model_shapes(OVERLAY);

    for shape_id in OUTPUT_SHAPES {
        let declared = overlay
            .get(shape_id)
            .unwrap_or_else(|| panic!("the overlay must declare {shape_id}"));
        let modelled = snapshot
            .get(shape_id)
            .unwrap_or_else(|| panic!("the snapshot no longer declares {shape_id}"));
        let declared_members = members_of(declared);
        let modelled_members = members_of(modelled);

        for (member, modelled_member) in &modelled_members {
            let restated = declared_members.get(member).unwrap_or_else(|| {
                panic!("the overlay drops {member} of {shape_id}, which the snapshot declares")
            });
            assert_eq!(
                restated["target"], modelled_member["target"],
                "case {shape_id:?}: member {member} must be restated with the target the \
                 snapshot gives it, got {restated:?}"
            );
            assert_eq!(
                is_required(restated),
                is_required(modelled_member),
                "case {shape_id:?}: member {member} must be restated as required={}, got \
                 {restated:?}",
                is_required(modelled_member)
            );
        }

        let added: Vec<&String> = declared_members
            .keys()
            .filter(|member| !modelled_members.contains_key(*member))
            .collect();
        let mut expected: Vec<String> = SUMMARY_MEMBERS.iter().map(|m| (*m).to_string()).collect();
        expected.sort();
        let mut got: Vec<String> = added.iter().map(|m| (*m).clone()).collect();
        got.sort();
        assert_eq!(
            got, expected,
            "case {shape_id:?}: the overlay may add the group's own capacity summary and \
             nothing else, added {got:?}"
        );
        for member in SUMMARY_MEMBERS {
            assert!(
                !modelled_members.contains_key(member),
                "case {shape_id:?}: the snapshot now declares {member} itself, so the local \
                 declaration should give way to the modelled member"
            );
            assert_eq!(
                declared_members[member]["target"],
                json!(CAPACITY_TARGET),
                "case {shape_id:?}: {member} is a capacity, got {:?}",
                declared_members[member]
            );
        }
    }

    // Every OTHER shape the overlay names is there only because those members target
    // it, and is restated as the snapshot declares it.
    for (shape_id, declared) in &overlay {
        if OUTPUT_SHAPES.contains(&shape_id.as_str()) {
            continue;
        }
        let modelled = snapshot
            .get(shape_id)
            .unwrap_or_else(|| panic!("the overlay restates {shape_id}, which the snapshot does not declare"));
        assert_eq!(
            declared["type"], modelled["type"],
            "case {shape_id:?}: must be restated with the type the snapshot gives it, got {declared:?}"
        );
        match declared["type"].as_str() {
            // An enum's values are what validation enforces, so they are restated.
            Some("enum") => assert_eq!(
                enum_values(declared),
                enum_values(modelled),
                "case {shape_id:?}: must be restated with the values the snapshot declares"
            ),
            Some("list") => assert_eq!(
                declared["member"]["target"], modelled["member"]["target"],
                "case {shape_id:?}: must be restated over the snapshot's own member target, \
                 got {declared:?}"
            ),
            // A structure member is resolved through the Rust field's type, which is
            // generated from the snapshot, so the overlay restates no member of one.
            Some("structure") => assert!(
                members_of(declared).is_empty(),
                "case {shape_id:?}: is restated for its type alone, so it must declare no \
                 members, got {declared:?}"
            ),
            // A scalar carries nothing but its type here.
            _ => assert!(
                members_of(declared).is_empty(),
                "case {shape_id:?}: a scalar target is restated for its type alone, got \
                 {declared:?}"
            ),
        }
    }
}
