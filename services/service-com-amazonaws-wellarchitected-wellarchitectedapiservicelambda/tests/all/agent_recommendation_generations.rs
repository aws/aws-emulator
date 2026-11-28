// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for an agent profile's recommendation generations:
//! `StartAgentRecommendationGeneration`, which records one, the
//! `GetAgentRecommendationGeneration` that reports it, and the
//! `ListAgentRecommendationGenerations` that pages over them.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. `GetAgentRecommendationGeneration` REPORTS the generation the start recorded: the
//!    minted `UUID`, the `profileArn` it was started against, the `name`, `scope` and
//!    `additionalContext` as sent, and the `createdBy` / `createdAt` the service owns.
//! 2. A generation is a JOB whose status is owed by the CLOCK: it is `QUEUED` until its
//!    analysis is due to start, `IN_PROGRESS` until it is due to finish and then the
//!    terminal `COMPLETED`, with `startedAt` / `endedAt` appearing exactly when those
//!    instants pass and `lastModifiedAt` reporting the latest transition that happened.
//!    Both spans are non-zero, so a caller polling straight after the start call still
//!    observes `QUEUED`.
//! 3. `progress` agrees with the scope and the status: `totalSteps` is how many pillars
//!    the generation's scope names, `stepsCompleted` grows from 0 to that total and
//!    `completionPercentage` is that same share as a percentage — 0 while queued and 100
//!    once complete.
//! 4. Reading a generation changes nothing a later caller sees: repeated polls report the
//!    same identity, scope and instants, and the status only ever moves forwards.
//! 5. A generation is reached only THROUGH its profile: an id that belongs to another
//!    profile, or to none, is a `404 ResourceNotFoundException` naming the composite
//!    resource id and the missing resource's type, and a value the modelled `UUID` /
//!    `AgentProfileArn` pattern refuses never reaches the lookup — the boundary refuses it
//!    `400 ValidationException`.
//! 6. `StartAgentRecommendationGeneration` is modelled `@idempotent`: repeating a start
//!    request while the generation it started is still in flight answers THAT generation
//!    and records no second one, while a request differing in any member starts its own.
//! 7. A start the service cannot serve is refused `400 ValidationException` and records
//!    nothing: a `profileArn` that names no stored profile, a profile that names no
//!    aggregation scope to analyse (the derived `eligibleFor…` answer `GetAgentProfile`
//!    reports), a `scope.goalIds` entry that names no goal of that profile, and a value
//!    outside a published enum's set.
//! 8. `additionalContext` is an untyped `Document`, so its nesting is caller-chosen: a
//!    document nested to the ceiling this service holds documents to
//!    ([`state::MAX_DOCUMENT_DEPTH`]) round-trips as sent, and one past it is refused
//!    `400 ValidationException` rather than walked.
//! 9. A generation does not outlive the profile it is reached through: deleting the
//!    profile takes its generations with it, which is what keeps the per-profile ceiling
//!    a real bound on the store — an `AgentProfileArn` is never minted twice, so a
//!    generation left under a deleted profile's ARN could never be reclaimed.
//! 10. `ListAgentRecommendationGenerations` reports the generations of ONE profile, in
//!     one stable id order: another profile's generations never appear, a profile that
//!     holds none reports an empty `items`, and a well-formed `profileArn` that names no
//!     stored profile is this operation's modelled `404 ResourceNotFoundException`
//!     (unlike `ListAgentContexts`, which models no such error and so reports an empty
//!     page). The path resource is resolved BEFORE the query members are judged.
//! 11. Its optional `RecommendationType` filter selects the generations started for that
//!     type and nothing else, for EVERY published value, and a value outside the enum's
//!     published set is refused `400 ValidationException` rather than read as "no filter".
//! 12. Paging enumerates every selected generation EXACTLY ONCE: no page is longer than
//!     the `maxResults` asked for, the ids ascend across the pages, the union of the pages
//!     is the whole selected set, and only the last page omits `nextToken` — with and
//!     without the type filter. `maxResults` is held to its published
//!     `@range(min: 1, max: 1000)`, and a `nextToken` this service did not issue is
//!     refused `400 ValidationException` with reason `CANNOT_PARSE` naming the member.
//! 13. Generations are metered PER PROFILE: a profile holding the quota refuses the next
//!     start `402 ServiceQuotaExceededException` and records nothing further, while a
//!     sibling profile still takes one.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`; the profile the
//! generations hang off, and the goals a scope may name, are minted by the operations in
//! `super::agent_goals`.

use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

// The id, list and timestamp assertions are the agent-suite helpers next door: an id the
// service minted and the `items` of a list are the same shapes here.
use super::agent_contexts::{assert_create_instant, assert_uuid, id_of, items};
use super::agent_goals::{absent_profile_arn, create_goal, profile_arn_of, PILLARS};
use super::agent_profile_lifecycle::delete_profile;
use super::workload_harness::{encode_path_segment, Answer, Client};

/// The published bounds of `ListAgentRecommendationGenerationsRequest$maxResults`
/// (`@range(min: 1, max: 1000)` — `smithy_query(query="operation",
/// operation="ListAgentRecommendationGenerations")`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 1_000;

/// The query-string names the listing binds its filter and paging members to.
///
/// They are CAPITALIZED here (`smithy_query(query="operation",
/// operation="ListAgentRecommendationGenerations")` shows `httpQuery: "MaxResults"`,
/// `"NextToken"` and `"RecommendationType"`), unlike the sibling `ListAgentContexts`,
/// which binds `maxResults` / `nextToken`: the wire name is per member, so the suite
/// sends what this operation's model says rather than one convention for the service.
const MAX_RESULTS_QUERY: &str = "MaxResults";
/// See [`MAX_RESULTS_QUERY`].
const NEXT_TOKEN_QUERY: &str = "NextToken";
/// See [`MAX_RESULTS_QUERY`].
const RECOMMENDATION_TYPE_QUERY: &str = "RecommendationType";

/// Every value the `GenerationStatus` enum publishes, in the order a generation passes
/// through them; `ERROR` is the terminal status of a FAILED generation and is not on that
/// path (`smithy_query(query="shape", shape="GenerationStatus")`). Spelled out here so the
/// suite pins the catalog rather than reading it back out of the code under test.
const GENERATION_STATUSES: [&str; 3] = ["QUEUED", "IN_PROGRESS", "COMPLETED"];

/// Every value the `RecommendationType` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="RecommendationType")`).
const RECOMMENDATION_TYPES: [&str; 3] = ["RESOURCE", "ARCHITECTURE", "APPLICATION"];

/// The published bounds of `StartAgentRecommendationGenerationRequest$types`
/// (`RecommendationTypes`, `@length(min: 1, max: 2)` with `@uniqueItems` —
/// `smithy_query(query="shape", shape="RecommendationTypes")`). A start naming the
/// maximum is served and one naming a type more is refused.
const TYPES_MAX: usize = 2;

/// `POST /api/v1/agent-profiles/{profileArn}/generations`: the ARN is a non-greedy
/// `@httpLabel`, so it travels percent-encoded in one path segment, exactly as an SDK
/// sends it.
async fn start_generation(client: &Client, profile_arn: &str, body: Value) -> Answer {
    client
        .send(
            "POST",
            &format!(
                "/api/v1/agent-profiles/{}/generations",
                encode_path_segment(profile_arn)
            ),
            Some(body),
        )
        .await
}

/// `GET /api/v1/agent-profiles/{profileArn}/generations/{generationId}`.
async fn get_generation(client: &Client, profile_arn: &str, generation_id: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/api/v1/agent-profiles/{}/generations/{}",
                encode_path_segment(profile_arn),
                encode_path_segment(generation_id)
            ),
            None,
        )
        .await
}

/// `GET /api/v1/agent-profiles/{profileArn}/generations`, with the filter and paging
/// members bound to the query string. `query` is passed as sent, empty for a request
/// that names none.
async fn list_generations(client: &Client, profile_arn: &str, query: &str) -> Answer {
    let path = format!(
        "/api/v1/agent-profiles/{}/generations",
        encode_path_segment(profile_arn)
    );
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    client.send("GET", &uri, None).await
}

/// The `status` a generation projection carries.
fn status_of(value: &Value) -> String {
    value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert `later` is at or after `earlier`. Both stamps come from the same serializer,
/// so numbers compare numerically and ISO-8601 UTC strings compare lexicographically.
fn assert_not_before(later: Option<&Value>, earlier: Option<&Value>, case: &str, label: &str) {
    match (later, earlier) {
        (Some(Value::Number(later)), Some(Value::Number(earlier))) => {
            let (later, earlier) = (
                later.as_f64().unwrap_or_default(),
                earlier.as_f64().unwrap_or_default(),
            );
            assert!(
                later >= earlier,
                "case {case:?}: {label}, got {later} before {earlier}"
            );
        }
        (Some(Value::String(later)), Some(Value::String(earlier))) => {
            assert!(
                later.as_str() >= earlier.as_str(),
                "case {case:?}: {label}, got {later:?} before {earlier:?}"
            );
        }
        (later, earlier) => panic!(
            "case {case:?}: {label}: both stamps must be present in one form, got \
             {later:?} and {earlier:?}"
        ),
    }
}

/// Assert a member is absent (or JSON null).
fn assert_absent(value: &Value, member: &str, case: &str, why: &str) {
    assert!(
        value.get(member).is_none_or(Value::is_null),
        "case {case:?}: {member} must be absent {why}, got {value:?}"
    );
}

/// The rows the listing tests span. They differ along what the rule generalises over:
/// which `RecommendationType` is asked for (every published value), whether the optional
/// `name` and `additionalContext` are present, how many pillars the `scope` names (one,
/// several, every published pillar), whether it names per-pillar `items`, and `name` at
/// each modelled `@length(min: 1, max: 128)` bound. A scope's `goalIds` must name goals
/// of the profile, which the rows of `rows()` below cover.
fn generation_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "minimal-one-pillar",
            json!({
                "types": [RECOMMENDATION_TYPES[0]],
                "scope": { "pillars": [PILLARS[1]] },
            }),
        ),
        (
            "named-two-pillars",
            json!({
                "types": [RECOMMENDATION_TYPES[1]],
                "name": "quarterly-architecture-review",
                "scope": { "pillars": [PILLARS[0], PILLARS[2]] },
            }),
        ),
        (
            "every-pillar-with-items",
            json!({
                "types": [RECOMMENDATION_TYPES[2]],
                // The modelled `@length(min: 1, max: 128)` / `^[A-Za-z0-9_-]+$` ceiling.
                "name": "n".repeat(128),
                "scope": {
                    "pillars": PILLARS,
                    "items": [
                        { "pillar": PILLARS[3], "ids": ["SEC01-BP01", "arn:aws:s3:::bucket"] },
                        { "pillar": PILLARS[4], "ids": ["lambda"] },
                    ],
                },
            }),
        ),
        (
            "additional-context-document",
            json!({
                "types": [RECOMMENDATION_TYPES[0]],
                // The modelled `@length(min: 1, …)` floor.
                "name": "x",
                "scope": { "pillars": [PILLARS[4]] },
                // `additionalContext` is a free-form `Document`, so a nested value has to
                // round-trip verbatim.
                "additionalContext": {
                    "budgetUsd": 120000,
                    "constraints": ["no downtime", "eu-only"],
                    "nested": { "approved": true, "ratio": 0.25 },
                },
            }),
        ),
    ]
}

