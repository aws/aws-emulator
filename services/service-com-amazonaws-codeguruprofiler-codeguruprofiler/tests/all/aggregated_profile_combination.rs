// SPDX-License-Identifier: CC0-1.0

//! What `GetProfile` serves as the AGGREGATED PROFILE of a window, end to end over HTTP
//! through the router this service mounts in production.
//!
//! The rules the cases here assert:
//!
//! 1. the aggregated profile a request's range resolves to
//!    holds EVERY aggregated profile the group holds in that range combined into one
//!    document — the service collects the profiles posted during an aggregation period
//!    into that period's aggregated profile, and a range covering several of them is
//!    answered with them combined as one aggregated profile — so frames of the same name
//!    combine, the sample counts two profiles record for one frame add, and a frame only
//!    one of them records is in the answer too. No profile of the range is left out of the
//!    answer.
//! 2. the document is the SERVICE'S own: it states the window it covers as the two
//!    ISO 8601 UTC instants `start` and `end`, at the millisecond resolution this service
//!    states every instant at, and an `aggregatedMetadata` object whose
//!    `numReportedAgents` counts the agents that reported into that window — all three
//!    derived from the aggregated profile served and the submissions it holds, never
//!    copied out of the epoch-millisecond bounds a submitted agent profile states for
//!    itself.
//!
//! Rows vary what the rules generalise over: how many profiles the range holds (one, two,
//! three), whether their frames are disjoint, shared, nested or listed, how deep the
//! frames sit, whether the counts are whole or fractional, which starts the profiles
//! state, what else the documents state about themselves, whether the profiles share one
//! aggregation window or sit in several inside the range, how the request names the window
//! (each of the three aggregation periods as a length, a length that is none of them, a
//! pair of bounds inside and across the data's windows), where that window sits on the
//! time axis (an epoch-aligned boundary, part-way in, before the epoch), which
//! representation each profile is stored in and which the answer is asked for, and what
//! the request asks be done with the combination (`Accept`, `maxDepth`).
//!
//! The emulator, the HTTP plumbing and the receipt placement are the shared
//! [`crate::profiling_group_lifecycle`] harness and [`crate::profile_times`] writers, not
//! a fork of either.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;
use ion_rs::{Element, v1_0};

use crate::frame_metric_data::{TOLERANCE, assert_value, datum, frame_metric, frame_metric_data};
use crate::interval_validation::create_group;
use crate::profile_times::{place_profile, place_receipts, place_receipts_at};
use crate::profiling_group_lifecycle::{
    Harness, aggregation_of, data_of, element_of, iso_millis, profile_data,
};

/// The media types the model's `GetProfile$accept` doc names: "application/json —
/// standard JSON format" and "application/x-amzn-ion — the Amazon Ion data format".
const JSON: &str = "application/json";
const ION: &str = "application/x-amzn-ion";

/// 2020-06-01T00:00:00Z: a whole number of days past the epoch, so an aggregation window
/// aligned to UTC starts here for each of the three periods, and long enough past that the
/// aggregation every placed profile is owed has fallen due.
const DAY_START: i64 = 1_590_969_600;

/// Five minutes in seconds — the narrowest aggregation period the model's
/// `AggregationPeriod` declares ("PT5M — 5 minutes"), so receipts this far apart sit in
/// different windows.
const FIVE_MINUTES: i64 = 5 * 60;

/// One hour in seconds.
const HOUR: i64 = 60 * 60;

/// An instant as the ISO 8601 `date-time` form the modelled query members carry.
fn iso(at: i64) -> String {
    DateTime::from_secs(at)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("unformattable instant {at}: {error}"))
}

/// Assert what one served aggregated profile states: the profiling DATA it holds (the
/// data of `expected`), the window it covers (`window`, epoch seconds) and how many
/// agents reported into that window.
fn assert_aggregated(
    case: &str,
    body: &[u8],
    expected: &serde_json::Value,
    window: (i64, i64),
    reported_agents: i64,
) {
    assert_eq!(
        profile_data(body),
        data_of(&expected_element(expected)),
        "case {case:?}: expected the data of {expected}, got {}",
        element_of(body)
    );
    let (start, end, agents) = aggregation_of(case, body);
    assert_eq!(
        start,
        iso_millis(window.0),
        "case {case:?}: expected the window to start at {}, got {}",
        iso_millis(window.0),
        element_of(body)
    );
    assert_eq!(
        end,
        iso_millis(window.1),
        "case {case:?}: expected the window to end at {}, got {}",
        iso_millis(window.1),
        element_of(body)
    );
    assert_eq!(
        agents, reported_agents,
        "case {case:?}: expected {reported_agents} reported agents, got {}",
        element_of(body)
    );
}

/// The expectation `document` states, in the one form a served document is compared in
/// ([`element_of`], the shared reader every suite in this target reads one through).
fn expected_element(document: &serde_json::Value) -> Element {
    element_of(&serde_json::to_vec(document).expect("a serializable document"))
}

/// `document` as the bytes an agent posts a profile as, in one of the two representations
/// this service serves.
fn posted_bytes(document: &serde_json::Value, representation: &str) -> Vec<u8> {
    let json = serde_json::to_vec(document).expect("a serializable document");
    if representation == ION {
        expected_element(document)
            .encode_as(v1_0::Binary)
            .expect("an encodable Ion document")
    } else {
        json
    }
}

