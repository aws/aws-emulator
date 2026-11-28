// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for an agent profile's optimization goals: `CreateAgentGoal`,
//! the `GetAgentGoal` that reads one back, `DeleteAgentGoal` that removes one, and the
//! `CreateAgentProfile` / `GetAgentProfile` pair the goals hang off.
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. A goal is STORED under the profile it was created against: `CreateAgentGoal`
//!    answers a `GoalSummary` carrying the request's members, a freshly minted `UUID`
//!    and the `createdBy` / `createdAt` the service owns, and `GetAgentGoal` of that id
//!    reports the identical summary.
//! 2. `DeleteAgentGoal` removes EXACTLY the goal its `(profileArn, id)` pair resolves
//!    to: that goal then answers 404 to `GetAgentGoal` and to a repeat
//!    `DeleteAgentGoal`, while every other goal of the profile still resolves unchanged.
//! 3. A goal is reached only THROUGH its profile: the same goal id under another
//!    profile's ARN resolves to nothing, and a well-formed `profileArn` or goal `id`
//!    that names no stored resource is answered `404 ResourceNotFoundException`
//!    carrying the identifier as sent and the missing resource's type. A value the
//!    modelled `AgentProfileArn` / `UUID` pattern refuses never reaches the lookup: the
//!    boundary refuses it as a `400 ValidationException` naming the member.
//! 4. A `clientToken` makes `CreateAgentGoal` idempotent: the same token with the same
//!    parameters answers the goal the original request created, the same token with
//!    different parameters is a `409 ConflictException`, and a fresh token records a
//!    second goal.
//! 5. `CreateAgentProfile` persists what the request carried and `GetAgentProfile`
//!    reports it, plus the members the service DERIVES from the stored profile: a
//!    profile that names no aggregation scope is eligible for neither kind of
//!    generation and says so in `fieldErrors`; one that names a scope is eligible for
//!    both and reports no field errors.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do
//! in production. The HTTP client lives in `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{encode_path_segment, Answer, Client};

/// Every value the `Pillar` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="Pillar")`). Spelled out here so the suite pins
/// the catalog rather than reading it back out of the code under test.
pub(crate) const PILLARS: [&str; 5] = [
    "COST_OPTIMIZATION",
    "SECURITY",
    "RESILIENCE",
    "PERFORMANCE",
    "OPERATIONAL_EXCELLENCE",
];

/// A `RoleArn` the modelled pattern accepts
/// (`^arn:([a-z\-]+):iam::\d{12}:role/(service-role/)?[a-zA-Z0-9+=,.@\-_]+$`), in the
/// emulator's one account.
fn role_arn(name: &str) -> String {
    format!(
        "arn:aws:iam::{}:role/{name}",
        engine::emulator_account_id()
    )
}

/// A well-formed `AgentProfileArn` (the modelled
/// `^arn:aws…:wellarchitected:<region>:<account>:agent-profile/[a-zA-Z0-9_-]+$`) that no
/// stored profile carries.
pub(crate) fn absent_profile_arn(resource: &str) -> String {
    format!(
        "arn:aws:wellarchitected:us-west-2:{}:agent-profile/{resource}",
        engine::emulator_account_id()
    )
}

pub(crate) fn member<'a>(answer: &'a Answer, name: &str) -> &'a str {
    answer.body.get(name).and_then(Value::as_str).unwrap_or("")
}

/// A minimal valid `CreateAgentProfile` body; a caller overrides what its case varies.
/// `aggregation` is how many account/Region scopes the profile names.
pub(crate) fn profile_body(name: &str, pillars: &[&str], aggregation: usize) -> Value {
    json!({
        "name": name,
        "displayName": format!("{name} display"),
        "description": "an agent profile under test",
        "businessOverview": "a business overview under test",
        "pillars": pillars,
        "executionRoleArn": role_arn("AgentProfileExecution"),
        "aggregationConfiguration": (0..aggregation)
            .map(|index| json!({
                "accountId": engine::emulator_account_id(),
                "regions": [PILLARS_REGIONS[index % PILLARS_REGIONS.len()]],
                "accessRoleArn": role_arn(&format!("AgentProfileAccess{index}")),
            }))
            .collect::<Vec<Value>>(),
    })
}

/// Regions an `AggregationConfiguration` row names. Any Region id satisfies the
/// modelled `Regions` member (a plain string list); two are used so a profile with
/// several scopes is not several copies of one.
const PILLARS_REGIONS: [&str; 2] = ["us-east-1", "eu-west-1"];

pub(crate) async fn create_profile(client: &Client, body: Value) -> Answer {
    client
        .send("POST", "/api/v1/agent-profiles", Some(body))
        .await
}