/// Assert one generation projection reports what `request` asked for plus the members
/// the SERVICE owns, for a generation in ANY status: the clock-independent members, that
/// `status` is a published `GenerationStatus`, and that the `lastModified…` pair is
/// present exactly when the generation has settled — so this holds however long the test
/// took to get here.
fn assert_projection(value: &Value, request: &Value, profile_arn: &str, case: &str) {
    assert_uuid(&id_of(value), case);
    assert_eq!(
        value.get("profileArn").and_then(Value::as_str),
        Some(profile_arn),
        "case {case:?}: profileArn must name the profile the generation was started \
         against, got {value:?}"
    );
    assert_eq!(
        value.get("name"),
        request.get("name"),
        "case {case:?}: name must report what the request carried, got {value:?}"
    );
    let status = status_of(value);
    assert!(
        GENERATION_STATUSES.contains(&status.as_str()),
        "case {case:?}: status must be a published GenerationStatus, got {value:?}"
    );
    assert_eq!(
        value.get("createdBy").and_then(Value::as_str),
        Some(engine::emulator_account_id()),
        "case {case:?}: createdBy must be the account the emulator serves, got {value:?}"
    );
    assert_create_instant(value.get("createdAt"), case, "createdAt");
    // "The estimated time for the generation to complete" is the instant the clock owes
    // this generation its terminal status, so it is at or after the start.
    assert_create_instant(
        value.get("estimatedCompletionTime"),
        case,
        "estimatedCompletionTime",
    );
    assert_not_before(
        value.get("estimatedCompletionTime"),
        value.get("createdAt"),
        case,
        "estimatedCompletionTime must not precede createdAt",
    );
    // A transition the clock owes is the only change a generation undergoes, so the
    // `lastModified…` pair tracks the status rather than drifting from it.
    if status == state::GENERATION_QUEUED {
        for member in ["lastModifiedBy", "lastModifiedAt"] {
            assert_absent(value, member, case, "while the generation is queued");
        }
    } else {
        assert_eq!(
            value.get("lastModifiedBy").and_then(Value::as_str),
            Some(engine::emulator_account_id()),
            "case {case:?}: a generation the clock has moved is modified by the account \
             the emulator serves, got {value:?}"
        );
        assert_create_instant(value.get("lastModifiedAt"), case, "lastModifiedAt");
    }
}

/// Assert a generation is still in the transient status, with nothing reported about a
/// change to it. Called only where the test's own clock proves the queue span has not
/// passed, so it never depends on wall-clock slack.
fn assert_queued(value: &Value, case: &str) {
    assert_eq!(
        status_of(value),
        state::GENERATION_QUEUED,
        "case {case:?}: a generation whose analysis is not due yet is {}, got {value:?}",
        state::GENERATION_QUEUED
    );
    for member in ["lastModifiedBy", "lastModifiedAt", "endedAt", "errorDetails"] {
        assert_absent(value, member, case, "while the generation is queued");
    }
}

/// Whether the generation started at `started_at` is certainly still queued by the test's
/// own clock, so the strict assertions above may be made. A machine slow enough to spend
/// the whole queue span inside one round trip simply skips them rather than failing.
fn still_queued(started_at: Instant) -> bool {
    started_at.elapsed() < state::GENERATION_QUEUE_DURATION
}

/// Rule 10: every row round-trips — the start's answer, the
/// `GetAgentRecommendationGeneration` of its id and the entry
/// `ListAgentRecommendationGenerations` reports agree, and each reports what the request
/// carried plus the service-owned members.
#[tokio::test]
async fn a_started_generation_round_trips_through_get_and_list() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-host", 1).await;

    let mut started: Vec<(&str, Value, Value)> = Vec::new();
    for (case, request) in generation_rows() {
        let started_at = Instant::now();
        let answer = start_generation(&client, &profile_arn, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected StartAgentRecommendationGeneration 200, got {answer:?}"
        );
        assert_projection(&answer.body, &request, &profile_arn, case);
        // The start's own answer is computed at the instant of the call, so it is queued
        // whatever the machine does next.
        assert_queued(&answer.body, case);

        let read = get_generation(&client, &profile_arn, &id_of(&answer.body)).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetAgentRecommendationGeneration 200, got {read:?}"
        );
        assert_projection(&read.body, &request, &profile_arn, case);
        if still_queued(started_at) {
            assert_queued(&read.body, case);
        }
        // The getter reports the same projection plus the members only it carries. The
        // clock-derived members (`status`, `lastModified…`) are left to
        // `assert_projection` above: a read is free to be later than the start.
        for member in [
            "id",
            "profileArn",
            "name",
            "createdBy",
            "createdAt",
            "estimatedCompletionTime",
        ] {
            assert_eq!(
                read.body.get(member),
                answer.body.get(member),
                "case {case:?}: {member} must agree with what the start answered, got {read:?}"
            );
        }
        assert_eq!(
            read.body.get("scope"),
            request.get("scope"),
            "case {case:?}: scope must report what the request carried, got {read:?}"
        );
        assert_eq!(
            read.body.get("additionalContext"),
            request.get("additionalContext"),
            "case {case:?}: additionalContext must round-trip verbatim, got {read:?}"
        );
        started.push((case, request, answer.body));
    }

    let listed = list_generations(&client, &profile_arn, &format!("{MAX_RESULTS_QUERY}=1000")).await;
    assert_eq!(
        listed.status, 200,
        "expected ListAgentRecommendationGenerations 200, got {listed:?}"
    );
    let reported = items(&listed);
    assert_eq!(
        reported.len(),
        started.len(),
        "the list must report every stored generation, got {listed:?}"
    );
    for (case, request, _) in &started {
        let entry = reported
            .iter()
            .find(|entry| entry.get("name") == request.get("name"))
            .unwrap_or_else(|| panic!("case {case:?}: the list must report this generation, got {reported:?}"));
        assert_projection(entry, request, &profile_arn, case);
    }
    let ids: Vec<String> = reported.iter().map(id_of).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        ids, sorted,
        "the list must report generations in one stable id order, got {ids:?}"
    );
    assert_absent(
        &listed.body,
        "nextToken",
        "every-generation-on-one-page",
        "on a page that reported every generation",
    );
}