/// One profile the group is posted: the document it states and the representation its
/// bytes are written in.
struct Posted {
    document: serde_json::Value,
    representation: &'static str,
}

/// One row: the profiles posted into one aggregation window, oldest first, and the
/// aggregated profile a request over that window must serve.
struct Row {
    label: &'static str,
    posted: Vec<Posted>,
    expected: serde_json::Value,
}

/// A profile stating its `start` and the call graph `callgraph`, which is the document
/// shape this service's profile readers walk (a frame is a container keyed by its name and
/// the numbers it carries are the samples observed for it).
fn profile(start: i64, callgraph: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "start": start, "callgraph": callgraph })
}

fn json_profile(start: i64, callgraph: serde_json::Value) -> Posted {
    Posted {
        document: profile(start, callgraph),
        representation: JSON,
    }
}

fn ion_profile(start: i64, callgraph: serde_json::Value) -> Posted {
    Posted {
        document: profile(start, callgraph),
        representation: ION,
    }
}

/// The table. Each row posts its profiles into ONE aggregation window and states the one
/// aggregated profile that window holds.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "two profiles recording different frames",
            posted: vec![
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
                json_profile(1_000, serde_json::json!({ "B": { "RUNNABLE": 3 } })),
            ],
            expected: profile(
                1_000,
                serde_json::json!({ "A": { "RUNNABLE": 1 }, "B": { "RUNNABLE": 3 } }),
            ),
        },
        Row {
            label: "two profiles recording the same frame",
            posted: vec![
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 4 } })),
            ],
            expected: profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 5 } })),
        },
        Row {
            label: "three profiles, one frame shared by two of them",
            posted: vec![
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 2 } })),
                json_profile(1_000, serde_json::json!({ "C": { "BLOCKED": 7 } })),
            ],
            expected: profile(
                1_000,
                serde_json::json!({ "A": { "RUNNABLE": 3 }, "C": { "BLOCKED": 7 } }),
            ),
        },
        Row {
            label: "frames nested under a shared caller",
            posted: vec![
                json_profile(
                    1_000,
                    serde_json::json!({ "A": { "RUNNABLE": 1, "B": { "RUNNABLE": 2 } } }),
                ),
                json_profile(
                    1_000,
                    serde_json::json!({ "A": { "RUNNABLE": 4, "D": { "WAITING": 1 } } }),
                ),
            ],
            expected: profile(
                1_000,
                serde_json::json!({
                    "A": { "RUNNABLE": 5, "B": { "RUNNABLE": 2 }, "D": { "WAITING": 1 } }
                }),
            ),
        },
        Row {
            label: "frames listed rather than keyed",
            posted: vec![
                json_profile(
                    1_000,
                    serde_json::json!({ "frames": [{ "A": { "RUNNABLE": 1 } }] }),
                ),
                json_profile(
                    1_000,
                    serde_json::json!({ "frames": [{ "B": { "RUNNABLE": 2 } }] }),
                ),
            ],
            expected: profile(
                1_000,
                serde_json::json!({
                    "frames": [{ "A": { "RUNNABLE": 1 } }, { "B": { "RUNNABLE": 2 } }]
                }),
            ),
        },
        Row {
            label: "fractional counts",
            posted: vec![
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 0.5 } })),
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 0.25 } })),
            ],
            expected: profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 0.75 } })),
        },
        Row {
            // The combination covers the periods of every profile it combines, so it
            // starts at the earliest of their starts, whichever profile was posted first.
            label: "the later profile states the earlier start",
            posted: vec![
                json_profile(2_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
            ],
            expected: profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 2 } })),
        },
        Row {
            label: "profiles stored in different representations",
            posted: vec![
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
                ion_profile(1_000, serde_json::json!({ "B": { "RUNNABLE": 2 } })),
            ],
            expected: profile(
                1_000,
                serde_json::json!({ "A": { "RUNNABLE": 1 }, "B": { "RUNNABLE": 2 } }),
            ),
        },
        Row {
            // A member no document can state twice: the value the most recently received
            // profile reports is the one the combination carries, and the frames of both
            // are still in it.
            label: "a member the two profiles state different text for",
            posted: vec![
                json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
                Posted {
                    document: serde_json::json!({
                        "start": 1_000,
                        "agent": "second",
                        "callgraph": { "B": { "RUNNABLE": 1 } }
                    }),
                    representation: JSON,
                },
            ],
            expected: serde_json::json!({
                "start": 1_000,
                "callgraph": { "A": { "RUNNABLE": 1 }, "B": { "RUNNABLE": 1 } },
                "agent": "second"
            }),
        },
        Row {
            // A frame of the call graph keyed with the name a profile states its period's
            // start under is a frame like any other: its counts combine, and neither
            // profile's frames under it are dropped.
            label: "a frame named like the period's start member",
            posted: vec![
                json_profile(
                    1_000,
                    serde_json::json!({ "start": { "RUNNABLE": 1, "A": { "RUNNABLE": 2 } } }),
                ),
                json_profile(
                    1_000,
                    serde_json::json!({ "start": { "RUNNABLE": 4, "B": { "RUNNABLE": 8 } } }),
                ),
            ],
            expected: profile(
                1_000,
                serde_json::json!({
                    "start": { "RUNNABLE": 5, "A": { "RUNNABLE": 2 }, "B": { "RUNNABLE": 8 } }
                }),
            ),
        },
        Row {
            // A number the DOCUMENT states is about the profile itself, not a count of
            // samples of a frame, so it does not add up: the period's start is the
            // earliest of the two, and any other such number is the one the most recently
            // received profile states.
            label: "numbers the documents state about themselves",
            posted: vec![
                Posted {
                    document: serde_json::json!({
                        "start": 2_000,
                        "end": 3_000,
                        "durationInMs": 500,
                        "callgraph": { "A": { "RUNNABLE": 1 } }
                    }),
                    representation: JSON,
                },
                Posted {
                    document: serde_json::json!({
                        "start": 1_000,
                        "end": 4_000,
                        "durationInMs": 700,
                        "callgraph": { "A": { "RUNNABLE": 2 } }
                    }),
                    representation: JSON,
                },
            ],
            expected: serde_json::json!({
                "start": 1_000,
                "end": 4_000,
                "durationInMs": 700,
                "callgraph": { "A": { "RUNNABLE": 3 } }
            }),
        },
        Row {
            label: "one profile, which is a combination of one",
            posted: vec![json_profile(
                1_000,
                serde_json::json!({ "A": { "RUNNABLE": 1 } }),
            )],
            expected: profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
        },
    ]
}