pub(crate) async fn get_profile(client: &Client, profile_arn: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/api/v1/agent-profiles/{}",
                encode_path_segment(profile_arn)
            ),
            None,
        )
        .await
}

pub(crate) async fn create_goal(client: &Client, profile_arn: &str, body: Value) -> Answer {
    client
        .send(
            "POST",
            &format!(
                "/api/v1/agent-profiles/{}/goals",
                encode_path_segment(profile_arn)
            ),
            Some(body),
        )
        .await
}

/// `GET /api/v1/agent-profiles/{profileArn}/goals/{goalId}`.
///
/// `pub(crate)` because the listing suite beside this one reads a listed goal back
/// through the single-goal reader, and a goal is read by exactly one operation.
pub(crate) async fn get_goal(client: &Client, profile_arn: &str, id: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/api/v1/agent-profiles/{}/goals/{}",
                encode_path_segment(profile_arn),
                encode_path_segment(id)
            ),
            None,
        )
        .await
}

/// `DELETE /api/v1/agent-profiles/{profileArn}/goals/{goalId}`.
///
/// `pub(crate)` because the update suite beside this one deletes the goal it spent its
/// idempotency tokens on, and a goal is removed by exactly one operation.
pub(crate) async fn delete_goal(client: &Client, profile_arn: &str, id: &str) -> Answer {
    client
        .send(
            "DELETE",
            &format!(
                "/api/v1/agent-profiles/{}/goals/{}",
                encode_path_segment(profile_arn),
                encode_path_segment(id)
            ),
            None,
        )
        .await
}

/// The ARN of a profile this suite creates, with the create itself asserted.
///
/// `pub(crate)` because the context suite beside this one hangs its contexts off a
/// profile the same way, and a profile is minted by exactly one operation.
pub(crate) async fn profile_arn_of(client: &Client, name: &str, aggregation: usize) -> String {
    let answer = create_profile(client, profile_body(name, &["SECURITY"], aggregation)).await;
    assert_eq!(
        answer.status, 200,
        "case {name:?}: expected CreateAgentProfile 200, got {answer:?}"
    );
    let arn = member(&answer, "arn").to_string();
    assert!(
        arn.starts_with("arn:aws:wellarchitected:") && arn.contains(":agent-profile/"),
        "case {name:?}: arn must be an AgentProfileArn, got {arn:?}"
    );
    arn
}

/// The goal id a successful `CreateAgentGoal` answered with, checked against the
/// modelled `UUID` pattern
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
fn created_goal_id(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateAgentGoal 200, got {answer:?}"
    );
    let id = answer
        .body
        .get("goal")
        .and_then(|goal| goal.get("id"))
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
        "case {case:?}: goal id must match the modelled UUID pattern, got {id:?}"
    );
    id
}

/// The instant a create happened, as this service's protocol carries a timestamp:
/// epoch seconds (the same form `UpdatedAt` and `RecordedAt` take in the suites beside
/// this one). Checked against the TEST's own clock rather than a recorded value, which
/// no two runs share: the stamp has to be the moment of the call, not a constant.
pub(crate) fn assert_create_instant(value: Option<&Value>, case: &str, label: &str) {
    let seconds = value.and_then(Value::as_f64).unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch")
        .as_secs_f64();
    assert!(
        seconds > 0.0 && (now - seconds).abs() < 300.0,
        "case {case:?}: {label} must be the epoch-seconds instant of the call \
         (about {now}), got {seconds}"
    );
}

/// A tag list in one stable order, so a comparison does not depend on the order the
/// request happened to list them in: tags are keyed by `Tag$key`, and this service
/// reports a resource's tags in key order.
pub(crate) fn tags_by_key(value: Option<&Value>) -> Vec<(String, String)> {
    let mut tags: Vec<(String, String)> = value
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag.get("key")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        tag.get("value")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    tags.sort();
    tags
}

/// Whether the response shape `reader` projects an agent profile through declares
/// `member`.
///
/// The three projections of one stored profile are three shapes, and the model gives them
/// different member lists (`smithy_query(query="operation",
/// operation="CreateAgentProfile,GetAgentProfile")`, `smithy_query(query="shape",
/// shape="AgentProfileSummary")`):
///
/// * `tags` — `CreateAgentProfileResponse` and `GetAgentProfileResponse` only.
/// * `profileType` and `organizationalAggregationConfiguration` —
///   `GetAgentProfileResponse` and `AgentProfileSummary` only.
///
/// Held in one place so each suite asserts the members ITS shape declares, and the
/// absence of the ones it does not.
pub(crate) fn declares(reader: &str, member: &str) -> bool {
    match member {
        "tags" => matches!(reader, "CreateAgentProfile" | "GetAgentProfile"),
        "profileType" | "organizationalAggregationConfiguration" => {
            matches!(reader, "GetAgentProfile" | "ListAgentProfiles")
        }
        other => panic!("no projection rule is held for the member {other:?}"),
    }
}