/// Rule 10 (cont.): the LISTING reports the status the clock owes, not one a caller
/// moved: a generation is `QUEUED` while its analysis is not due to start, and the
/// terminal `COMPLETED` once both owed spans have passed, with the transition attributed
/// and `errorDetails` absent — and reading it never moves it.
#[tokio::test]
async fn the_listing_reports_the_status_the_clock_owes() {
    assert!(
        !state::GENERATION_QUEUE_DURATION.is_zero() && !state::GENERATION_RUN_DURATION.is_zero(),
        "both owed spans must be non-zero so each status is observable, got {:?} and {:?}",
        state::GENERATION_QUEUE_DURATION,
        state::GENERATION_RUN_DURATION
    );

    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-settle", 1).await;

    // One row per published recommendation type, so the rule is asserted over the whole
    // class rather than for the one type the issue cited.
    let mut queued: Vec<(&str, String)> = Vec::new();
    for kind in RECOMMENDATION_TYPES {
        let started_at = Instant::now();
        let answer = start_generation(
            &client,
            &profile_arn,
            json!({
                "types": [kind],
                "name": format!("settling-{}", kind.to_lowercase()),
                "scope": { "pillars": [PILLARS[0]] },
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {kind:?}: expected StartAgentRecommendationGeneration 200, got {answer:?}"
        );
        // A caller acting straight after the start still sees the transient status: the
        // start's own answer is computed at the instant of the call.
        assert_queued(&answer.body, kind);
        let id = id_of(&answer.body);
        let listed = list_generations(&client, &profile_arn, "").await;
        // Asserted only while the test's own clock proves the queue span has not passed,
        // so a slow machine skips the strict check rather than failing it.
        if still_queued(started_at) {
            let entry = items(&listed)
                .iter()
                .find(|entry| id_of(entry) == id)
                .unwrap_or_else(|| panic!("case {kind:?}: the list must report {id}"))
                .clone();
            assert_queued(&entry, kind);
        }
        queued.push((kind, id));
    }

    // A test, not a handler: the handler never sleeps for the stored instants, it compares
    // them with the clock on every read. `std::thread::sleep` rather than
    // `tokio::time::sleep` because the workspace pins tokio without its `time` feature
    // ("macros", "rt", "rt-multi-thread" only), and the suite beside this one
    // (`custom_lens_import::wait_for_the_import_to_land`) waits the same way; there is no
    // other task on this test's runtime to starve.
    std::thread::sleep(
        state::GENERATION_QUEUE_DURATION
            + state::GENERATION_RUN_DURATION
            + Duration::from_millis(250),
    );

    let listed = list_generations(&client, &profile_arn, "").await;
    assert_eq!(
        listed.status, 200,
        "expected ListAgentRecommendationGenerations 200, got {listed:?}"
    );
    assert_eq!(
        items(&listed).len(),
        queued.len(),
        "the list must report every started generation, got {listed:?}"
    );
    for (case, id) in &queued {
        let entry = items(&listed)
            .iter()
            .find(|entry| &id_of(entry) == id)
            .unwrap_or_else(|| panic!("case {case:?}: the list must report {id}, got {listed:?}"))
            .clone();
        assert_eq!(
            status_of(&entry),
            "COMPLETED",
            "case {case:?}: a generation past both owed spans is terminal, got {entry:?}"
        );
        // The transition is the SERVICE's own change to the generation, so it is
        // attributed to the account this emulator serves at the instant it was due.
        assert_eq!(
            entry.get("lastModifiedBy").and_then(Value::as_str),
            Some(engine::emulator_account_id()),
            "case {case:?}: lastModifiedBy must be the account the emulator serves, \
             got {entry:?}"
        );
        assert_create_instant(entry.get("lastModifiedAt"), case, "lastModifiedAt");
        assert_not_before(
            entry.get("lastModifiedAt"),
            entry.get("createdAt"),
            case,
            "lastModifiedAt must not precede createdAt",
        );

        // A read must not change what a later caller sees: the same read again answers the
        // same thing.
        let again = list_generations(&client, &profile_arn, "").await;
        assert_eq!(
            items(&again)
                .iter()
                .find(|other| id_of(other) == *id)
                .cloned(),
            Some(entry),
            "case {case:?}: listing a terminal generation must not change it, got {again:?}"
        );
    }
}

/// Rule 10: the list is scoped to ONE profile — a sibling profile's generations, a profile
/// holding none, and an ARN no profile carries.
#[tokio::test]
async fn generations_are_listed_per_profile() {
    let client = Client::new();
    let first = profile_arn_of(&client, "generation-host-first", 1).await;
    let second = profile_arn_of(&client, "generation-host-second", 2).await;
    let empty = profile_arn_of(&client, "generation-host-empty", 1).await;

    // A different number of generations under each profile, so a handler that ignored the
    // ARN could not pass by accident.
    let mut expected: Vec<(&str, Vec<String>)> = Vec::new();
    for (case, profile_arn, count) in [("first", &first, 1usize), ("second", &second, 3)] {
        let mut ids = Vec::new();
        for index in 0..count {
            let answer = start_generation(
                &client,
                profile_arn,
                json!({
                    "types": [RECOMMENDATION_TYPES[index % RECOMMENDATION_TYPES.len()]],
                    "name": format!("{case}-generation-{index}"),
                    "scope": { "pillars": [PILLARS[index % PILLARS.len()]] },
                }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected StartAgentRecommendationGeneration 200, got {answer:?}"
            );
            ids.push(id_of(&answer.body));
        }
        ids.sort();
        expected.push((case, ids));
    }

    for ((case, ids), profile_arn) in expected.iter().zip([&first, &second]) {
        let answer = list_generations(&client, profile_arn, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListAgentRecommendationGenerations 200, got {answer:?}"
        );
        let reported: Vec<String> = items(&answer).iter().map(id_of).collect();
        assert_eq!(
            &reported, ids,
            "case {case:?}: the list must report exactly this profile's generations, \
             got {answer:?}"
        );
    }

    // A profile that holds none reports an empty page; a generation id that belongs to
    // ANOTHER profile is as absent as one that belongs to none.
    let answer = list_generations(&client, &empty, "").await;
    assert_eq!(
        answer.status, 200,
        "case \"profile-with-no-generations\": expected 200, got {answer:?}"
    );
    assert!(
        items(&answer).is_empty(),
        "case \"profile-with-no-generations\": expected an empty items list, got {answer:?}"
    );
    let borrowed = expected[0].1[0].clone();
    let crossed = get_generation(&client, &second, &borrowed).await;
    assert_eq!(
        (crossed.status, crossed.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"another-profiles-generation\": a generation is reached only through its \
         own profile, got {crossed:?}"
    );

    // An ARN that names no stored profile: the listing DOES model
    // `ResourceNotFoundException`, so it is a 404 rather than an empty page. The missing
    // resource differs by operation: the listing resolves the PROFILE the path names,
    // while the single-generation read resolves the (profile, generation) pair in one
    // lookup and so reports the generation that is missing.
    for (case, profile_arn, resource_type) in [
        (
            "list",
            absent_profile_arn("absent-for-list"),
            state::AGENT_PROFILE_RESOURCE_TYPE,
        ),
        (
            "get",
            absent_profile_arn("absent-for-get"),
            state::AGENT_GENERATION_RESOURCE_TYPE,
        ),
    ] {
        let answer = if case == "list" {
            list_generations(&client, &profile_arn, "").await
        } else {
            get_generation(&client, &profile_arn, &borrowed).await
        };
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an ARN that names no profile must be a 404, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ResourceType").and_then(Value::as_str),
            Some(resource_type),
            "case {case:?}: the 404 must name the resource that was missing, got {answer:?}"
        );
    }
}

/// The generation id a successful start answered with, checked against the modelled
/// `UUID` pattern (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
fn started_id(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected StartAgentRecommendationGeneration 200, got {answer:?}"
    );
    let id = answer
        .body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let groups: Vec<&str> = id.split('-').collect();
    assert!(
        groups.len() == 5
            && [8usize, 4, 4, 4, 12]
                .iter()
                .zip(&groups)
                .all(|(len, group)| group.len() == *len
                    && group.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))),
        "case {case:?}: the generation id must match the modelled UUID pattern, got {id:?}"
    );
    id
}

/// A timestamp this operation reports, in epoch seconds.
///
/// Every `Timestamp` of these two shapes carries `@timestampFormat("date-time")`
/// (`smithy_query(query="operation", operation="GetAgentRecommendationGeneration")`), so
/// it travels as an RFC 3339 string; a number is accepted too, so the helper pins the
/// INSTANT rather than one spelling of it.
fn instant_seconds(value: Option<&Value>, case: &str, label: &str) -> f64 {
    match value {
        Some(Value::Number(number)) => number.as_f64().unwrap_or_default(),
        Some(Value::String(text)) => ::aws_smithy_types::DateTime::from_str(
            text,
            ::aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {label} must be an RFC 3339 instant, got {text:?}: {error}")
        })
        .as_secs_f64(),
        other => panic!("case {case:?}: expected a {label} timestamp, got {other:?}"),
    }
}

/// The test's own clock in epoch seconds — what a reported instant is checked against,
/// since no two runs share a recorded one.
fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch")
        .as_secs_f64()
}

/// The status the clock owes a generation at `instant`, from the two instants the start
/// answered: `QUEUED` until its analysis is due to start (`createdAt` plus the queue
/// span), `IN_PROGRESS` until it is due to finish (`estimatedCompletionTime`), then the
/// terminal `COMPLETED`.
fn status_at(created_at: f64, estimated: f64, instant: f64) -> &'static str {
    if instant < created_at + state::GENERATION_QUEUE_DURATION.as_secs_f64() {
        "QUEUED"
    } else if instant < estimated {
        "IN_PROGRESS"
    } else {
        "COMPLETED"
    }
}

/// The statuses a read taken somewhere between `before` and `after` may report: the
/// contiguous span the clock passed through while the request was in flight. A read is
/// never asserted against the status at ONE instant, because the request takes time to
/// serve — but it must report a status the clock really owed while it was being served.
fn statuses_between(created_at: f64, estimated: f64, before: f64, after: f64) -> Vec<&'static str> {
    let index = |instant| {
        GENERATION_STATUSES
            .iter()
            .position(|published| *published == status_at(created_at, estimated, instant))
            .expect("the derived status is one the enum publishes")
    };
    GENERATION_STATUSES[index(before)..=index(after)].to_vec()
}

/// Read one generation, with the statuses the clock owed while the read was in flight.
async fn read_generation(
    client: &Client,
    profile_arn: &str,
    id: &str,
    created_at: f64,
    estimated: f64,
) -> (Answer, Vec<&'static str>) {
    let before = now_seconds();
    let answer = get_generation(client, profile_arn, id).await;
    let after = now_seconds();
    (
        answer,
        statuses_between(created_at, estimated, before, after),
    )
}

/// The status a read reported, checked against the statuses the clock owed while it was
/// in flight.
fn reported_status(answer: &Answer, owed: &[&str], case: &str) -> String {
    let status = answer
        .body
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        owed.contains(&status.as_str()),
        "case {case:?}: a read taken while the clock owed one of {owed:?} must report one \
         of them, got {status:?} in {answer:?}"
    );
    status
}

/// A document nested `levels` deep: `levels` objects around one string leaf, so its
/// deepest node sits at `levels + 1`. What the document-depth bound is tested on.
fn nested_document(levels: usize) -> Value {
    let mut document = json!("leaf");
    for _ in 0..levels {
        document = json!({ "nested": document });
    }
    document
}

/// One row of the table every generation test spans: what the start request carries.
struct Row {
    case: &'static str,
    /// The pillars the generation's scope names — what `progress.totalSteps` counts.
    pillars: Vec<&'static str>,
    recommendation_type: &'static str,
    name: Option<&'static str>,
    /// The untyped `additionalContext` document, in the forms a `Document` takes.
    additional_context: Option<Value>,
    /// Per-pillar item filtering, so `Scope$items` is exercised as well as omitted.
    items: Option<Value>,
    /// How many of the profile's goals `scope.goalIds` names.
    goal_ids: usize,
}