/// Post `posted` to a freshly created group and place their receipts inside the one
/// five-minute window starting at [`DAY_START`], oldest first.
async fn group_holding(harness: &Harness, name: &str, posted: &[Posted], label: &str) {
    create_group(harness, name).await;
    for (index, profile) in posted.iter().enumerate() {
        let answer = harness
            .post_agent_profile(
                name,
                profile.representation,
                Some(&format!("ptok-{name}-{index}")),
                posted_bytes(&profile.document, profile.representation),
            )
            .await;
        assert_eq!(
            answer.status,
            204,
            "case {label:?}: posting profile {index} must succeed, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
    let receipts: Vec<i64> = (0..posted.len())
        .map(|index| DAY_START + 10 + i64::try_from(index).unwrap_or(0))
        .collect();
    place_receipts(harness, name, &receipts);
}

/// The query naming the one five-minute window every row's receipts fall in.
fn window_query() -> String {
    format!(
        "?startTime={}&endTime={}",
        iso(DAY_START),
        iso(DAY_START + FIVE_MINUTES)
    )
}

#[tokio::test]
async fn an_aggregated_profile_holds_every_profile_of_the_requested_range() {
    let harness = Harness::new();

    for (index, row) in rows().iter().enumerate() {
        let label = row.label;
        let name = format!("combined-{index}");
        group_holding(&harness, &name, &row.posted, label).await;

        let answer = harness.get_profile(&name, &window_query()).await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let window = (DAY_START, DAY_START + FIVE_MINUTES);
        let agents = i64::try_from(row.posted.len()).unwrap_or(0);
        assert_aggregated(label, &answer.body, &row.expected, window, agents);

        // Serving the combination changed nothing: the next read of the same window is
        // the same aggregated profile.
        let again = harness.get_profile(&name, &window_query()).await;
        assert_eq!(
            element_of(&again.body),
            element_of(&answer.body),
            "case {label:?}: a read must not change what the next caller is served, got {}",
            element_of(&again.body)
        );
    }
}

/// The combination is what the request's `accept` and `maxDepth` are applied to: it is
/// served in either representation the caller asks for, and bounded to the depth asked
/// for, exactly as a single stored profile is.
#[tokio::test]
async fn the_combination_is_what_accept_and_max_depth_are_applied_to() {
    let harness = Harness::new();
    let name = "combined-asked";
    group_holding(
        &harness,
        name,
        &[
            json_profile(
                1_000,
                serde_json::json!({ "A": { "RUNNABLE": 1, "B": { "RUNNABLE": 2 } } }),
            ),
            json_profile(1_000, serde_json::json!({ "C": { "RUNNABLE": 4 } })),
        ],
        "accept and maxDepth",
    )
    .await;
    let whole = serde_json::json!({
        "start": 1_000,
        "callgraph": { "A": { "RUNNABLE": 1, "B": { "RUNNABLE": 2 } }, "C": { "RUNNABLE": 4 } }
    });

    // Each representation the model names, and a range that allows either.
    for (label, accept, announced) in [
        ("JSON", Some(JSON), JSON),
        ("Ion", Some(ION), ION),
        // Both profiles are stored in JSON, so that is the representation a request
        // accepting anything leaves the combination in.
        ("anything", Some("*/*"), JSON),
        ("no Accept at all", None, JSON),
    ] {
        let answer = harness
            .get_profile_accepting(name, &window_query(), accept)
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some(announced),
            "case {label:?}: expected {announced}, got {:?}",
            answer.content_type
        );
        let served = element_of(&answer.body);
        assert_eq!(
            profile_data(&answer.body),
            data_of(&expected_element(&whole)),
            "case {label:?}: expected the combination, got {served}"
        );
    }

    // `maxDepth`: "if the maxDepth is set to 2, then the aggregated profile contains
    // representations of methods A and B" — the frames of the COMBINATION nested deeper
    // than it are the ones left out, so a depth bound still sees every profile's frames
    // above it.
    for (max_depth, expected) in [
        (1i64, serde_json::json!({ "start": 1_000, "callgraph": {} })),
        (
            2,
            serde_json::json!({ "start": 1_000, "callgraph": { "A": {}, "C": {} } }),
        ),
        (
            3,
            serde_json::json!({
                "start": 1_000,
                "callgraph": { "A": { "RUNNABLE": 1, "B": {} }, "C": { "RUNNABLE": 4 } }
            }),
        ),
        (4, whole.clone()),
        (10_000, whole.clone()),
    ] {
        let answer = harness
            .get_profile(name, &format!("{}&maxDepth={max_depth}", window_query()))
            .await;
        assert_eq!(
            answer.status,
            200,
            "case depth {max_depth}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let served = element_of(&answer.body);
        assert_eq!(
            profile_data(&answer.body),
            data_of(&expected_element(&expected)),
            "case depth {max_depth}: expected the data of {expected}, got {served}"
        );
        // The depth bounds the stacks of the DATA, so the window the aggregation covers
        // is stated however shallow the profile asked for is.
        let (_, _, agents) = aggregation_of(&format!("depth {max_depth}"), &answer.body);
        assert_eq!(
            agents, 2,
            "case depth {max_depth}: both submissions must still be reported, got {served}"
        );
    }
}

/// A range covering several aggregation WINDOWS is answered with the aggregated profiles
/// of all of them combined — "CodeGuru Profiler makes a best effort to combine existing
/// aggregated profiles from the requested time range and return them as one aggregated
/// profile" — while a range covering one of those windows is answered with that window's
/// profiles alone.
#[tokio::test]
async fn profiles_of_several_windows_in_the_range_are_combined() {
    let harness = Harness::new();
    let name = "combined-windows";
    create_group(&harness, name).await;

    // One profile per window, three windows apart inside the same hour.
    let placed: [(i64, &str, i64); 3] = [
        (DAY_START + 10, "A", 1_000),
        (DAY_START + FIVE_MINUTES + 10, "B", 2_000),
        (DAY_START + 2 * FIVE_MINUTES + 10, "C", 3_000),
    ];
    for (index, (_, frame, start)) in placed.iter().enumerate() {
        let answer = harness
            .post_agent_profile(
                name,
                JSON,
                Some(&format!("ptok-windows-{index}")),
                posted_bytes(
                    &profile(*start, serde_json::json!({ *frame: { "RUNNABLE": 1 } })),
                    JSON,
                ),
            )
            .await;
        assert_eq!(
            answer.status,
            204,
            "posting the profile of window {index} must succeed, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
    let receipts: Vec<i64> = placed.iter().map(|(at, _, _)| *at).collect();
    place_receipts(&harness, name, &receipts);

    // (the case, how many windows the range covers, the aggregated profile it serves)
    let cases: [(&str, i64, serde_json::Value); 3] = [
        (
            "one window",
            1,
            profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
        ),
        (
            "two windows",
            2,
            profile(
                1_000,
                serde_json::json!({ "A": { "RUNNABLE": 1 }, "B": { "RUNNABLE": 1 } }),
            ),
        ),
        (
            "three windows",
            3,
            profile(
                1_000,
                serde_json::json!({
                    "A": { "RUNNABLE": 1 },
                    "B": { "RUNNABLE": 1 },
                    "C": { "RUNNABLE": 1 }
                }),
            ),
        ),
    ];
    for (label, windows, expected) in cases {
        let answer = harness
            .get_profile(
                name,
                &format!(
                    "?startTime={}&endTime={}",
                    iso(DAY_START),
                    iso(DAY_START + windows * FIVE_MINUTES)
                ),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let window = (DAY_START, DAY_START + windows * FIVE_MINUTES);
        assert_aggregated(label, &answer.body, &expected, window, windows);
    }
}

/// A stored profile whose bytes are a document in neither representation this service
/// reads holds no frames it can see, so it contributes none to the combination — and
/// where it is the only profile of the range, that range is still served exactly as those
/// bytes are stored.
#[tokio::test]
async fn bytes_that_are_no_document_contribute_nothing_to_the_combination() {
    let harness = Harness::new();
    let opaque = b"{\"callgraph\":".to_vec();

    // (the case, the readable profiles beside the opaque bytes, what the range serves)
    let readable = profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } }));
    for (index, (label, readable_profiles, expected)) in [
        (
            "one readable profile beside it",
            vec![readable.clone()],
            Some(readable.clone()),
        ),
        (
            "two readable profiles beside it",
            vec![
                readable.clone(),
                profile(2_000, serde_json::json!({ "B": { "RUNNABLE": 2 } })),
            ],
            Some(profile(
                1_000,
                serde_json::json!({ "A": { "RUNNABLE": 1 }, "B": { "RUNNABLE": 2 } }),
            )),
        ),
        ("no readable profile at all", Vec::new(), None),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("combined-opaque-{index}");
        create_group(&harness, &name).await;
        for (post, document) in readable_profiles.iter().enumerate() {
            let answer = harness
                .post_agent_profile(
                    &name,
                    JSON,
                    Some(&format!("ptok-opaque-{index}-{post}")),
                    posted_bytes(document, JSON),
                )
                .await;
            assert_eq!(
                answer.status,
                204,
                "case {label:?}: posting profile {post} must succeed, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }
        // Bytes that are no document cannot be SUBMITTED — `PostAgentProfile` refuses
        // data that is not an agent profile — so they are placed in the store through the
        // one shared writer, received last.
        place_receipts(
            &harness,
            &name,
            &(0..readable_profiles.len())
                .map(|post| DAY_START + 10 + i64::try_from(post).unwrap_or(0))
                .collect::<Vec<i64>>(),
        );
        place_profile(
            &harness,
            &name,
            JSON,
            opaque.clone(),
            DAY_START + 10 + i64::try_from(readable_profiles.len()).unwrap_or(0),
        );

        let answer = harness.get_profile(&name, &window_query()).await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        match expected {
            Some(expected) => assert_aggregated(
                label,
                &answer.body,
                &expected,
                (DAY_START, DAY_START + FIVE_MINUTES),
                // The opaque bytes were reported by an agent too, so the window's
                // submissions are the readable ones and that one.
                i64::try_from(readable_profiles.len() + 1).unwrap_or(0),
            ),
            None => assert_eq!(
                answer.body, opaque,
                "case {label:?}: expected the stored bytes, got {:?}",
                String::from_utf8_lossy(&answer.body)
            ),
        }
    }
}

/// The two readers of a group's aggregated data agree about what a window holds: the
/// frame metric `BatchGetFrameMetricData` reports for a window is the ratio the aggregated
/// profile `GetProfile` serves for that same window records, so a frame that appears in
/// only one of the window's profiles is in both answers.
#[tokio::test]
async fn the_served_combination_records_the_counts_a_frame_metric_reports() {
    let harness = Harness::new();
    let name = "combined-metric";
    group_holding(
        &harness,
        name,
        &[
            json_profile(1_000, serde_json::json!({ "A": { "RUNNABLE": 1 } })),
            json_profile(1_000, serde_json::json!({ "B": { "RUNNABLE": 3 } })),
        ],
        "frame metric",
    )
    .await;

    let served = element_of(&harness.get_profile(name, &window_query()).await.body);
    for (frame, expected) in [("A", 0.25), ("B", 0.75)] {
        // What the served document records for the frame, read the way the metric's own
        // definition reads it: the counts under that frame over the counts of the whole
        // profile.
        let under = counts_under(&served, frame);
        let whole = counts_under(&served, "callgraph");
        assert!(
            whole > 0.0,
            "frame {frame}: the served combination must record counts, got {served}"
        );
        assert!(
            (under / whole - expected).abs() < TOLERANCE,
            "frame {frame}: the served combination must record {expected}, got {} in \
             {served}",
            under / whole
        );

        let requested = frame_metric(frame, &["RUNNABLE"]);
        let answer = frame_metric_data(
            &harness,
            name,
            &format!(
                "?startTime={}&endTime={}&targetResolution=PT1H",
                iso(DAY_START - DAY_START.rem_euclid(HOUR)),
                iso(DAY_START - DAY_START.rem_euclid(HOUR) + HOUR)
            ),
            std::slice::from_ref(&requested),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "frame {frame}: the metric must answer 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let (_, values) = datum(&answer, 0);
        assert_eq!(
            values.len(),
            1,
            "frame {frame}: expected one value per step, got {values:?}"
        );
        assert_value(
            &values,
            0,
            Some(under / whole),
            &format!("frame {frame}"),
        );
    }
}

/// The `RUNNABLE` counts `document` records inside the frame named `frame`, summed over
/// every frame of that name — the counts half of the metric's own definition, computed
/// here from the SERVED document so the two answers are compared rather than restated.
fn counts_under(document: &Element, frame: &str) -> f64 {
    fn walk(node: &Element, frame: &str, inside: bool, total: &mut f64) {
        match node.value() {
            ion_rs::Value::Struct(fields) => {
                for (name, value) in fields.fields() {
                    let name = name.text().unwrap_or_default();
                    match value.value() {
                        ion_rs::Value::Int(count) if name == "RUNNABLE" => {
                            if inside {
                                *total += count.expect_i64().unwrap_or_default() as f64;
                            }
                        }
                        _ => walk(value, frame, inside || name == frame, total),
                    }
                }
            }
            ion_rs::Value::List(items) | ion_rs::Value::SExp(items) => {
                for item in items.elements() {
                    walk(item, frame, inside, total);
                }
            }
            _ => {}
        }
    }
    let mut total = 0.0;
    walk(document, frame, false, &mut total);
    total
}

/// The epoch-millisecond `start` every submission of the window table below states for
/// itself: one instant, far from any window those cases place data in, so a served
/// `start` can never be this member carried over from an upload.
const SUBMITTED_START_MILLIS: i64 = 1_000;

/// One window case: the instants the group's data covers, how the request names the
/// window, and the window the served aggregated profile must then state.
struct WindowCase {
    label: &'static str,
    /// The instants (epoch seconds) the group's profiles cover, oldest first — one posted
    /// profile each.
    data_at: Vec<i64>,
    /// The `GetProfile` query naming the window asked for.
    query: String,
    /// The window the answer must state it covers, as (start, end) in epoch seconds.
    window: (i64, i64),
    /// How many agents must be reported as having reported into that window.
    reported_agents: i64,
}

/// The window table. Rows differ along how the request names the window (each of the
/// three aggregation periods as a length, a length that is none of them, a sub-second
/// one, a pair of bounds inside one window and a pair across two), how many submissions
/// the window holds, and where the window sits (an epoch-aligned boundary, part-way in,
/// before the epoch).
fn window_cases() -> Vec<WindowCase> {
    vec![
        WindowCase {
            label: "a five-minute length, data on a boundary every period shares",
            data_at: vec![DAY_START],
            query: "?period=PT5M".to_string(),
            window: (DAY_START, DAY_START + FIVE_MINUTES),
            reported_agents: 1,
        },
        WindowCase {
            label: "a five-minute length, data part-way into its window",
            data_at: vec![DAY_START + 42],
            query: "?period=PT5M".to_string(),
            window: (DAY_START, DAY_START + FIVE_MINUTES),
            reported_agents: 1,
        },
        WindowCase {
            label: "two submissions in one five-minute window",
            data_at: vec![DAY_START + 10, DAY_START + 200],
            query: "?period=PT5M".to_string(),
            window: (DAY_START, DAY_START + FIVE_MINUTES),
            reported_agents: 2,
        },
        WindowCase {
            label: "a five-minute length over data in two windows, the later one served",
            data_at: vec![
                DAY_START + 10,
                DAY_START + FIVE_MINUTES + 10,
                DAY_START + FIVE_MINUTES + 20,
            ],
            query: "?period=PT5M".to_string(),
            window: (DAY_START + FIVE_MINUTES, DAY_START + 2 * FIVE_MINUTES),
            reported_agents: 2,
        },
        WindowCase {
            label: "an hour-long length over the same two windows",
            data_at: vec![DAY_START + 10, DAY_START + FIVE_MINUTES + 10],
            query: "?period=PT1H".to_string(),
            window: (DAY_START, DAY_START + HOUR),
            reported_agents: 2,
        },
        WindowCase {
            label: "a day-long length",
            data_at: vec![DAY_START + 10, DAY_START + HOUR],
            query: "?period=P1D".to_string(),
            window: (DAY_START, DAY_START + 24 * HOUR),
            reported_agents: 2,
        },
        WindowCase {
            // A length the service aggregates at none of its periods still names how long
            // the aggregated profile asked for is, and the window of that length is
            // aligned to UTC like every other.
            label: "a length that is no aggregation period",
            data_at: vec![DAY_START + 42],
            query: "?period=PT7M".to_string(),
            window: (
                aligned(DAY_START + 42, 7 * 60),
                aligned(DAY_START + 42, 7 * 60) + 7 * 60,
            ),
            reported_agents: 1,
        },
        WindowCase {
            label: "a sub-second length",
            data_at: vec![DAY_START + 42],
            query: "?period=PT1S".to_string(),
            window: (DAY_START + 42, DAY_START + 43),
            reported_agents: 1,
        },
        WindowCase {
            // The operation's doc: "if the requested time range is from 00:00 to 00:20,
            // and the existing aggregated profiles are from 00:15 and 00:25, then the
            // aggregated profiles from 00:15 to 00:20 are returned" — the data's window
            // start, and the request's bound where the data's window runs past it.
            label: "a pair of bounds ending inside the window the data covers",
            data_at: vec![DAY_START + 10],
            query: format!(
                "?startTime={}&endTime={}",
                iso(DAY_START),
                iso(DAY_START + 200)
            ),
            window: (DAY_START, DAY_START + 200),
            reported_agents: 1,
        },
        WindowCase {
            label: "a pair of bounds across two windows the data covers",
            data_at: vec![DAY_START + 10, DAY_START + FIVE_MINUTES + 10],
            query: format!(
                "?startTime={}&endTime={}",
                iso(DAY_START),
                iso(DAY_START + 2 * FIVE_MINUTES)
            ),
            window: (DAY_START, DAY_START + 2 * FIVE_MINUTES),
            reported_agents: 2,
        },
        WindowCase {
            label: "a window before the epoch",
            data_at: vec![-FIVE_MINUTES + 7],
            query: "?period=PT5M".to_string(),
            window: (-FIVE_MINUTES, 0),
            reported_agents: 1,
        },
    ]
}

/// The start of the `span`-long window, aligned to the UTC epoch, that contains the
/// instant `at` (both in epoch seconds) — the alignment the `GetProfile` doc states for
/// every aggregation period ("aligned to UTC"), computed here so a row states the window
/// it expects rather than a number copied out of the service.
fn aligned(at: i64, span: i64) -> i64 {
    at.div_euclid(span) * span
}

/// The frame names the call graph of `document` records, so a case can count the
/// submissions whose data reached the answer.
fn frame_names(document: &Element) -> Vec<String> {
    document
        .as_struct()
        .and_then(|fields| fields.get("callgraph"))
        .and_then(Element::as_struct)
        .map(|callgraph| {
            callgraph
                .fields()
                .filter_map(|(name, _)| name.text().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `text` states an instant the way this service states one: the ISO 8601 UTC
/// `date-time` form with the millisecond field, `YYYY-MM-DDTHH:MM:SS.mmmZ` — the form the
/// service's own time-member docs give ("This is specified using the ISO 8601 format. For
/// example, 2020-06-01T13:15:02.001Z").
fn is_iso_millis(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 24
        && bytes.last() == Some(&b'Z')
        && bytes.get(19) == Some(&b'.')
        && bytes
            .iter()
            .skip(20)
            .take(3)
            .all(|digit| digit.is_ascii_digit())
        && DateTime::from_str(text, Format::DateTime).is_ok()
}

/// A freshly created group holding one posted profile per instant in `data_at`, each
/// covering that instant.
///
/// Every submission states the same epoch-millisecond `start`
/// ([`SUBMITTED_START_MILLIS`]) and one frame of its own, so what the answer states about
/// the window it covers can be nothing the uploads stated; the data each profile covers
/// is placed through the one shared writer, which is also how a case places data before
/// the epoch (the clock cannot be asked for such an instant).
async fn group_covering(harness: &Harness, name: &str, data_at: &[i64], label: &str) {
    create_group(harness, name).await;
    for (index, _) in data_at.iter().enumerate() {
        let document = serde_json::json!({
            "start": SUBMITTED_START_MILLIS,
            "callgraph": { format!("F{index}"): { "RUNNABLE": 1 } },
        });
        let answer = harness
            .post_agent_profile(
                name,
                JSON,
                Some(&format!("ptok-{name}-{index}")),
                posted_bytes(&document, JSON),
            )
            .await;
        assert_eq!(
            answer.status,
            204,
            "case {label:?}: posting profile {index} must succeed, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
    let placed: Vec<(i64, u32)> = data_at.iter().map(|at| (*at, 0)).collect();
    place_receipts_at(harness, name, &placed);
}

/// The aggregated profile states the WINDOW it covers and what the aggregation holds, in
/// either representation the request accepts: `start` and `end` are that window's bounds
/// as ISO 8601 UTC instants with the millisecond field, and
/// `aggregatedMetadata.numReportedAgents` counts the agents that reported into it —
/// derived from the window served and the submissions it holds, never the
/// epoch-millisecond bounds the submitted documents state for themselves.
#[tokio::test]
async fn the_aggregated_profile_states_the_window_it_covers_and_its_reported_agents() {
    let harness = Harness::new();

    for (index, case) in window_cases().iter().enumerate() {
        let label = case.label;
        let name = format!("aggregated-window-{index}");
        group_covering(&harness, &name, &case.data_at, label).await;

        // Both representations the model's `accept` names, and a request that names
        // neither: the document the service states is the same one in each.
        for (asked, accept) in [
            ("no Accept at all", None),
            ("JSON", Some(JSON)),
            ("Ion", Some(ION)),
        ] {
            let case_name = format!("{label:?} accepting {asked}");
            let answer = harness
                .get_profile_accepting(&name, &case.query, accept)
                .await;
            assert_eq!(
                answer.status,
                200,
                "case {case_name}: expected HTTP 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let (start, end, reported_agents) = aggregation_of(&case_name, &answer.body);
            let served = element_of(&answer.body);
            assert!(
                is_iso_millis(&start) && is_iso_millis(&end),
                "case {case_name}: expected ISO 8601 instants with the millisecond field, \
                 got {start:?} and {end:?} in {served}"
            );
            assert_eq!(
                (start.as_str(), end.as_str()),
                (
                    iso_millis(case.window.0).as_str(),
                    iso_millis(case.window.1).as_str()
                ),
                "case {case_name}: expected the window [{}, {}), got {served}",
                iso_millis(case.window.0),
                iso_millis(case.window.1)
            );
            assert_eq!(
                reported_agents, case.reported_agents,
                "case {case_name}: expected {} reported agents, got {served}",
                case.reported_agents
            );
            // The data of every submission the window holds is in the answer: each
            // submission records one frame of its own, so the combination records one
            // frame per reported agent.
            let data = profile_data(&answer.body);
            let frames = frame_names(&data);
            assert_eq!(
                i64::try_from(frames.len()).unwrap_or(0),
                case.reported_agents,
                "case {case_name}: expected one frame per reported agent, got {frames:?} \
                 in {data}"
            );
            assert!(
                !start.contains(&SUBMITTED_START_MILLIS.to_string()),
                "case {case_name}: the window must not be the start the uploads stated, \
                 got {served}"
            );
        }
    }
}

/// One submitted-period case: the epoch-millisecond `start` the submission states, and
/// what the service must then do with it.
struct SubmittedPeriod {
    label: &'static str,
    /// The `start` the submitted document states, as the JSON value it carries — a number
    /// of epoch milliseconds, or a value that states no instant at all.
    stated: serde_json::Value,
    /// The window the served aggregated profile must state (epoch seconds), or `None`
    /// when the period stated has not closed, so the group has no aggregated profile yet
    /// and the read is the operation's 404.
    window: Option<(i64, i64)>,
}

/// The submitted-period table, computed against the clock the posts are stamped with:
/// every row's stated period sits in a DIFFERENT window from the upload's own, so what
/// the service does with it cannot follow from the receipt.
///
/// `now` is the clock instant in epoch seconds, and `current` the five-minute window it
/// falls in — the window every one of these uploads is RECEIVED in. Rows differ in how
/// far the stated period is from that window and on which side of it, in the era of the
/// instant stated (this hour, hours back, days back, before the epoch), in whether the
/// number carries a fraction of a millisecond, and in whether the value states an instant
/// at all.
fn submitted_periods(now: i64) -> Vec<SubmittedPeriod> {
    let current = aligned(now, FIVE_MINUTES);
    let closed = current - 2 * FIVE_MINUTES;
    let hours_back = aligned(now - 3 * HOUR, FIVE_MINUTES);
    let days_back = aligned(now - 2 * 24 * HOUR, FIVE_MINUTES);
    vec![
        SubmittedPeriod {
            label: "a window two periods before the one the upload arrives in",
            stated: serde_json::json!(closed * 1_000),
            window: Some((closed, closed + FIVE_MINUTES)),
        },
        SubmittedPeriod {
            label: "a window hours before the upload, stated part-way into it",
            stated: serde_json::json!(hours_back * 1_000 + 61_000),
            window: Some((hours_back, hours_back + FIVE_MINUTES)),
        },
        SubmittedPeriod {
            label: "a window days before the upload",
            stated: serde_json::json!(days_back * 1_000),
            window: Some((days_back, days_back + FIVE_MINUTES)),
        },
        SubmittedPeriod {
            label: "a period stated with a fraction of a millisecond",
            stated: serde_json::json!((closed * 1_000) as f64 + 0.5),
            window: Some((closed, closed + FIVE_MINUTES)),
        },
        SubmittedPeriod {
            label: "a period before the epoch",
            stated: serde_json::json!(-FIVE_MINUTES * 1_000 + 7_000),
            window: Some((-FIVE_MINUTES, 0)),
        },
        SubmittedPeriod {
            // The service is still collecting into a window that has not closed, so a
            // submission covering one is in no aggregated profile however long ago it
            // arrived — the upload's own window has nothing to do with it.
            label: "a period that has not closed",
            stated: serde_json::json!((current + FIVE_MINUTES) * 1_000),
            window: None,
        },
        SubmittedPeriod {
            // A `start` that states no instant leaves the service nothing to place the
            // data by, so the data is placed where the upload arrived: in the window the
            // service is still collecting into, which is covered by no aggregated profile
            // yet.
            label: "a start that states no instant at all",
            stated: serde_json::json!("not an instant"),
            window: None,
        },
        SubmittedPeriod {
            label: "a start that states a container instead of an instant",
            stated: serde_json::json!({ "epochMillis": 1_000 }),
            window: None,
        },
    ]
}

/// A posted agent profile is collected into the aligned window of the period IT STATES,
/// not of the instant its upload arrived: a submission whose stated period sits in a
/// window that has already closed is aggregated once the pass over it has run and the
/// aggregated profile served states THAT window, while one covering a window the service
/// is still collecting into is covered by no aggregated profile however long the pass has
/// had to run — and a `start` that states no instant leaves the data where the upload
/// arrived.
///
/// The pass runs after the submission ARRIVES as well as after its window closes, so every
/// row is pending at the instant its post returns; only the rows whose stated window has
/// closed become retrievable once that pass is through.
///
/// Every row posts over HTTP and nothing but the pass is let run, so what the service
/// serves follows from the submitted document and the clock alone.
#[tokio::test]
async fn a_submission_is_collected_into_the_window_the_period_it_states_falls_in() {
    let harness = Harness::new();
    let now = DateTime::from(std::time::SystemTime::now()).secs();
    let current = aligned(now, FIVE_MINUTES);

    for (index, case) in submitted_periods(now).iter().enumerate() {
        let label = case.label;
        let name = format!("submitted-period-{index}");
        create_group(&harness, &name).await;
        let document = serde_json::json!({
            "start": case.stated,
            "callgraph": { "A": { "RUNNABLE": 1 } },
        });
        let posted = harness
            .post_agent_profile(&name, JSON, None, posted_bytes(&document, JSON))
            .await;
        assert_eq!(
            posted.status,
            204,
            "case {label:?}: posting the profile must succeed, got {} {}",
            posted.status,
            String::from_utf8_lossy(&posted.body)
        );

        // Whatever window the submission states, the pass that would collect it has not
        // run yet at the instant the post returns.
        let submitted = harness.get_profile(&name, "?period=PT5M").await;
        assert_eq!(
            submitted.status,
            404,
            "case {label:?}: a submission the aggregation pass has not run over yet must \
             not be served, got {} {}",
            submitted.status,
            String::from_utf8_lossy(&submitted.body)
        );

        harness.complete_aggregation_pass(&name);
        let answer = harness.get_profile(&name, "?period=PT5M").await;
        match case.window {
            Some(window) => {
                assert_eq!(
                    answer.status,
                    200,
                    "case {label:?}: the aggregated profile of a closed period must be \
                     served once the pass over it has run, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                let (start, end, reported_agents) = aggregation_of(label, &answer.body);
                let served = element_of(&answer.body);
                assert_eq!(
                    (start.as_str(), end.as_str()),
                    (iso_millis(window.0).as_str(), iso_millis(window.1).as_str()),
                    "case {label:?}: expected the window [{}, {}) the submission states, \
                     got {served}",
                    iso_millis(window.0),
                    iso_millis(window.1)
                );
                // The window the UPLOAD arrived in is a different one, so a service that
                // placed the data by its receipt would state that window instead.
                assert_ne!(
                    start,
                    iso_millis(current),
                    "case {label:?}: the window must be the one the submission states, \
                     not the one the upload arrived in, got {served}"
                );
                assert_eq!(
                    reported_agents, 1,
                    "case {label:?}: one agent reported into that window, got {served}"
                );
                assert_eq!(
                    profile_data(&answer.body),
                    data_of(&expected_element(&document)),
                    "case {label:?}: expected the submitted data, got {served}"
                );
            }
            None => {
                assert_eq!(
                    answer.status,
                    404,
                    "case {label:?}: a period the service is still collecting into is \
                     covered by no aggregated profile, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                assert_eq!(
                    answer.error_code(),
                    "ResourceNotFoundException",
                    "case {label:?}: expected ResourceNotFoundException, got {}",
                    String::from_utf8_lossy(&answer.body)
                );
                // The submission WAS stored — the group records the receipt — so the 404
                // is about the pending aggregation and not about a refused post.
                assert!(
                    harness
                        .describe(&name)
                        .await
                        .json()
                        .pointer("/profilingStatus/latestAgentProfileReportedAt")
                        .is_some(),
                    "case {label:?}: the post must be recorded as received"
                );
            }
        }
    }
}