/// Assert that `answer` projects the agent profile `sent` created: every member the
/// request carried comes back as sent, the members the SERVICE owns are its own, and the
/// members DERIVED from the stored profile answer for that profile's aggregation scope.
///
/// The one place this projection is pinned, so a change to it is re-pinned once:
/// `CreateAgentProfile`, `GetAgentProfile` and the `AgentProfileSummary` entries
/// `ListAgentProfiles` reports all answer the same stored record through their own shape,
/// and the suites in this crate check each against it. `projection` is the JSON carrying
/// it — a single-profile answer's body, or one entry of a list's `items`. `reader` names
/// the operation whose answer is under test, `expected_protection` the
/// `deletionProtection` the row must read back (the value sent, or the modelled
/// `default: true`), and `eligible` whether the profile names a scope to analyse.
///
/// Which members the answer must carry follows from the SHAPE the `reader` projects the
/// profile through, which is what [`declares`] holds: the model gives `tags` to the create
/// and the read responses only, and `profileType` with
/// `organizationalAggregationConfiguration` to the read response and the summary only.
pub(crate) fn assert_profile_projection(
    projection: &Value,
    sent: &Value,
    case: &str,
    reader: &str,
    expected_protection: bool,
    eligible: bool,
) {
    for member_name in [
        "name",
        "displayName",
        "description",
        "businessOverview",
        "pillars",
        "executionRoleArn",
        "aggregationConfiguration",
    ] {
        assert_eq!(
            projection.get(member_name),
            sent.get(member_name),
            "case {case:?}/{reader}: expected {member_name} as sent, got {projection:?}"
        );
    }
    if declares(reader, "tags") {
        assert_eq!(
            tags_by_key(projection.get("tags")),
            tags_by_key(sent.get("tags")),
            "case {case:?}/{reader}: expected the tags as sent, got {projection:?}"
        );
    } else {
        assert_eq!(
            projection.get("tags"),
            None,
            "case {case:?}/{reader}: the shape this projection goes through declares no \
             tags member, got {projection:?}"
        );
    }
    if declares(reader, "profileType") {
        // Which kind of profile the create minted: `CreateAgentProfile` takes the
        // execution role and the aggregation configuration `@required`, which is what
        // `ProfileType$STANDARD` is defined by (`smithy_query(query="shape",
        // shape="ProfileType")`), so every projection of a profile this create minted
        // reports that kind.
        assert_eq!(
            projection.get("profileType").and_then(Value::as_str),
            Some("STANDARD"),
            "case {case:?}/{reader}: expected profileType STANDARD, got {projection:?}"
        );
        // The organizational monitoring boundary is "present only for organizational
        // profiles", so a STANDARD profile reports none.
        assert_eq!(
            projection.get("organizationalAggregationConfiguration"),
            None,
            "case {case:?}/{reader}: a STANDARD profile carries no organizational \
             boundary, got {projection:?}"
        );
    }
    assert_eq!(
        projection.get("deletionProtection").and_then(Value::as_bool),
        Some(expected_protection),
        "case {case:?}/{reader}: expected deletionProtection {expected_protection}, \
         got {projection:?}"
    );
    assert_eq!(
        projection.get("createdBy").and_then(Value::as_str),
        Some(engine::emulator_account_id()),
        "case {case:?}/{reader}: createdBy must be the account the request is attributed \
         to, got {projection:?}"
    );
    assert_create_instant(projection.get("createdAt"), case, "createdAt");

    // A profile is eligible for generation exactly when it names a scope to analyse, and
    // `fieldErrors` says so when it names none.
    for derived in [
        "eligibleForScheduledGeneration",
        "eligibleForArchitectureGeneration",
    ] {
        assert_eq!(
            projection.get(derived).and_then(Value::as_bool),
            Some(eligible),
            "case {case:?}/{reader}: expected {derived} {eligible}, got {projection:?}"
        );
    }
    let field_errors = projection.get("fieldErrors");
    if eligible {
        assert!(
            field_errors.is_none_or(|errors| errors
                .as_object()
                .is_some_and(serde_json::Map::is_empty)),
            "case {case:?}/{reader}: an eligible profile reports no fieldErrors, \
             got {projection:?}"
        );
    } else {
        assert_eq!(
            field_errors
                .and_then(|errors| errors.get("aggregationConfiguration"))
                .and_then(Value::as_str),
            Some(
                "At least one aggregation configuration is required to generate \
                 recommendations."
            ),
            "case {case:?}/{reader}: an ineligible profile must say which member leaves \
             it so, got {projection:?}"
        );
    }
}