/// The rows every generation test spans: scopes of one, several and every published
/// pillar (the modelled `Scope$pillars` `@length(1, 5)` bounds among them), each
/// recommendation type the enum publishes, the optional `name` present and absent, and an
/// `additionalContext` document of each form one can take — object, nested object, string,
/// number and array — beside a request that carries none.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one-pillar-bare",
            pillars: vec!["SECURITY"],
            recommendation_type: "RESOURCE",
            name: None,
            additional_context: None,
            items: None,
            goal_ids: 0,
        },
        Row {
            case: "two-pillars-named-object-context",
            pillars: vec!["COST_OPTIMIZATION", "RESILIENCE"],
            recommendation_type: "ARCHITECTURE",
            name: Some("quarterly-review"),
            additional_context: Some(json!({ "budget": "fixed", "window": "nights" })),
            items: None,
            goal_ids: 1,
        },
        Row {
            case: "every-published-pillar-nested-context",
            pillars: PILLARS.to_vec(),
            recommendation_type: "APPLICATION",
            name: Some("whole-framework"),
            additional_context: Some(json!({
                "constraints": { "regions": ["us-east-1", "eu-west-1"], "maxChanges": 3 },
                "notes": ["migrating", "regulated"],
            })),
            items: Some(json!([{ "pillar": "SECURITY", "ids": ["item-1", "item-2"] }])),
            goal_ids: 2,
        },
        Row {
            case: "pillar-the-issue-did-not-cite-string-context",
            pillars: vec!["OPERATIONAL_EXCELLENCE"],
            recommendation_type: "RESOURCE",
            name: Some("runbook-rehearsal"),
            additional_context: Some(json!("free-form guidance")),
            items: None,
            goal_ids: 0,
        },
        Row {
            case: "three-pillars-numeric-context",
            pillars: vec!["PERFORMANCE", "SECURITY", "RESILIENCE"],
            recommendation_type: "ARCHITECTURE",
            name: None,
            additional_context: Some(json!(42)),
            // `PillarItem$ids` is modelled `@length(min: 1)`, so every row names at
            // least one item.
            items: Some(json!([
                { "pillar": "PERFORMANCE", "ids": ["p-1"] },
                { "pillar": "RESILIENCE", "ids": ["r-1", "r-2"] },
            ])),
            goal_ids: 1,
        },
        Row {
            case: "four-pillars-array-context",
            pillars: vec![
                "SECURITY",
                "COST_OPTIMIZATION",
                "PERFORMANCE",
                "OPERATIONAL_EXCELLENCE",
            ],
            recommendation_type: "APPLICATION",
            name: Some("broad-sweep"),
            additional_context: Some(json!(["one", 2, true, null])),
            items: None,
            goal_ids: 0,
        },
    ]
}

impl Row {
    /// The `StartAgentRecommendationGeneration` body this row sends, with `goals` the ids
    /// of the goals its `scope.goalIds` may name.
    fn body(&self, goals: &[String]) -> Value {
        let mut scope = json!({ "pillars": self.pillars });
        if self.goal_ids > 0 {
            scope["goalIds"] = json!(goals[..self.goal_ids]);
        }
        if let Some(items) = self.items.clone() {
            scope["items"] = items;
        }
        let mut body = json!({ "types": [self.recommendation_type], "scope": scope });
        if let Some(name) = self.name {
            body["name"] = json!(name);
        }
        if let Some(context) = self.additional_context.clone() {
            body["additionalContext"] = context;
        }
        body
    }
}