/// The published `@length` bounds of `GoalSummary$title` and `GoalSummary$description`,
/// both `SensitiveString` — `@length(1, 1000)` and `@length(1, 4000)` respectively
/// (`smithy_query(query="shape", shape="GoalSummary")`). The table below sits on both
/// ends of each, so a goal is exercised at the published extremes and not only in the
/// middle.
pub(crate) const TITLE_MAX: usize = 1000;
/// See [`TITLE_MAX`].
pub(crate) const DESCRIPTION_MAX: usize = 4000;

/// The rows every goal test spans: the pillar sets a goal may name (one, several, and
/// the whole published catalog, which is the modelled `@length(1, 5)` maximum), with and
/// without the optional `description`, and titles and descriptions of different form and
/// size (both published `@length` bounds of each among them). No two rows share a title,
/// so an answer served from the wrong record is caught.
///
/// `pub(crate)` because the listing suite and the single-goal read suite beside this one
/// span the same class of goal, and the table a goal rule is asserted over is spelled
/// ONCE here rather than copied per suite.
pub(crate) fn goal_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one-pillar-no-description",
            json!({ "pillars": [PILLARS[1]], "title": "Harden the perimeter" }),
        ),
        (
            "two-pillars-with-description",
            json!({
                "pillars": [PILLARS[0], PILLARS[3]],
                "title": "Right-size the fleet",
                "description": "Trade idle capacity for latency headroom",
            }),
        ),
        (
            "every-published-pillar",
            json!({
                "pillars": PILLARS,
                "title": "Review the whole framework",
                "description": "One goal spanning every pillar the model publishes",
            }),
        ),
        (
            "pillars-the-issue-did-not-cite",
            json!({
                "pillars": [PILLARS[4], PILLARS[2]],
                "title": "Rehearse the failover runbook",
            }),
        ),
        (
            "shortest-title-and-description",
            json!({ "pillars": [PILLARS[2]], "title": "x", "description": "d" }),
        ),
        (
            "longest-title-and-description",
            json!({
                "pillars": [PILLARS[3]],
                "title": "t".repeat(TITLE_MAX),
                "description": "d".repeat(DESCRIPTION_MAX),
            }),
        ),
    ]
}