/// Two goals of `profile_arn`, so a row whose scope names goals has stored ones to name.
async fn goals_of(client: &Client, profile_arn: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for (index, pillar) in ["SECURITY", "COST_OPTIMIZATION"].iter().enumerate() {
        let answer = create_goal(
            client,
            profile_arn,
            json!({ "pillars": [pillar], "title": format!("goal {index}") }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "expected CreateAgentGoal 200 for the scope's goals, got {answer:?}"
        );
        ids.push(
            answer
                .body
                .get("goal")
                .and_then(|goal| goal.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
    }
    ids
}

/// Assert the projection of one generation: every member the start request carried comes
/// back as sent, the service-owned members are its own, and the clock-derived members
/// agree with `expected_status`.
///
/// The one place the projection is pinned, so both the start answer and the read are
/// checked against the same rule. `reader` names the operation under test.
fn assert_generation(
    answer: &Answer,
    row: &Row,
    sent: &Value,
    profile_arn: &str,
    id: &str,
    expected_status: &str,
    reader: &str,
) {
    let case = row.case;
    assert_eq!(
        answer.status, 200,
        "case {case:?}/{reader}: expected 200, got {answer:?}"
    );
    let body = &answer.body;
    assert_eq!(
        body.get("id").and_then(Value::as_str),
        Some(id),
        "case {case:?}/{reader}: expected the generation the start minted, got {body:?}"
    );
    assert_eq!(
        body.get("profileArn").and_then(Value::as_str),
        Some(profile_arn),
        "case {case:?}/{reader}: the generation must name the profile it was started \
         against, got {body:?}"
    );
    assert_eq!(
        body.get("name").and_then(Value::as_str),
        row.name,
        "case {case:?}/{reader}: expected the name as sent, got {body:?}"
    );
    assert_eq!(
        body.get("status").and_then(Value::as_str),
        Some(expected_status),
        "case {case:?}/{reader}: expected status {expected_status:?}, got {body:?}"
    );
    assert_eq!(
        body.get("createdBy").and_then(Value::as_str),
        Some(engine::emulator_account_id()),
        "case {case:?}/{reader}: createdBy must be the account the request is attributed \
         to, got {body:?}"
    );

    let created_at = instant_seconds(body.get("createdAt"), case, "createdAt");
    let now = now_seconds();
    assert!(
        created_at > 0.0 && (now - created_at).abs() < 300.0,
        "case {case:?}/{reader}: createdAt must be the instant of the start call \
         (about {now}), got {created_at}"
    );
    let estimated = instant_seconds(
        body.get("estimatedCompletionTime"),
        case,
        "estimatedCompletionTime",
    );
    assert!(
        estimated > created_at,
        "case {case:?}/{reader}: estimatedCompletionTime must be after createdAt \
         ({created_at}), got {estimated}"
    );

    // Members only `GetAgentRecommendationGeneration` reports.
    if reader == "GetAgentRecommendationGeneration" {
        assert_eq!(
            body.get("additionalContext"),
            sent.get("additionalContext"),
            "case {case:?}/{reader}: expected additionalContext as sent, got {body:?}"
        );
        assert_eq!(
            body.get("scope"),
            sent.get("scope"),
            "case {case:?}/{reader}: expected the scope as sent, got {body:?}"
        );
        assert!(
            body.get("errorDetails").is_none(),
            "case {case:?}/{reader}: a generation that did not fail reports no \
             errorDetails, got {body:?}"
        );

        let progress = body.get("progress").cloned().unwrap_or(Value::Null);
        let total = progress.get("totalSteps").and_then(Value::as_i64);
        assert_eq!(
            total,
            Some(row.pillars.len() as i64),
            "case {case:?}/{reader}: totalSteps must count the pillars the scope names, \
             got {progress:?}"
        );
        let completed = progress
            .get("stepsCompleted")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        let percentage = progress
            .get("completionPercentage")
            .and_then(Value::as_f64)
            .unwrap_or(-1.0);
        let expected_completed = match expected_status {
            "QUEUED" => Some(0),
            "COMPLETED" => total,
            _ => None,
        };
        if let Some(expected) = expected_completed {
            assert_eq!(
                completed,
                expected,
                "case {case:?}/{reader}: a {expected_status} generation reports \
                 {expected} of {total:?} steps, got {progress:?}"
            );
        } else {
            assert!(
                (0..total.unwrap_or_default()).contains(&completed),
                "case {case:?}/{reader}: an in-flight generation reports fewer steps \
                 than its total, got {progress:?}"
            );
        }
        let expected_percentage =
            completed as f64 * 100.0 / total.unwrap_or(1).max(1) as f64;
        assert!(
            (percentage - expected_percentage).abs() < 1e-6,
            "case {case:?}/{reader}: completionPercentage must be the share of steps \
             done ({expected_percentage}), got {progress:?}"
        );
    }

    // `startedAt`, `endedAt` and the `lastModified…` pair are owed by the clock: each
    // appears exactly once its instant has passed, and the latest transition that
    // happened is what last modified the generation.
    let started = body.get("startedAt");
    let ended = body.get("endedAt");
    let last_modified_at = body.get("lastModifiedAt");
    match expected_status {
        "QUEUED" => {
            assert!(
                started.is_none() && ended.is_none() && last_modified_at.is_none(),
                "case {case:?}/{reader}: a QUEUED generation has not started, ended or \
                 been modified, got {body:?}"
            );
            assert!(
                body.get("lastModifiedBy").is_none(),
                "case {case:?}/{reader}: a QUEUED generation reports no lastModifiedBy, \
                 got {body:?}"
            );
        }
        "IN_PROGRESS" => {
            let started = instant_seconds(started, case, "startedAt");
            assert!(
                ended.is_none(),
                "case {case:?}/{reader}: an IN_PROGRESS generation has not ended, \
                 got {body:?}"
            );
            assert!(
                (instant_seconds(last_modified_at, case, "lastModifiedAt") - started).abs()
                    < 1e-6,
                "case {case:?}/{reader}: lastModifiedAt must be the latest transition \
                 ({started}), got {body:?}"
            );
            assert!(
                started >= created_at,
                "case {case:?}/{reader}: startedAt must not precede createdAt \
                 ({created_at}), got {started}"
            );
        }
        _ => {
            let started = instant_seconds(started, case, "startedAt");
            let ended = instant_seconds(ended, case, "endedAt");
            assert!(
                ended >= started && started >= created_at,
                "case {case:?}/{reader}: expected createdAt {created_at} <= startedAt \
                 {started} <= endedAt {ended}, got {body:?}"
            );
            assert!(
                (instant_seconds(last_modified_at, case, "lastModifiedAt") - ended).abs()
                    < 1e-6,
                "case {case:?}/{reader}: lastModifiedAt must be the latest transition \
                 ({ended}), got {body:?}"
            );
            assert_eq!(
                body.get("lastModifiedBy").and_then(Value::as_str),
                Some(engine::emulator_account_id()),
                "case {case:?}/{reader}: the service is what moved the status, \
                 got {body:?}"
            );
        }
    }
    if reader == "GetAgentRecommendationGeneration" && expected_status == "COMPLETED" {
        let ended = instant_seconds(ended, case, "endedAt");
        let estimated = instant_seconds(
            body.get("estimatedCompletionTime"),
            case,
            "estimatedCompletionTime",
        );
        assert!(
            (ended - estimated).abs() < 1e-6,
            "case {case:?}/{reader}: a completed generation ended at the instant it was \
             estimated to ({estimated}), got {ended}"
        );
    }
}

/// Rule 1, 2 and 3 at the start of the job: every row's generation is recorded and read
/// back as the clock owes it — `QUEUED` for a read taken inside the queue span, which is
/// non-zero on purpose so a caller polling straight after the start can observe it.
#[tokio::test]
async fn a_started_generation_round_trips_through_the_read() {
    assert!(
        !state::GENERATION_QUEUE_DURATION.is_zero(),
        "a generation's queue span must be non-zero so a caller can observe QUEUED, got {:?}",
        state::GENERATION_QUEUE_DURATION
    );

    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-host", 1).await;
    let goals = goals_of(&client, &profile_arn).await;

    let mut ids: Vec<String> = Vec::new();
    for row in rows() {
        let body = row.body(&goals);
        let started = start_generation(&client, &profile_arn, body.clone()).await;
        let id = started_id(&started, row.case);
        // The start answers the generation as of the instant it accepted it, which is
        // inside the queue span whatever the machine's load.
        assert_generation(
            &started,
            &row,
            &body,
            &profile_arn,
            &id,
            "QUEUED",
            "StartAgentRecommendationGeneration",
        );
        let created_at = instant_seconds(started.body.get("createdAt"), row.case, "createdAt");
        let estimated = instant_seconds(
            started.body.get("estimatedCompletionTime"),
            row.case,
            "estimatedCompletionTime",
        );

        let (fetched, owed) =
            read_generation(&client, &profile_arn, &id, created_at, estimated).await;
        let status = reported_status(&fetched, &owed, row.case);
        assert_generation(
            &fetched,
            &row,
            &body,
            &profile_arn,
            &id,
            &status,
            "GetAgentRecommendationGeneration",
        );

        assert!(
            !ids.contains(&id),
            "case {:?}: every generation gets its own id, got a repeat of {id:?}",
            row.case
        );
        ids.push(id);
    }
}

/// Rule 2 and 3 over the whole job: a generation's status and progress are whatever the
/// clock owes at the instant of the read, and once the spans have passed it is
/// `COMPLETED` with every step of its scope done.
#[tokio::test]
async fn a_generation_moves_through_the_statuses_the_clock_owes() {
    assert!(
        !state::GENERATION_RUN_DURATION.is_zero(),
        "a generation's run span must be non-zero so a caller can observe IN_PROGRESS, \
         got {:?}",
        state::GENERATION_RUN_DURATION
    );

    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-clock", 2).await;
    let goals = goals_of(&client, &profile_arn).await;

    // Start every row first, so one pair of sleeps covers the whole table.
    let mut started: Vec<(Row, Value, String, f64, f64)> = Vec::new();
    for row in rows() {
        let body = row.body(&goals);
        let answer = start_generation(&client, &profile_arn, body.clone()).await;
        let id = started_id(&answer, row.case);
        let created_at = instant_seconds(answer.body.get("createdAt"), row.case, "createdAt");
        let estimated = instant_seconds(
            answer.body.get("estimatedCompletionTime"),
            row.case,
            "estimatedCompletionTime",
        );
        started.push((row, body, id, created_at, estimated));
    }

    // Partway into the analysis: each read reports the status the clock owed while it was
    // in flight, and its members agree with that status.
    tokio::time::sleep(state::GENERATION_QUEUE_DURATION + state::GENERATION_RUN_DURATION / 2)
        .await;
    for (row, body, id, created_at, estimated) in &started {
        let (answer, owed) =
            read_generation(&client, &profile_arn, id, *created_at, *estimated).await;
        let status = reported_status(&answer, &owed, row.case);
        assert_generation(
            &answer,
            row,
            body,
            &profile_arn,
            id,
            &status,
            "GetAgentRecommendationGeneration",
        );
    }

    // Past the instant every completion was due, plus a margin: the only status the clock
    // owes now is the terminal one.
    tokio::time::sleep(state::GENERATION_RUN_DURATION / 2 + Duration::from_millis(500)).await;
    for (row, body, id, created_at, estimated) in &started {
        let (answer, owed) =
            read_generation(&client, &profile_arn, id, *created_at, *estimated).await;
        assert_eq!(
            owed,
            vec!["COMPLETED"],
            "case {:?}: the sleeps must have outlasted both spans the clock owes",
            row.case
        );
        let status = reported_status(&answer, &owed, row.case);
        assert_generation(
            &answer,
            row,
            body,
            &profile_arn,
            id,
            &status,
            "GetAgentRecommendationGeneration",
        );
    }
}

/// Rule 2 and 3 exactly, at the instants the transitions come due: the stored record
/// derives each published status, its progress and its transition timestamps from the
/// clock, so every phase is pinned without a sleep deciding which one a read lands in.
#[tokio::test]
async fn the_stored_record_derives_each_status_and_progress_from_the_clock() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-derived", 1).await;
    let goals = goals_of(&client, &profile_arn).await;

    for row in rows() {
        let answer = start_generation(&client, &profile_arn, row.body(&goals)).await;
        let id = started_id(&answer, row.case);
        let store = client.store();
        let guard = store.lock();
        let record = guard
            .agent_generation(&profile_arn, &id)
            .expect("the start must have recorded the generation under its profile");
        let total = i32::try_from(row.pillars.len()).expect("a scope names at most 5 pillars");
        let tick = Duration::from_millis(1);

        let phases: [(&str, SystemTime, &str); 8] = [
            ("the instant of the start call", record.created_at, "QUEUED"),
            (
                "halfway through the queue",
                record.created_at + state::GENERATION_QUEUE_DURATION / 2,
                "QUEUED",
            ),
            (
                "one tick before the analysis starts",
                record.starts_at - tick,
                "QUEUED",
            ),
            (
                "the instant the analysis starts",
                record.starts_at,
                "IN_PROGRESS",
            ),
            (
                "halfway through the analysis",
                record.starts_at + state::GENERATION_RUN_DURATION / 2,
                "IN_PROGRESS",
            ),
            (
                "one tick before it ends",
                record.ends_at - tick,
                "IN_PROGRESS",
            ),
            ("the instant it ends", record.ends_at, "COMPLETED"),
            (
                "long after it ended",
                record.ends_at + Duration::from_secs(3_600),
                "COMPLETED",
            ),
        ];

        for (phase, instant, expected) in phases {
            let case = format!("{}/{phase}", row.case);
            assert_eq!(
                record.status(instant),
                expected,
                "case {case:?}: expected status {expected:?}, got {:?}",
                record.status(instant)
            );
            assert_eq!(
                record.started_at(instant).is_some(),
                expected != "QUEUED",
                "case {case:?}: startedAt appears exactly once the analysis has started, \
                 got {:?}",
                record.started_at(instant)
            );
            assert_eq!(
                record.ended_at(instant).is_some(),
                expected == "COMPLETED",
                "case {case:?}: endedAt appears exactly once the analysis has ended, \
                 got {:?}",
                record.ended_at(instant)
            );
            let expected_modified = match expected {
                "QUEUED" => None,
                "IN_PROGRESS" => Some(record.starts_at),
                _ => Some(record.ends_at),
            };
            assert_eq!(
                record.last_modified(instant).map(|(_, at)| at),
                expected_modified,
                "case {case:?}: lastModifiedAt is the latest transition that happened, \
                 got {:?}",
                record.last_modified(instant)
            );

            let progress = record.progress(instant);
            assert_eq!(
                progress.total_steps, total,
                "case {case:?}: totalSteps counts the pillars the scope names, \
                 got {progress:?}"
            );
            let expected_steps = match expected {
                "QUEUED" => 0,
                "COMPLETED" => total,
                _ => progress.steps_completed,
            };
            assert_eq!(
                progress.steps_completed, expected_steps,
                "case {case:?}: expected {expected_steps} of {total} steps, \
                 got {progress:?}"
            );
            assert!(
                (progress.completion_percentage
                    - f64::from(progress.steps_completed) * 100.0 / f64::from(total))
                    .abs()
                    < 1e-9,
                "case {case:?}: completionPercentage is the share of steps done, \
                 got {progress:?}"
            );
        }

        // Every step of the scope comes due in its own slice of the run span, so the
        // progress a read reports grows one step at a time rather than jumping.
        for step in 0..total {
            let share = (f64::from(step) + 0.5) / f64::from(total);
            let instant = record.starts_at
                + Duration::from_secs_f64(state::GENERATION_RUN_DURATION.as_secs_f64() * share);
            let progress = record.progress(instant);
            assert_eq!(
                progress.steps_completed, step,
                "case {:?}/step {step} of {total}: expected {step} steps done partway \
                 through that step, got {progress:?}",
                row.case
            );
        }
    }
}

/// Rule 4: the read is a REPORT. Polling it repeatedly never changes the generation's
/// identity, scope or instants, and the status only moves forwards through the published
/// order.
#[tokio::test]
async fn polling_a_generation_changes_nothing_a_later_caller_sees() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-poll", 1).await;
    let goals = goals_of(&client, &profile_arn).await;
    let row = rows()
        .into_iter()
        .find(|row| row.case == "two-pillars-named-object-context")
        .expect("the row the poll test uses");
    let body = row.body(&goals);
    let started = start_generation(&client, &profile_arn, body.clone()).await;
    let id = started_id(&started, row.case);

    let mut last_status = 0usize;
    let mut first: Option<Value> = None;
    for poll in 0..8 {
        let answer = get_generation(&client, &profile_arn, &id).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}/poll {poll}: expected 200, got {answer:?}",
            row.case
        );
        let status = answer
            .body
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let index = GENERATION_STATUSES
            .iter()
            .position(|published| *published == status)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}/poll {poll}: status must be one of {GENERATION_STATUSES:?}, \
                     got {status:?}",
                    row.case
                )
            });
        assert!(
            index >= last_status,
            "case {:?}/poll {poll}: a generation's status never moves backwards, went \
             from {:?} to {status:?}",
            row.case,
            GENERATION_STATUSES[last_status]
        );
        last_status = index;

        // The members a read must never change, compared against the FIRST read.
        let stable = json!({
            "id": answer.body.get("id"),
            "profileArn": answer.body.get("profileArn"),
            "name": answer.body.get("name"),
            "scope": answer.body.get("scope"),
            "additionalContext": answer.body.get("additionalContext"),
            "createdAt": answer.body.get("createdAt"),
            "createdBy": answer.body.get("createdBy"),
            "estimatedCompletionTime": answer.body.get("estimatedCompletionTime"),
        });
        match &first {
            None => first = Some(stable),
            Some(expected) => assert_eq!(
                &stable, expected,
                "case {:?}/poll {poll}: a read must report the same stored generation, \
                 got {answer:?}",
                row.case
            ),
        }
        tokio::time::sleep(state::GENERATION_QUEUE_DURATION / 2).await;
    }
    assert_eq!(
        GENERATION_STATUSES[last_status], "COMPLETED",
        "case {:?}: the polls must have outlasted the spans the clock owes",
        row.case
    );
}