/// Rules 1 and 2: every row round-trips through `GetAgentGoal`, and deleting one row
/// removes exactly that row — it answers 404 afterwards (to the read and to a repeat
/// delete) while every goal not yet deleted still reports its original summary.
#[tokio::test]
async fn a_created_goal_round_trips_and_delete_removes_exactly_that_goal() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-host", 1).await;

    // Create every row under one profile, remembering the summary each create answered.
    let mut created: Vec<(&str, String, Value)> = Vec::new();
    for (case, body) in goal_rows() {
        let answer = create_goal(&client, &profile_arn, body.clone()).await;
        let id = created_goal_id(&answer, case);
        let goal = answer.body.get("goal").cloned().unwrap_or(Value::Null);

        assert_eq!(
            goal.get("profileArn").and_then(Value::as_str),
            Some(profile_arn.as_str()),
            "case {case:?}: the goal must name the profile it was created against, got {goal:?}"
        );
        assert_eq!(
            goal.get("pillars"),
            body.get("pillars"),
            "case {case:?}: expected the pillars as sent, got {goal:?}"
        );
        assert_eq!(
            goal.get("title"),
            body.get("title"),
            "case {case:?}: expected the title as sent, got {goal:?}"
        );
        assert_eq!(
            goal.get("description").and_then(Value::as_str),
            body.get("description").and_then(Value::as_str),
            "case {case:?}: expected the description as sent, got {goal:?}"
        );
        assert_eq!(
            goal.get("createdBy").and_then(Value::as_str),
            Some(engine::emulator_account_id()),
            "case {case:?}: createdBy must be the account the request is attributed to, \
             got {goal:?}"
        );
        assert_create_instant(goal.get("createdAt"), case, "createdAt");
        assert!(
            goal.get("lastModifiedAt").is_none() && goal.get("lastModifiedBy").is_none(),
            "case {case:?}: a goal nobody has modified reports no lastModified members, \
             got {goal:?}"
        );
        assert!(
            created.iter().all(|(_, other, _)| other != &id),
            "case {case:?}: every goal gets its own id, got a repeat of {id:?}"
        );

        let fetched = get_goal(&client, &profile_arn, &id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetAgentGoal 200, got {fetched:?}"
        );
        assert_eq!(
            fetched.body.get("goal"),
            Some(&goal),
            "case {case:?}: GetAgentGoal must report the summary the create answered, \
             got {fetched:?}"
        );

        created.push((case, id, goal));
    }

    // Delete the rows one at a time: each delete takes exactly its own goal with it.
    for index in 0..created.len() {
        let (case, id, _) = created[index].clone();
        let deleted = delete_goal(&client, &profile_arn, &id).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteAgentGoal 200, got {deleted:?}"
        );
        assert!(
            deleted.body.as_object().is_none_or(serde_json::Map::is_empty),
            "case {case:?}: DeleteAgentGoal answers no members, got {deleted:?}"
        );

        let read_back = get_goal(&client, &profile_arn, &id).await;
        assert_eq!(
            read_back.status, 404,
            "case {case:?}: a deleted goal must not resolve, got {read_back:?}"
        );
        assert_eq!(
            read_back.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {read_back:?}"
        );

        let again = delete_goal(&client, &profile_arn, &id).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: deleting a goal twice must answer 404, got {again:?}"
        );
        assert_eq!(
            again.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {again:?}"
        );

        // Every goal not yet deleted is untouched by this delete.
        for (other_case, other_id, other_goal) in &created[index + 1..] {
            let still_there = get_goal(&client, &profile_arn, other_id).await;
            assert_eq!(
                still_there.status, 200,
                "case {case:?}: deleting it must leave {other_case:?} in place, \
                 got {still_there:?}"
            );
            assert_eq!(
                still_there.body.get("goal"),
                Some(other_goal),
                "case {case:?}: {other_case:?} must still report its own summary, \
                 got {still_there:?}"
            );
        }
    }

    // The profile the goals hung off outlives them all.
    let profile = get_profile(&client, &profile_arn).await;
    assert_eq!(
        profile.status, 200,
        "case \"host-profile\": deleting its goals must not delete the profile, got {profile:?}"
    );
}

/// Rule 3, across profiles: the same goal id under a DIFFERENT profile's ARN resolves to
/// nothing, for the read and for the delete, and the goal stays where it is. The rows
/// are each profile of a pair, so neither direction is privileged.
#[tokio::test]
async fn a_goal_is_reached_only_through_the_profile_that_holds_it() {
    let client = Client::new();
    let first = profile_arn_of(&client, "first-profile", 1).await;
    let second = profile_arn_of(&client, "second-profile", 0).await;
    assert_ne!(
        first, second,
        "case \"two-profiles\": each CreateAgentProfile mints its own ARN"
    );

    let mut goals: Vec<(&str, String, String)> = Vec::new();
    for (case, arn) in [("first", &first), ("second", &second)] {
        let answer = create_goal(
            &client,
            arn,
            json!({ "pillars": ["SECURITY"], "title": format!("{case} profile goal") }),
        )
        .await;
        let id = created_goal_id(&answer, case);
        goals.push((case, arn.clone(), id));
    }

    for (case, owner, id) in &goals {
        let other = if owner == &first { &second } else { &first };
        for (operation, answer) in [
            ("GetAgentGoal", get_goal(&client, other, id).await),
            ("DeleteAgentGoal", delete_goal(&client, other, id).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}/{operation}: a goal of another profile must not resolve, \
                 got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceType"),
                "GOAL",
                "case {case:?}/{operation}: expected ResourceType GOAL, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceId"),
                format!("{other}.{id}"),
                "case {case:?}/{operation}: ResourceId must name the profile asked and the \
                 id as sent, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "Message"),
                format!(
                    "[ResourceNotFound] No goal with ID {id} in an agent profile with ARN \
                     {other} was found in account {}.",
                    engine::emulator_account_id()
                ),
                "case {case:?}/{operation}: expected the goal/profile/account not-found \
                 message, got {answer:?}"
            );
        }

        let still_there = get_goal(&client, owner, id).await;
        assert_eq!(
            still_there.status, 200,
            "case {case:?}: the goal must still resolve under its own profile, \
             got {still_there:?}"
        );
    }
}