/// Rule 5: a generation is reached only through the profile that holds it, and a
/// malformed identifier never reaches the lookup.
#[tokio::test]
async fn a_generation_is_reached_only_through_the_profile_that_holds_it() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-owner", 1).await;
    let other_arn = profile_arn_of(&client, "generation-stranger", 1).await;
    let row = rows().into_iter().next().expect("the first row");
    let started = start_generation(&client, &profile_arn, row.body(&[])).await;
    let id = started_id(&started, row.case);

    // A well-formed UUID the stored profile does not hold. It carries hex LETTERS, so
    // upper-casing it is a value the modelled pattern refuses.
    let absent_id = "abcdefab-0000-4a00-8b00-00000000000c";
    let not_found: [(&str, String, String); 3] = [
        (
            "another profile's generation",
            other_arn.clone(),
            id.clone(),
        ),
        (
            "an id no profile holds",
            profile_arn.clone(),
            absent_id.to_string(),
        ),
        (
            "a profile that is not stored",
            absent_profile_arn("absent-generation-host"),
            id.clone(),
        ),
    ];
    for (case, arn, generation_id) in not_found {
        let answer = get_generation(&client, &arn, &generation_id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ResourceId").and_then(Value::as_str),
            Some(format!("{arn}.{generation_id}").as_str()),
            "case {case:?}: the ResourceId names the pair as sent, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("ResourceType").and_then(Value::as_str),
            Some("GENERATION"),
            "case {case:?}: the ResourceType names the missing resource, got {answer:?}"
        );
    }

    // Values the modelled patterns refuse are refused at the boundary, before any lookup.
    for (case, arn, generation_id) in [
        ("not a UUID", profile_arn.clone(), "not-a-uuid".to_string()),
        (
            "uppercase UUID",
            profile_arn.clone(),
            absent_id.to_uppercase(),
        ),
        ("not an ARN", "not-an-arn".to_string(), id.clone()),
    ] {
        let answer = get_generation(&client, &arn, &generation_id).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a value the modelled pattern refuses is a 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }
}

/// Rule 6: `@idempotent`. The same start request repeated while its generation is in
/// flight answers that generation; a request differing in any one member starts its own.
///
/// Whether the first generation is still in flight when the repeat arrives is up to the
/// machine, so the assertion is on the RULE either way: a repeat answers the generation
/// already running, and a second one appears only once the first had already completed.
#[tokio::test]
async fn repeating_a_start_while_it_is_in_flight_answers_the_same_generation() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-idempotent", 1).await;
    let goals = goals_of(&client, &profile_arn).await;

    for row in rows() {
        let body = row.body(&goals);
        let first = start_generation(&client, &profile_arn, body.clone()).await;
        let id = started_id(&first, row.case);
        let first_ends = instant_seconds(
            first.body.get("estimatedCompletionTime"),
            row.case,
            "estimatedCompletionTime",
        );
        let repeat = start_generation(&client, &profile_arn, body.clone()).await;
        let repeat_id = started_id(&repeat, row.case);
        if repeat_id == id {
            // The repeat landed while the first analysis was still running, so it
            // answered that one rather than starting a second.
            assert!(
                instant_seconds(repeat.body.get("createdAt"), row.case, "createdAt")
                    < first_ends,
                "case {:?}: the generation the repeat answered must be the one still in \
                 flight, got {repeat:?}",
                row.case
            );
        } else {
            // A second generation appears ONLY because the first had already completed by
            // the time the repeat arrived, which is a fresh analysis, not a lost replay.
            let repeat_started =
                instant_seconds(repeat.body.get("createdAt"), row.case, "createdAt");
            assert!(
                repeat_started >= first_ends,
                "case {:?}: a repeat of a start still in flight must answer the \
                 generation it started (in flight until {first_ends}, repeat at \
                 {repeat_started}), got {repeat:?}",
                row.case
            );
        }

        // One member at a time differs: each variant is a DIFFERENT analysis, so it gets
        // its own generation.
        let mut variants: Vec<(&str, Value)> = vec![
            ("another name", {
                let mut other = body.clone();
                other["name"] = json!("a-different-name");
                other
            }),
            ("another type", {
                let mut other = body.clone();
                let next = RECOMMENDATION_TYPES
                    .iter()
                    .find(|published| **published != row.recommendation_type)
                    .expect("another published recommendation type");
                other["types"] = json!([next]);
                other
            }),
            ("another context", {
                let mut other = body.clone();
                other["additionalContext"] = json!({ "unlike": "the first" });
                other
            }),
            ("another pillar set", {
                let mut other = body.clone();
                let extra = PILLARS
                    .iter()
                    .find(|pillar| !row.pillars.contains(pillar))
                    .copied();
                let mut pillars = row.pillars.clone();
                match extra {
                    Some(pillar) => pillars.push(pillar),
                    // The row already names every published pillar, so drop one instead.
                    None => {
                        pillars.pop();
                    }
                }
                other["scope"]["pillars"] = json!(pillars);
                other
            }),
        ];
        if !goals.is_empty() {
            let mut other = body.clone();
            other["scope"]["goalIds"] = json!([goals[goals.len() - 1]]);
            variants.push(("another goal set", other));
        }

        for (variant, variant_body) in variants {
            if variant_body == body {
                continue;
            }
            let answer = start_generation(&client, &profile_arn, variant_body.clone()).await;
            let variant_case = format!("{}/{variant}", row.case);
            assert_ne!(
                started_id(&answer, &variant_case),
                id,
                "case {variant_case:?}: a start differing in one member is its own \
                 generation, got {answer:?}"
            );
        }
    }
}

/// Rule 7: a start the service cannot serve is refused and records nothing.
#[tokio::test]
async fn a_start_the_service_cannot_serve_is_refused_as_a_validation_exception() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-refusals", 1).await;
    let ineligible_arn = profile_arn_of(&client, "generation-no-scope", 0).await;
    let goals = goals_of(&client, &profile_arn).await;
    let good_body = json!({ "types": ["RESOURCE"], "scope": { "pillars": ["SECURITY"] } });

    let refusals: Vec<(&str, String, Value)> = vec![
        (
            "a profile that is not stored",
            absent_profile_arn("absent-start-host"),
            good_body.clone(),
        ),
        (
            "a profile with no aggregation scope to analyse",
            ineligible_arn.clone(),
            good_body.clone(),
        ),
        (
            "a goal id the profile does not hold",
            profile_arn.clone(),
            json!({
                "types": ["RESOURCE"],
                "scope": {
                    "pillars": ["SECURITY"],
                    "goalIds": ["abcdefab-0000-4a00-8b00-00000000000c"],
                },
            }),
        ),
        (
            "a recommendation type the enum does not publish",
            profile_arn.clone(),
            json!({ "types": ["TELEPATHY"], "scope": { "pillars": ["SECURITY"] } }),
        ),
        (
            "a pillar the enum does not publish",
            profile_arn.clone(),
            json!({ "types": ["RESOURCE"], "scope": { "pillars": ["ASTROLOGY"] } }),
        ),
        (
            "a scope naming no pillar at all",
            profile_arn.clone(),
            json!({ "types": ["RESOURCE"], "scope": { "pillars": [] } }),
        ),
        (
            "more recommendation types than the published maximum",
            profile_arn.clone(),
            json!({
                "types": RECOMMENDATION_TYPES[..=TYPES_MAX],
                "scope": { "pillars": ["SECURITY"] },
            }),
        ),
        (
            "an additionalContext nested one level past the ceiling",
            profile_arn.clone(),
            json!({
                "types": ["RESOURCE"],
                "scope": { "pillars": ["SECURITY"] },
                "additionalContext": nested_document(state::MAX_DOCUMENT_DEPTH),
            }),
        ),
        (
            "an additionalContext nested far past the ceiling",
            profile_arn.clone(),
            json!({
                "types": ["RESOURCE"],
                "scope": { "pillars": ["SECURITY"] },
                "additionalContext": nested_document(state::MAX_DOCUMENT_DEPTH * 2),
            }),
        ),
    ];

    for (case, arn, body) in refusals {
        let answer = start_generation(&client, &arn, body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }

    // The document ceiling is a BOUND: the deepest document it allows is served, and only
    // one past it is refused (the two rows above).
    let at_the_ceiling = json!({
        "types": ["RESOURCE"],
        "scope": { "pillars": ["SECURITY"] },
        "additionalContext": nested_document(state::MAX_DOCUMENT_DEPTH - 1),
    });
    let answer = start_generation(&client, &profile_arn, at_the_ceiling.clone()).await;
    let deep_id = started_id(&answer, "an additionalContext nested to the ceiling");
    let fetched = get_generation(&client, &profile_arn, &deep_id).await;
    assert_eq!(
        fetched.body.get("additionalContext"),
        at_the_ceiling.get("additionalContext"),
        "a document nested to the ceiling round-trips as sent, got {fetched:?}"
    );

    // The type list's `@length` is a BOUND too: a start naming the published maximum is
    // SERVED, and only one naming a type more is refused (the row above), and the
    // generation reports the whole set it covers.
    let at_the_type_maximum = json!({
        "types": RECOMMENDATION_TYPES[..TYPES_MAX],
        "scope": { "pillars": ["SECURITY"] },
    });
    let answer = start_generation(&client, &profile_arn, at_the_type_maximum).await;
    let typed_id = started_id(&answer, "types at the published maximum");
    let fetched = get_generation(&client, &profile_arn, &typed_id).await;
    assert_eq!(
        fetched
            .body
            .get("generationRecommendationTypes")
            .and_then(Value::as_array)
            .map(|types| types.len()),
        Some(TYPES_MAX),
        "a generation reports every recommendation type it covers, got {fetched:?}"
    );

    // The refusal that names the profile's own missing scope says which member leaves it
    // ineligible — the sentence `GetAgentProfile` reports in `fieldErrors`.
    let answer = start_generation(&client, &ineligible_arn, good_body.clone()).await;
    let message = answer
        .body
        .get("Message")
        .or_else(|| answer.body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains("At least one aggregation configuration is required to generate \
                          recommendations."),
        "an ineligible profile must say what leaves it so, got {answer:?}"
    );

    // Nothing was recorded by any refusal: the eligible profile's only generation is the
    // one a successful start records now.
    // Nothing was recorded by any refusal: the profile holds exactly the starts that WERE
    // served — the document nested to the ceiling, the type list at its published maximum,
    // and this one.
    let started = start_generation(&client, &profile_arn, good_body).await;
    let id = started_id(&started, "after the refusals");
    let fetched = get_generation(&client, &profile_arn, &id).await;
    assert_eq!(
        fetched.status, 200,
        "a start after the refusals must still record its generation, got {fetched:?}"
    );
    // Read through the guard into a value before asserting: a failing assertion whose
    // message takes the store's lock again would deadlock instead of reporting.
    let served = client.store().lock().agent_generation_count(&profile_arn);
    assert_eq!(
        served, 3,
        "the refusals must have recorded no generation beside the starts that were \
         served, got {served} for {profile_arn}"
    );
    let ineligible_generations = client.store().lock().agent_generation_count(&ineligible_arn);
    assert_eq!(
        ineligible_generations, 0,
        "an ineligible profile holds no generation, got {ineligible_generations}"
    );
    assert!(
        !goals.is_empty(),
        "the suite's goals must have been recorded for the scope rows"
    );
}