/// Rule 3, for an identifier that names nothing: a path-bound `profileArn` is RESOLVED,
/// so a well-formed value naming no stored profile is the lookup's 404 on every
/// operation that resolves one — and nothing is recorded against it. The rows differ
/// along the ARN's resource part, Region and partition, so the lookup is exact rather
/// than a match on the resource id alone.
#[tokio::test]
async fn an_unresolvable_profile_arn_is_a_not_found_naming_the_arn_and_the_account() {
    let client = Client::new();
    // One profile with one goal exists, so each row below is a real miss.
    let present = profile_arn_of(&client, "present-profile", 1).await;
    let present_goal = created_goal_id(
        &create_goal(
            &client,
            &present,
            json!({ "pillars": ["SECURITY"], "title": "a stored goal" }),
        )
        .await,
        "present",
    );

    let rows: Vec<(&str, String)> = vec![
        ("absent-hex-resource", absent_profile_arn(&"0".repeat(32))),
        ("absent-name-resource", absent_profile_arn("some_profile-2")),
        (
            "absent-in-another-region",
            format!(
                "arn:aws:wellarchitected:eu-west-1:{}:agent-profile/{}",
                engine::emulator_account_id(),
                "f".repeat(32)
            ),
        ),
        (
            "absent-in-another-partition",
            format!(
                "arn:aws-us-gov:wellarchitected:us-gov-west-1:{}:agent-profile/{}",
                engine::emulator_account_id(),
                "a".repeat(32)
            ),
        ),
    ];

    for (case, profile_arn) in rows {
        assert_ne!(
            profile_arn, present,
            "case {case:?}: the row must not name the profile that exists"
        );
        for (operation, answer) in [
            ("GetAgentProfile", get_profile(&client, &profile_arn).await),
            (
                "CreateAgentGoal",
                create_goal(
                    &client,
                    &profile_arn,
                    json!({ "pillars": ["SECURITY"], "title": "no such profile" }),
                )
                .await,
            ),
            (
                "GetAgentGoal",
                get_goal(&client, &profile_arn, &present_goal).await,
            ),
            (
                "DeleteAgentGoal",
                delete_goal(&client, &profile_arn, &present_goal).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}/{operation}: expected HTTP 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceType"),
                "AGENT_PROFILE",
                "case {case:?}/{operation}: expected ResourceType AGENT_PROFILE, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceId"),
                profile_arn,
                "case {case:?}/{operation}: ResourceId must be the ARN as sent, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "Message"),
                format!(
                    "[ResourceNotFound] No agent profile with ARN {profile_arn} was found in \
                     account {}.",
                    engine::emulator_account_id()
                ),
                "case {case:?}/{operation}: expected the ARN/account not-found message, \
                 got {answer:?}"
            );
        }
    }

    // Nothing was recorded against any absent profile, and the stored goal is untouched.
    assert_eq!(
        client.store().lock().agent_goals.len(),
        1,
        "case \"absent-profiles\": a refused CreateAgentGoal records nothing"
    );
    let fetched = get_goal(&client, &present, &present_goal).await;
    assert_eq!(
        fetched.status, 200,
        "case \"present\": the stored goal must still resolve, got {fetched:?}"
    );
}

/// Rule 3, for a goal id that names nothing: the id is resolved within its profile, so
/// every well-formed id no goal of the profile carries is the same 404, naming the
/// composite `<profileArn>.<id>` as sent. The rows differ along the id's hex digits,
/// including both extremes of the modelled `UUID` pattern's value space.
#[tokio::test]
async fn an_unresolvable_goal_id_is_a_not_found_naming_the_composite_id() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "lookup-profile", 1).await;
    let present_goal = created_goal_id(
        &create_goal(
            &client,
            &profile_arn,
            json!({ "pillars": ["SECURITY"], "title": "a stored goal" }),
        )
        .await,
        "present",
    );

    let rows: Vec<(&str, String)> = vec![
        (
            "all-zeros",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "all-f",
            "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
        ),
        (
            "mixed-hex",
            "0123abcd-4567-89ef-0123-456789abcdef".to_string(),
        ),
        (
            "version-4-looking",
            "3f2504e0-4f89-41d3-9a0c-0305e82c3301".to_string(),
        ),
    ];

    for (case, id) in rows {
        assert_ne!(
            id, present_goal,
            "case {case:?}: the row must not name the goal that exists"
        );
        for (operation, answer) in [
            ("GetAgentGoal", get_goal(&client, &profile_arn, &id).await),
            (
                "DeleteAgentGoal",
                delete_goal(&client, &profile_arn, &id).await,
            ),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {case:?}/{operation}: expected HTTP 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ResourceNotFoundException",
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceType"),
                "GOAL",
                "case {case:?}/{operation}: expected ResourceType GOAL, got {answer:?}"
            );
            assert_eq!(
                member(&answer, "ResourceId"),
                format!("{profile_arn}.{id}"),
                "case {case:?}/{operation}: ResourceId must be the composite as sent, \
                 got {answer:?}"
            );
        }
    }

    let fetched = get_goal(&client, &profile_arn, &present_goal).await;
    assert_eq!(
        fetched.status, 200,
        "case \"present\": the stored goal must still resolve, got {fetched:?}"
    );
}