/// Rule 9: a generation does not outlive the profile it is reached through.
#[tokio::test]
async fn deleting_the_profile_takes_its_generations_with_it() {
    let client = Client::new();
    // `deletionProtection` is `@default(true)`, so a profile this test can delete has to
    // clear it on the create.
    let mut body = super::agent_goals::profile_body("generation-deleted-host", &["SECURITY"], 1);
    body["deletionProtection"] = json!(false);
    let created = super::agent_goals::create_profile(&client, body).await;
    assert_eq!(
        created.status, 200,
        "expected CreateAgentProfile 200 for the profile under test, got {created:?}"
    );
    let profile_arn = created
        .body
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let goals = goals_of(&client, &profile_arn).await;

    let mut ids: Vec<(&str, String)> = Vec::new();
    for row in rows() {
        let answer = start_generation(&client, &profile_arn, row.body(&goals)).await;
        ids.push((row.case, started_id(&answer, row.case)));
    }
    assert_eq!(
        client.store().lock().agent_generation_count(&profile_arn),
        ids.len(),
        "every row's generation must be recorded under the profile before the delete"
    );

    let deleted = delete_profile(&client, &profile_arn).await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteAgentProfile 200, got {deleted:?}"
    );

    for (case, id) in &ids {
        let answer = get_generation(&client, &profile_arn, id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a generation of a deleted profile must not resolve, \
             got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
    }

    let listed = list_generations(&client, &profile_arn, "").await;
    assert_eq!(
        (listed.status, listed.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "a deleted profile resolves to nothing, got {listed:?}"
    );
    // Reached through the shared state rather than over HTTP, because an ARN is minted
    // fresh per profile: a record left behind is invisible on the wire and still wrong.
    assert!(
        !client
            .store()
            .lock()
            .agent_generations
            .contains_key(&profile_arn),
        "no generation may outlive the profile it belongs to"
    );
}

/// Rule 11: the `RecommendationType` filter selects the generations asked for that type,
/// for every published value; an unpublished value is refused.
#[tokio::test]
async fn the_recommendation_type_filter_selects_the_generations_asked_for_that_type() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-filter", 1).await;

    // Two generations per published type, so a filter that reported one entry by accident
    // could not pass.
    let mut by_type: Vec<(&str, Vec<String>)> =
        RECOMMENDATION_TYPES.iter().map(|kind| (*kind, Vec::new())).collect();
    for round in 0..2 {
        for (kind, ids) in &mut by_type {
            let answer = start_generation(
                &client,
                &profile_arn,
                json!({
                    "types": [*kind],
                    "name": format!("{}-{round}", kind.to_lowercase()),
                    "scope": { "pillars": [PILLARS[round % PILLARS.len()]] },
                }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {kind:?}: expected StartAgentRecommendationGeneration 200, got {answer:?}"
            );
            ids.push(id_of(&answer.body));
        }
    }

    let mut every_id: Vec<String> = by_type
        .iter()
        .flat_map(|(_, ids)| ids.iter().cloned())
        .collect();
    every_id.sort();

    for (kind, ids) in &by_type {
        let mut expected = ids.clone();
        expected.sort();
        let answer = list_generations(
            &client,
            &profile_arn,
            &format!("{RECOMMENDATION_TYPE_QUERY}={kind}&{MAX_RESULTS_QUERY}=1000"),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {kind:?}: expected ListAgentRecommendationGenerations 200, got {answer:?}"
        );
        let reported: Vec<String> = items(&answer).iter().map(id_of).collect();
        assert_eq!(
            reported, expected,
            "case {kind:?}: the filter must report exactly the generations asked for that \
             type, got {answer:?}"
        );
    }

    let unfiltered =
        list_generations(&client, &profile_arn, &format!("{MAX_RESULTS_QUERY}=1000")).await;
    let reported: Vec<String> = items(&unfiltered).iter().map(id_of).collect();
    assert_eq!(
        reported, every_id,
        "case \"no-filter\": a request that names no type must report every generation, \
         got {unfiltered:?}"
    );

    // A value outside the enum's published set is a refusal, never "no filter".
    for case in ["COST", "resource", "RESOURCES"] {
        let answer = list_generations(
            &client,
            &profile_arn,
            &format!("{RECOMMENDATION_TYPE_QUERY}={case}"),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a value outside the published set must be refused, got {answer:?}"
        );
    }
}

/// Rule 12: paging over a stored set with several page sizes enumerates every selected
/// generation exactly once, in ascending id order, and only the last page omits
/// `nextToken` — with and without the type filter.
#[tokio::test]
async fn paging_enumerates_every_generation_exactly_once() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-paging", 1).await;

    let stored = 7usize;
    let mut all_ids: Vec<String> = Vec::new();
    let mut filtered_ids: Vec<String> = Vec::new();
    for index in 0..stored {
        // Alternating types, so the filtered enumeration is a strict subset with gaps.
        let kind = RECOMMENDATION_TYPES[index % 2];
        let answer = start_generation(
            &client,
            &profile_arn,
            json!({
                "types": [kind],
                "name": format!("paged-generation-{index}"),
                "scope": { "pillars": [PILLARS[index % PILLARS.len()]] },
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "expected StartAgentRecommendationGeneration 200 for generation {index}, \
             got {answer:?}"
        );
        let id = id_of(&answer.body);
        if kind == RECOMMENDATION_TYPES[0] {
            filtered_ids.push(id.clone());
        }
        all_ids.push(id);
    }
    all_ids.sort();
    filtered_ids.sort();

    // A page size below, at and above the selected count, plus the request that names none
    // (the member's `@default(100)`), so the rule is asserted where the page divides the
    // set evenly, unevenly and not at all.
    for filter in [None, Some(RECOMMENDATION_TYPES[0])] {
        let expected = if filter.is_some() {
            &filtered_ids
        } else {
            &all_ids
        };
        for page_size in [None, Some(1usize), Some(2), Some(3), Some(stored), Some(stored + 4)] {
            let case = format!("filter={filter:?} maxResults={page_size:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let mut query = String::new();
                if let Some(kind) = filter {
                    query.push_str(&format!("{RECOMMENDATION_TYPE_QUERY}={kind}"));
                }
                if let Some(size) = page_size {
                    if !query.is_empty() {
                        query.push('&');
                    }
                    query.push_str(&format!("{MAX_RESULTS_QUERY}={size}"));
                }
                if let Some(token) = &next_token {
                    if !query.is_empty() {
                        query.push('&');
                    }
                    query.push_str(&format!("{NEXT_TOKEN_QUERY}={token}"));
                }
                let answer = list_generations(&client, &profile_arn, &query).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected ListAgentRecommendationGenerations 200, \
                     got {answer:?}"
                );
                let page: Vec<String> = items(&answer).iter().map(id_of).collect();
                if let Some(size) = page_size {
                    assert!(
                        page.len() <= size,
                        "case {case:?}: a page must not be longer than maxResults, got {page:?}"
                    );
                }
                seen.extend(page);
                pages += 1;
                assert!(
                    pages <= stored + 1,
                    "case {case:?}: paging must terminate, got {pages} pages for {stored} \
                     generations"
                );
                next_token = answer
                    .body
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                if next_token.is_none() {
                    break;
                }
            }
            assert_eq!(
                &seen, expected,
                "case {case:?}: paging must enumerate every selected generation exactly \
                 once, in ascending id order"
            );
        }
    }
}

/// Rule 12 (cont.): `maxResults` at each published bound is served; one past either bound
/// is
/// refused by the boundary as a `400 ValidationException`.
#[tokio::test]
async fn max_results_is_held_to_its_published_range() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-max-results", 1).await;
    for (index, kind) in RECOMMENDATION_TYPES.iter().enumerate().take(2) {
        let answer = start_generation(
            &client,
            &profile_arn,
            json!({
                "types": [kind],
                "name": format!("bounded-generation-{index}"),
                "scope": { "pillars": [PILLARS[0]] },
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "expected StartAgentRecommendationGeneration 200 for generation {index}, \
             got {answer:?}"
        );
    }

    for max_results in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let answer = list_generations(
            &client,
            &profile_arn,
            &format!("{MAX_RESULTS_QUERY}={max_results}"),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {max_results}: a published bound must be served, got {answer:?}"
        );
        let served = items(&answer).len();
        assert!(
            served >= 1 && served <= usize::try_from(max_results).expect("a positive bound"),
            "case {max_results}: expected at most {max_results} of the 2 stored \
             generations, got {served}"
        );
    }

    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let answer = list_generations(
            &client,
            &profile_arn,
            &format!("{MAX_RESULTS_QUERY}={max_results}"),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {max_results}: a value past a published bound must be refused, \
             got {answer:?}"
        );
    }
}