/// Rule 3, for an identifier the MODEL refuses: `profileArn` and `id` carry modelled
/// patterns, so a value outside either is refused by the boundary walk as a 400
/// `ValidationException` naming the member — never answered as a lookup miss, and never
/// recorded. The rows differ along how each value breaks its pattern.
#[tokio::test]
async fn a_malformed_path_identifier_is_refused_as_a_validation_exception() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "refusal-profile", 1).await;
    let present_goal = created_goal_id(
        &create_goal(
            &client,
            &profile_arn,
            json!({ "pillars": ["SECURITY"], "title": "a stored goal" }),
        )
        .await,
        "present",
    );

    // Values the modelled `AgentProfileArn` pattern refuses.
    let bad_arns: Vec<(&str, String)> = vec![
        ("not-an-arn-at-all", "not-an-agent-profile".to_string()),
        (
            "another-service",
            format!(
                "arn:aws:iam::{}:role/NotAProfile",
                engine::emulator_account_id()
            ),
        ),
        (
            "wrong-resource-kind",
            format!(
                "arn:aws:wellarchitected:us-west-2:{}:profile/{}",
                engine::emulator_account_id(),
                "0".repeat(32)
            ),
        ),
        (
            "account-too-short",
            "arn:aws:wellarchitected:us-west-2:12345:agent-profile/p".to_string(),
        ),
    ];
    for (case, arn) in bad_arns {
        for (operation, answer) in [
            ("GetAgentProfile", get_profile(&client, &arn).await),
            (
                "CreateAgentGoal",
                create_goal(
                    &client,
                    &arn,
                    json!({ "pillars": ["SECURITY"], "title": "a refused goal" }),
                )
                .await,
            ),
            (
                "GetAgentGoal",
                get_goal(&client, &arn, &present_goal).await,
            ),
            (
                "DeleteAgentGoal",
                delete_goal(&client, &arn, &present_goal).await,
            ),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {case:?}/{operation}: expected HTTP 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}/{operation}: expected ValidationException, got {answer:?}"
            );
            assert!(
                member(&answer, "Message").contains("profileArn"),
                "case {case:?}/{operation}: the refusal must name the member, got {answer:?}"
            );
        }
    }

    // Values the modelled `UUID` pattern refuses.
    let bad_ids: Vec<(&str, String)> = vec![
        ("no-dashes", "0".repeat(32)),
        (
            "uppercase-hex",
            "AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA".to_string(),
        ),
        (
            "one-character-short",
            "00000000-0000-0000-0000-00000000000".to_string(),
        ),
        (
            "one-character-long",
            "00000000-0000-0000-0000-0000000000000".to_string(),
        ),
        ("not-hex-at-all", "not-a-goal-id".to_string()),
    ];
    for (case, id) in bad_ids {
        for (operation, answer) in [
            ("GetAgentGoal", get_goal(&client, &profile_arn, &id).await),
            (
                "DeleteAgentGoal",
                delete_goal(&client, &profile_arn, &id).await,
            ),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {case:?}/{operation}: expected HTTP 400, got {answer:?}"
            );
            assert_eq!(
                answer.error_code, "ValidationException",
                "case {case:?}/{operation}: expected ValidationException, got {answer:?}"
            );
            assert!(
                member(&answer, "Message").contains("id"),
                "case {case:?}/{operation}: the refusal must name the member, got {answer:?}"
            );
        }
    }

    // Nothing the boundary refused reached the store, and the stored goal is untouched.
    assert_eq!(
        client.store().lock().agent_goal_count(&profile_arn),
        1,
        "case \"refused-rows\": a refused request records and removes nothing"
    );
    let fetched = get_goal(&client, &profile_arn, &present_goal).await;
    assert_eq!(
        fetched.status, 200,
        "case \"present\": the stored goal must still resolve, got {fetched:?}"
    );
}