/// Rule 12 (cont.): a `nextToken` this service did not issue is a refusal, not a page.
#[tokio::test]
async fn an_unissued_next_token_is_refused() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-tokens", 1).await;
    let started = start_generation(
        &client,
        &profile_arn,
        json!({
            "types": [RECOMMENDATION_TYPES[0]],
            "name": "the-only-generation",
            "scope": { "pillars": [PILLARS[0]] },
        }),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "expected StartAgentRecommendationGeneration 200, got {started:?}"
    );

    // Each row satisfies the modelled `NextToken` pattern `^[A-Za-z0-9+/=_-]+$`, so the
    // boundary passes it to the handler: what is under test is the handler's own refusal.
    // The last two rows are cursors OTHER paginated operations of this service issue,
    // which are not cursors here.
    let rows: Vec<(&str, String)> = vec![
        ("not-a-cursor", "notacursor".to_string()),
        ("hex-of-nothing", "6e756c6c".to_string()),
        ("one-character", "a".to_string()),
        (
            "another-operations-cursor",
            state::encode_agent_context_next_token(&id_of(&started.body)),
        ),
        (
            "a-share-cursor",
            state::encode_share_next_token("0123456789abcdef0123456789abcdef"),
        ),
    ];
    for (case, token) in rows {
        let answer =
            list_generations(&client, &profile_arn, &format!("{NEXT_TOKEN_QUERY}={token}")).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: an unissued nextToken must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.body.get("Reason").and_then(Value::as_str),
            Some("CANNOT_PARSE"),
            "case {case:?}: an undecodable cursor is CANNOT_PARSE, got {answer:?}"
        );
        let fields: Vec<&str> = answer
            .body
            .get("Fields")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field.get("Name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            fields,
            vec![state::NEXT_TOKEN_MEMBER],
            "case {case:?}: the finding must name the member whose value could not be \
             decoded, got {answer:?}"
        );
    }

    // The path's resource is resolved BEFORE the query members are judged: a request
    // against a profile that is not there is the 404 however its token looks.
    let absent = list_generations(
        &client,
        &absent_profile_arn("absent-with-a-bad-token"),
        &format!("{NEXT_TOKEN_QUERY}=notacursor"),
    )
    .await;
    assert_eq!(
        (absent.status, absent.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"absent-profile-and-bad-token\": the missing profile is reported before \
         the token is judged, got {absent:?}"
    );
}

/// Rule 7 (cont.): a start whose `profileArn` names no stored profile is refused as that
/// field, and records nothing.
#[tokio::test]
async fn starting_a_generation_for_an_unknown_profile_is_refused_and_records_nothing() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-unknown-profile", 1).await;

    for (case, target) in [
        ("never-created", absent_profile_arn("no-such-profile")),
        (
            "another-account",
            "arn:aws:wellarchitected:eu-west-1:210987654321:agent-profile/foreign".to_string(),
        ),
    ] {
        let answer = start_generation(
            &client,
            &target,
            json!({
                "types": [RECOMMENDATION_TYPES[0]],
                "name": "unstartable",
                "scope": { "pillars": [PILLARS[0]] },
            }),
        )
        .await;
        // The operation models no `ResourceNotFoundException`
        // (`smithy_query(query="operation",
        // operation="StartAgentRecommendationGeneration")`), so the unresolvable ARN is
        // reported as the field whose value the service refused.
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: an ARN that names no profile must be refused, got {answer:?}"
        );
        let fields: Vec<&str> = answer
            .body
            .get("Fields")
            .and_then(Value::as_array)
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field.get("Name").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            fields,
            vec![state::PROFILE_ARN_MEMBER],
            "case {case:?}: the finding must name the member that could not be resolved, \
             got {answer:?}"
        );
    }

    // A value outside a published enum set is refused the same way, and stores nothing.
    for (case, body) in [
        (
            "unknown-recommendation-type",
            json!({
                "types": ["COST"],
                "scope": { "pillars": [PILLARS[0]] },
            }),
        ),
        (
            "unknown-pillar",
            json!({
                "types": [RECOMMENDATION_TYPES[0]],
                "scope": { "pillars": ["COST"] },
            }),
        ),
        (
            "unknown-pillar-in-items",
            json!({
                "types": [RECOMMENDATION_TYPES[0]],
                "scope": {
                    "pillars": [PILLARS[0]],
                    "items": [{ "pillar": "COST", "ids": ["x"] }],
                },
            }),
        ),
    ] {
        let answer = start_generation(&client, &profile_arn, body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a value outside the enum's published set must be refused, \
             got {answer:?}"
        );
    }

    let listed = list_generations(&client, &profile_arn, "").await;
    assert_eq!(
        listed.status, 200,
        "expected ListAgentRecommendationGenerations 200, got {listed:?}"
    );
    assert!(
        items(&listed).is_empty(),
        "a refused start must record nothing, got {listed:?}"
    );
}

/// Rule 13: generations are metered per profile. A profile already holding the quota
/// refuses the next start with the modelled `402 ServiceQuotaExceededException` and
/// records nothing further; a sibling profile is unaffected, because the quota is per
/// profile rather than per account.
#[tokio::test]
async fn a_profile_at_its_generation_quota_refuses_the_next_start() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "generation-quota", 1).await;
    let sibling = profile_arn_of(&client, "generation-quota-sibling", 1).await;

    let quota = state::MAX_GENERATIONS_PER_AGENT_PROFILE;
    for index in 0..quota {
        let answer = start_generation(
            &client,
            &profile_arn,
            json!({
                "types": [RECOMMENDATION_TYPES[index % RECOMMENDATION_TYPES.len()]],
                "name": format!("metered-{index}"),
                "scope": { "pillars": [PILLARS[index % PILLARS.len()]] },
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "a start inside the quota must be served (generation {index}), got {answer:?}"
        );
    }

    let past = start_generation(
        &client,
        &profile_arn,
        json!({
            "types": [RECOMMENDATION_TYPES[0]],
            "name": "one-too-many",
            "scope": { "pillars": [PILLARS[0]] },
        }),
    )
    .await;
    assert_eq!(
        (past.status, past.error_code.as_str()),
        (402, "ServiceQuotaExceededException"),
        "a start past the quota must be refused, got {past:?}"
    );
    assert_eq!(
        past.body.get("ResourceType").and_then(Value::as_str),
        Some(state::AGENT_GENERATION_RESOURCE_TYPE),
        "the refusal must name what the quota meters, got {past:?}"
    );
    assert_eq!(
        items(&list_generations(&client, &profile_arn, &format!("{MAX_RESULTS_QUERY}=1000")).await)
            .len(),
        quota,
        "the refused start must record nothing"
    );

    // The quota is per profile: the sibling still takes a start.
    let sibling_start = start_generation(
        &client,
        &sibling,
        json!({
            "types": [RECOMMENDATION_TYPES[0]],
            "name": "unmetered-elsewhere",
            "scope": { "pillars": [PILLARS[0]] },
        }),
    )
    .await;
    assert_eq!(
        sibling_start.status, 200,
        "another profile's quota must not refuse this start, got {sibling_start:?}"
    );
}

/// Rule 6's foundation: the fingerprint an `@idempotent` repeat is matched on is a
/// function of the document's MEMBERS, not of a `HashMap`'s iteration order.
///
/// `Document::Object` is a `HashMap`, so two instances holding the same keys can iterate
/// in different orders; two documents built in opposite orders must still fingerprint
/// alike, or an identical repeat would quietly start a second analysis.
#[test]
fn a_document_fingerprints_by_its_members_not_by_its_map_order() {
    use ::aws_smithy_types::Document;

    /// Member names enough to make two `HashMap`s of them iterate differently.
    const NAMES: [&str; 8] = [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
    ];

    let build = |reverse: bool| -> Document {
        let mut names: Vec<&str> = NAMES.to_vec();
        if reverse {
            names.reverse();
        }
        let mut members = std::collections::HashMap::new();
        for name in names {
            members.insert(
                name.to_string(),
                Document::Object(std::collections::HashMap::from([
                    (
                        "value".to_string(),
                        Document::String(format!("{name}-value")),
                    ),
                    ("flag".to_string(), Document::Bool(name.len() % 2 == 0)),
                ])),
            );
        }
        Document::Object(members)
    };

    let forwards = build(false);
    let backwards = build(true);
    assert_eq!(
        state::document_fingerprint(Some(&forwards)),
        state::document_fingerprint(Some(&backwards)),
        "case \"same members, opposite insertion order\": the rendering must not depend \
         on map order, got {:?} and {:?}",
        state::document_fingerprint(Some(&forwards)),
        state::document_fingerprint(Some(&backwards))
    );
    assert_eq!(
        state::parameter_fingerprint(&state::document_fingerprint(Some(&forwards))),
        state::parameter_fingerprint(&state::document_fingerprint(Some(&backwards))),
        "case \"same members, opposite insertion order\": the fingerprints must match"
    );

    // A document that differs in one member fingerprints differently, so the match is not
    // vacuous.
    let mut different = std::collections::HashMap::new();
    different.insert("alpha".to_string(), Document::String("other".to_string()));
    let different = Document::Object(different);
    assert_ne!(
        state::parameter_fingerprint(&state::document_fingerprint(Some(&forwards))),
        state::parameter_fingerprint(&state::document_fingerprint(Some(&different))),
        "case \"one member differs\": the fingerprints must differ"
    );
    assert_ne!(
        state::parameter_fingerprint(&state::document_fingerprint(Some(&forwards))),
        state::parameter_fingerprint(&state::document_fingerprint(None)),
        "case \"no document at all\": a document and no document must not fingerprint alike"
    );
}

/// Rule 8 at the state layer: the document-depth ceiling is a BOUND — the deepest
/// document it allows passes and one level past it is refused, however much deeper the
/// document goes, and the check itself costs no stack.
#[test]
fn the_document_depth_ceiling_is_a_bound() {
    use ::aws_smithy_types::Document;

    /// A document whose deepest node sits at `depth`: `depth - 1` arrays around a leaf.
    fn nested(depth: usize) -> Document {
        let mut document = Document::String("leaf".to_string());
        for _ in 1..depth {
            document = Document::Array(vec![document]);
        }
        document
    }

    for (case, depth, exceeded) in [
        ("one level", 1, false),
        ("one short of the ceiling", state::MAX_DOCUMENT_DEPTH - 1, false),
        ("exactly the ceiling", state::MAX_DOCUMENT_DEPTH, false),
        ("one past the ceiling", state::MAX_DOCUMENT_DEPTH + 1, true),
        ("far past the ceiling", state::MAX_DOCUMENT_DEPTH * 16, true),
    ] {
        let document = nested(depth);
        assert_eq!(
            state::document_depth_exceeded(Some(&document)),
            exceeded,
            "case {case:?}: a document {depth} levels deep against a ceiling of {} must \
             {}be refused",
            state::MAX_DOCUMENT_DEPTH,
            if exceeded { "" } else { "not " }
        );
    }
    assert!(
        !state::document_depth_exceeded(None),
        "case \"no document at all\": a request that carries none is never too deep"
    );
}