/// Rule 4: `clientToken` replay. The rows are the three outcomes a token can have —
/// replayed with the same parameters, replayed with different ones, and a fresh token —
/// each checked against what the store holds afterwards.
#[tokio::test]
async fn a_client_token_makes_the_goal_create_idempotent() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "token-profile", 1).await;

    let original = json!({
        "pillars": ["SECURITY", "RESILIENCE"],
        "title": "Patch the fleet",
        "description": "the original request",
        "clientToken": "goal-token-1",
    });
    let first = create_goal(&client, &profile_arn, original.clone()).await;
    let id = created_goal_id(&first, "original");

    let replay = create_goal(&client, &profile_arn, original.clone()).await;
    assert_eq!(
        replay.status, 200,
        "case \"same-token-same-parameters\": expected 200, got {replay:?}"
    );
    assert_eq!(
        replay.body.get("goal"),
        first.body.get("goal"),
        "case \"same-token-same-parameters\": the replay must answer the original goal, \
         got {replay:?}"
    );

    let mut changed = original.clone();
    changed["title"] = json!("A different goal");
    let conflict = create_goal(&client, &profile_arn, changed).await;
    assert_eq!(
        conflict.status, 409,
        "case \"same-token-other-parameters\": expected 409, got {conflict:?}"
    );
    assert_eq!(
        conflict.error_code, "ConflictException",
        "case \"same-token-other-parameters\": expected ConflictException, got {conflict:?}"
    );
    assert_eq!(
        member(&conflict, "ResourceId"),
        format!("{profile_arn}.{id}"),
        "case \"same-token-other-parameters\": the conflict must name the goal the token \
         already created, got {conflict:?}"
    );

    let mut fresh = original;
    fresh["clientToken"] = json!("goal-token-2");
    let second = create_goal(&client, &profile_arn, fresh).await;
    let second_id = created_goal_id(&second, "fresh-token");
    assert_ne!(
        second_id, id,
        "case \"fresh-token\": a fresh token records a second goal"
    );

    assert_eq!(
        client.store().lock().agent_goal_count(&profile_arn),
        2,
        "case \"token-rows\": exactly the two created goals are stored"
    );

    // The replay is of the STORED goal: deleting it makes the token's goal unresolvable,
    // and the second goal is untouched.
    let deleted = delete_goal(&client, &profile_arn, &id).await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete-the-token-goal\": expected 200, got {deleted:?}"
    );
    let after = get_goal(&client, &profile_arn, &second_id).await;
    assert_eq!(
        after.status, 200,
        "case \"delete-the-token-goal\": the other goal must still resolve, got {after:?}"
    );
    assert_eq!(
        client.store().lock().agent_goal_count(&profile_arn),
        1,
        "case \"delete-the-token-goal\": only the deleted goal is gone"
    );
}

/// Rule 5: the profile round trip, over rows that differ along how many aggregation
/// scopes the profile names — none (the modelled `@length(min: 0)` floor), one and two —
/// because that is what the derived eligibility members report on.
#[tokio::test]
async fn a_created_agent_profile_round_trips_with_its_derived_members() {
    let client = Client::new();

    let rows: [(&str, usize, &[&str]); 4] = [
        ("no-aggregation-scope", 0, &["SECURITY"]),
        ("one-aggregation-scope", 1, &["COST_OPTIMIZATION"]),
        ("two-aggregation-scopes", 2, &PILLARS),
        (
            "two-pillars-one-scope",
            1,
            &["OPERATIONAL_EXCELLENCE", "PERFORMANCE"],
        ),
    ];

    for (case, aggregation, pillars) in rows {
        let mut body = profile_body(case, pillars, aggregation);
        body["tags"] = json!([
            { "key": "team", "value": case },
            { "key": "cost-centre", "value": "4711" },
        ]);
        // `deletionProtection` is modelled `default: true`; one row sends it explicitly
        // so both the default and a sent value are covered.
        let expected_protection = aggregation % 2 == 0;
        if !expected_protection {
            body["deletionProtection"] = json!(false);
        }

        let created = create_profile(&client, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateAgentProfile 200, got {created:?}"
        );
        let arn = member(&created, "arn").to_string();
        let fetched = get_profile(&client, &arn).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetAgentProfile 200, got {fetched:?}"
        );

        for (reader, answer) in [("CreateAgentProfile", &created), ("GetAgentProfile", &fetched)] {
            assert_profile_projection(
                &answer.body,
                &body,
                case,
                reader,
                expected_protection,
                aggregation > 0,
            );
        }

        // The create and the read answer the same stored profile through two shapes, and
        // the read's declares one member the create's does not: the kind the profile was
        // minted as (`GetAgentProfileResponse$profileType`).
        let mut read_back = fetched.body.clone();
        let projected_type = read_back
            .as_object_mut()
            .expect("the answer is an object")
            .remove("profileType");
        assert_eq!(
            read_back, created.body,
            "case {case:?}: GetAgentProfile must report what the create answered"
        );
        assert_eq!(
            projected_type.as_ref().and_then(Value::as_str),
            Some("STANDARD"),
            "case {case:?}: the read reports the kind the create minted, \
             got {projected_type:?}"
        );
    }
}
