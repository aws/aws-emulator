// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the AGENT-PLANE update that supplies no writable member.
//!
//! The rule these pin, over tables of inputs rather than one request: an update whose every
//! WRITABLE member is optional treats a request that supplies NONE of them as asking for no
//! change — it reports the resource exactly as it stands and records no modification, so
//! `lastModifiedBy` and `lastModifiedAt` keep whatever they held (absent on a resource
//! nothing has modified yet) — while a member the request DOES supply is a write even at the
//! emptiest value the model allows it, and that write IS the modification the resource
//! reports.
//!
//! The suites cover the three agent-plane updates the rule reaches, each of which models
//! every writable member as optional with only the path identifiers `@required`
//! (`smithy_query(query="operation", operation="UpdateAgentContext")` and the same for
//! `UpdateAgentGoal` and `UpdateAgentProfile`): `UpdateAgentContext` (`title`, `content`),
//! `UpdateAgentGoal` (`pillars`, `title`, `description`) and `UpdateAgentProfile`
//! (`displayName`, `description`, `businessOverview`, `pillars`, `executionRoleArn`,
//! `aggregationConfiguration`, `deletionProtection`). The same rule over the workload plane
//! is pinned by `super::no_op_updates`.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The request builders are the ones the suites about each update own, and the
//! resources these rows drive are created through the shared create helpers.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use super::agent_context_updates::update_context;
use super::agent_contexts::{context_rows, create_context, get_context, id_of, summary};
use super::agent_goal_updates::update_goal;
use super::agent_goals::{
    create_goal, create_profile, get_goal, get_profile, goal_rows, member, profile_arn_of,
    profile_body, PILLARS,
};
use super::agent_profile_updates::update_profile;
use super::workload_harness::{Answer, Client};

/// How far back a stored modification instant is moved before the request under test, so a
/// write the handler makes to it is observable whatever the clock's granularity between two
/// in-process requests — and so a write it must NOT make cannot hide behind that
/// granularity either.
const AGE: Duration = Duration::from_secs(3600);

/// The two members every agent resource under test reports a modification through
/// (`ContextSummary$lastModifiedBy` / `$lastModifiedAt` and their namesakes on a goal and a
/// profile): what an update records, and what a request that writes nothing must leave
/// exactly where it stood.
const MODIFICATION_MEMBERS: [&str; 2] = ["lastModifiedBy", "lastModifiedAt"];

/// The stored agent resource a row drives, named by the identifiers that reach its record.
///
/// The three records hold the same pair of modification members, so the suite reaches that
/// pair through ONE accessor rather than a copy of each read per resource kind.
enum Stored {
    Context { profile_arn: String, id: String },
    Goal { profile_arn: String, id: String },
    Profile { arn: String },
}

impl Stored {
    /// The instant the store holds as this resource's last modification, read through the
    /// shared state so two calls can be ordered without parsing a wire timestamp.
    fn modified_at(&self, client: &Client) -> Option<SystemTime> {
        self.with_modified_at(client, |instant| *instant)
    }

    /// Move the stored modification instant back by `age`, so a write the handler makes to
    /// it is observable whatever the clock's granularity between two in-process requests.
    /// A resource nothing has modified has no instant to move. Only the SEED reaches state
    /// directly; the write under test is the handler's.
    fn age(&self, client: &Client, age: Duration) {
        self.with_modified_at(client, |instant| {
            if let Some(recorded) = instant.as_mut() {
                *recorded -= age;
            }
        });
    }

    /// Hand `visit` the record's modification instant, resolved through the one state the
    /// handlers share.
    fn with_modified_at<R>(
        &self,
        client: &Client,
        visit: impl FnOnce(&mut Option<SystemTime>) -> R,
    ) -> R {
        let state = client.store();
        let mut store = state.lock();
        let instant = match self {
            Stored::Context { profile_arn, id } => {
                &mut store
                    .agent_contexts
                    .get_mut(profile_arn)
                    .expect("the profile under test must hold the context")
                    .get_mut(id)
                    .expect("the context under test must be stored")
                    .last_modified_at
            }
            Stored::Goal { profile_arn, id } => {
                &mut store
                    .agent_goals
                    .get_mut(profile_arn)
                    .expect("the profile under test must hold the goal")
                    .get_mut(id)
                    .expect("the goal under test must be stored")
                    .last_modified_at
            }
            Stored::Profile { arn } => {
                &mut store
                    .agent_profiles
                    .get_mut(arn)
                    .expect("the profile under test must be stored")
                    .last_modified_at
            }
        };
        visit(instant)
    }
}

/// The body of an accepted answer, with the status asserted first so a refusal is reported
/// as itself rather than as a missing member.
fn accepted(answer: &Answer, case: &str, operation: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}/{operation}: expected 200, got {answer:?}"
    );
    answer.body.clone()
}

/// The resource an accepted answer carries under `name` (`context` on the context
/// operations, `goal` on the goal ones). An `UpdateAgentProfile` / `GetAgentProfile`
/// response IS the profile, so the profile suite reads [`accepted`] directly.
fn carried(answer: &Answer, name: &str, case: &str, operation: &str) -> Value {
    accepted(answer, case, operation)
        .get(name)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}/{operation}: expected a {name}, got {answer:?}"))
}

/// Whether a resource reports a modification at all — the member is absent (or null) on one
/// nothing has modified yet.
fn records_a_modification(resource: &Value) -> bool {
    resource
        .get("lastModifiedAt")
        .is_some_and(|instant| !instant.is_null())
}

/// The rule's no-change half: the resource the no-op answered, and the one the reader
/// reports afterwards, are both exactly what stood before it — modification members
/// included.
///
/// `not_answered` names the members the UPDATE's own response shape does not declare,
/// which the READ's does: they cannot appear in its answer, so they are dropped from the
/// expectation the answer is compared against and asserted absent from it.
fn assert_no_modification_recorded(
    before: &Value,
    answered: &Value,
    read_back: &Value,
    case: &str,
    not_answered: &[&str],
) {
    let mut as_the_update_answers_it = before.clone();
    let members = as_the_update_answers_it
        .as_object_mut()
        .expect("the resource is an object");
    for name in not_answered {
        members.remove(*name);
        assert_eq!(
            answered.get(*name),
            None,
            "case {case:?}: the update's response shape declares no {name} member, \
             got {answered:?}"
        );
    }
    for name in MODIFICATION_MEMBERS {
        assert_eq!(
            answered.get(name),
            before.get(name),
            "case {case:?}: a request that supplies no writable member must leave {name} at \
             {:?}, got {:?}",
            before.get(name),
            answered.get(name)
        );
    }
    assert_eq!(
        answered, &as_the_update_answers_it,
        "case {case:?}: expected the resource as it stands {as_the_update_answers_it:?}, \
         got {answered:?}"
    );
    assert_eq!(
        read_back, before,
        "case {case:?}: expected the stored resource unchanged {before:?}, got {read_back:?}"
    );
}

/// The rule's other half: a supplied member is a write, so the modification IS recorded —
/// attributed to the one account this emulator serves, and stamped later than whatever
/// instant stood before the call.
fn assert_modification_recorded(
    answered: &Value,
    before: Option<SystemTime>,
    after: Option<SystemTime>,
    case: &str,
) {
    assert_eq!(
        answered.get("lastModifiedBy").and_then(Value::as_str),
        Some(engine::emulator_account_id()),
        "case {case:?}: a supplied member is a write, so lastModifiedBy must be the account \
         the request is attributed to, got {answered:?}"
    );
    let Some(recorded) = after else {
        panic!(
            "case {case:?}: a supplied member is a write, so the modification instant must be \
             recorded, got {after:?}"
        )
    };
    assert!(
        before.is_none_or(|previous| recorded > previous),
        "case {case:?}: a supplied member is a write, so the modification instant must move \
         past {before:?}, got {recorded:?}"
    );
    assert!(
        records_a_modification(answered),
        "case {case:?}: the answer must report the modification it recorded, got {answered:?}"
    );
}

/// A context of `profile_arn` created from `body`, and its minted id.
async fn stored_context(client: &Client, profile_arn: &str, body: Value, case: &str) -> String {
    let created = create_context(client, profile_arn, body).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected CreateAgentContext 200, got {created:?}"
    );
    id_of(summary(&created))
}

/// A goal of `profile_arn` created from `body`, and its minted id.
async fn stored_goal(client: &Client, profile_arn: &str, body: Value, case: &str) -> String {
    let created = create_goal(client, profile_arn, body).await;
    let goal = carried(&created, "goal", case, "CreateAgentGoal");
    let id = goal
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !id.is_empty(),
        "case {case:?}: the created goal must carry its @required id, got {goal:?}"
    );
    id
}

/// What was written to a resource BEFORE the no-op arrives: nothing, or one update body.
/// The rows span the resource nothing has modified and the resource an update has stamped.
type Priors<'a> = [(&'a str, Option<Value>)];

/// `UpdateAgentContext`: a request that supplies neither `title` nor `content` reports the
/// context as it stands and records no modification.
///
/// The rows cross what the CONTEXT holds (the shared create table, which spans the shapes a
/// `content` can take and both published `title` bounds) with what was written to it before
/// the no-op.
#[tokio::test]
async fn a_context_update_that_supplies_no_member_records_no_modification() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-no-op", 1).await;

    let priors: &Priors = &[
        ("as-created", None),
        (
            "after-a-title",
            Some(json!({ "title": "a title written before the no-op" })),
        ),
        (
            "after-a-content",
            Some(json!({ "content": { "industry": "Education" } })),
        ),
        (
            "after-both-members",
            Some(json!({ "title": "both members at once", "content": {} })),
        ),
    ];

    for (row, create) in context_rows() {
        for (prior_name, prior) in priors {
            let case = format!("{row}/{prior_name}");
            let id = stored_context(&client, &profile_arn, create.clone(), &case).await;
            let stored = Stored::Context {
                profile_arn: profile_arn.clone(),
                id: id.clone(),
            };

            if let Some(body) = prior {
                let seeded = update_context(&client, &profile_arn, &id, body.clone()).await;
                assert_eq!(
                    seeded.status, 200,
                    "case {case:?}: the seeding update must be served, got {seeded:?}"
                );
            }
            stored.age(&client, AGE);

            let before = carried(
                &get_context(&client, &profile_arn, &id).await,
                "context",
                &case,
                "GetAgentContext",
            );
            assert_eq!(
                records_a_modification(&before),
                prior.is_some(),
                "case {case:?}: a context reports a modification exactly when one was \
                 written, got {before:?}"
            );

            let answered = carried(
                &update_context(&client, &profile_arn, &id, json!({})).await,
                "context",
                &case,
                "UpdateAgentContext",
            );
            let read_back = carried(
                &get_context(&client, &profile_arn, &id).await,
                "context",
                &case,
                "GetAgentContext",
            );
            // `UpdateAgentContext` answers the same `ContextSummary` the read does.
            assert_no_modification_recorded(&before, &answered, &read_back, &case, &[]);
        }
    }
}

/// The other half of the rule for `UpdateAgentContext`: one supplied member, at the
/// emptiest value the model allows it, records the modification — both on a context nothing
/// had modified and on one an update had already stamped.
#[tokio::test]
async fn a_supplied_context_member_records_the_modification_at_its_emptiest_value() {
    // `title` is `@length(min: 1)`, so one character is the emptiest it is served at;
    // `ContextContent` declares no required member, so a content carrying none is the
    // emptiest content there is.
    let rows: [(&str, Value); 3] = [
        ("title-at-the-minimum-length", json!({ "title": "x" })),
        ("content-with-no-member", json!({ "content": {} })),
        ("both-members", json!({ "title": "x", "content": {} })),
    ];

    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "context-writes", 1).await;
    let (seed_row, create) = context_rows()
        .into_iter()
        .next()
        .expect("the shared context table must have rows");

    for (row, body) in rows {
        let case = format!("{seed_row}/{row}");
        let id = stored_context(&client, &profile_arn, create.clone(), &case).await;
        let stored = Stored::Context {
            profile_arn: profile_arn.clone(),
            id: id.clone(),
        };
        assert_eq!(
            stored.modified_at(&client),
            None,
            "case {case:?}: nothing has modified a context that has just been created"
        );

        let first = carried(
            &update_context(&client, &profile_arn, &id, body.clone()).await,
            "context",
            &case,
            "UpdateAgentContext",
        );
        assert_modification_recorded(&first, None, stored.modified_at(&client), &case);

        // The same supplied member again is another write, so it moves the instant on.
        stored.age(&client, AGE);
        let aged = stored.modified_at(&client);
        let second = carried(
            &update_context(&client, &profile_arn, &id, body.clone()).await,
            "context",
            &case,
            "UpdateAgentContext",
        );
        assert_modification_recorded(&second, aged, stored.modified_at(&client), &case);
    }
}

/// `UpdateAgentGoal`: a request that supplies none of `pillars`, `title` and `description`
/// reports the goal as it stands and records no modification.
#[tokio::test]
async fn a_goal_update_that_supplies_no_member_records_no_modification() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-no-op", 1).await;

    let priors: &Priors = &[
        ("as-created", None),
        (
            "after-a-title",
            Some(json!({ "title": "a title written before the no-op" })),
        ),
        ("after-pillars", Some(json!({ "pillars": [PILLARS[0]] }))),
        (
            "after-a-description",
            Some(json!({ "description": "a description written before the no-op" })),
        ),
    ];

    for (row, create) in goal_rows() {
        for (prior_name, prior) in priors {
            let case = format!("{row}/{prior_name}");
            let id = stored_goal(&client, &profile_arn, create.clone(), &case).await;
            let stored = Stored::Goal {
                profile_arn: profile_arn.clone(),
                id: id.clone(),
            };

            if let Some(body) = prior {
                let seeded = update_goal(&client, &profile_arn, &id, body.clone()).await;
                assert_eq!(
                    seeded.status, 200,
                    "case {case:?}: the seeding update must be served, got {seeded:?}"
                );
            }
            stored.age(&client, AGE);

            let before = carried(
                &get_goal(&client, &profile_arn, &id).await,
                "goal",
                &case,
                "GetAgentGoal",
            );
            assert_eq!(
                records_a_modification(&before),
                prior.is_some(),
                "case {case:?}: a goal reports a modification exactly when one was written, \
                 got {before:?}"
            );

            let answered = carried(
                &update_goal(&client, &profile_arn, &id, json!({})).await,
                "goal",
                &case,
                "UpdateAgentGoal",
            );
            let read_back = carried(
                &get_goal(&client, &profile_arn, &id).await,
                "goal",
                &case,
                "GetAgentGoal",
            );
            // `UpdateAgentGoal` answers the same `GoalSummary` the read does.
            assert_no_modification_recorded(&before, &answered, &read_back, &case, &[]);
        }
    }
}

/// The other half of the rule for `UpdateAgentGoal`: one supplied member, at the emptiest
/// value the model allows it, records the modification.
#[tokio::test]
async fn a_supplied_goal_member_records_the_modification_at_its_emptiest_value() {
    // `Pillars` is `@length(min: 1)`, `title` `@length(min: 1)` and `description`
    // `@length(min: 1)`, so one entry and one character are the emptiest each is served at.
    let rows: [(&str, Value); 4] = [
        ("one-pillar", json!({ "pillars": [PILLARS[2]] })),
        ("title-at-the-minimum-length", json!({ "title": "x" })),
        (
            "description-at-the-minimum-length",
            json!({ "description": "d" }),
        ),
        (
            "every-member",
            json!({ "pillars": [PILLARS[4]], "title": "x", "description": "d" }),
        ),
    ];

    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "goal-writes", 1).await;
    let (seed_row, create) = goal_rows()
        .into_iter()
        .next()
        .expect("the shared goal table must have rows");

    for (row, body) in rows {
        let case = format!("{seed_row}/{row}");
        let id = stored_goal(&client, &profile_arn, create.clone(), &case).await;
        let stored = Stored::Goal {
            profile_arn: profile_arn.clone(),
            id: id.clone(),
        };
        assert_eq!(
            stored.modified_at(&client),
            None,
            "case {case:?}: nothing has modified a goal that has just been created"
        );

        let first = carried(
            &update_goal(&client, &profile_arn, &id, body.clone()).await,
            "goal",
            &case,
            "UpdateAgentGoal",
        );
        assert_modification_recorded(&first, None, stored.modified_at(&client), &case);

        stored.age(&client, AGE);
        let aged = stored.modified_at(&client);
        let second = carried(
            &update_goal(&client, &profile_arn, &id, body.clone()).await,
            "goal",
            &case,
            "UpdateAgentGoal",
        );
        assert_modification_recorded(&second, aged, stored.modified_at(&client), &case);
    }
}

/// An agent profile created from `pillars` and `aggregation` scopes, and its minted ARN.
async fn stored_profile(
    client: &Client,
    name: &str,
    pillars: &[&str],
    aggregation: usize,
    case: &str,
) -> String {
    let created = create_profile(client, profile_body(name, pillars, aggregation)).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected CreateAgentProfile 200, got {created:?}"
    );
    let arn = member(&created, "arn").to_string();
    assert!(
        !arn.is_empty(),
        "case {case:?}: the created profile must carry its @required arn, got {created:?}"
    );
    arn
}

/// `UpdateAgentProfile`: a request that supplies none of its writable members reports the
/// profile as it stands and records no modification.
///
/// The rows cross what the PROFILE holds — how many pillars it reviews and how many
/// account/Region scopes it analyses, including none, which is also what the derived
/// eligibility members are computed from — with what was written to it before the no-op.
#[tokio::test]
async fn an_agent_profile_update_that_supplies_no_member_records_no_modification() {
    let client = Client::new();

    let seeds: [(&str, &[&str], usize); 3] = [
        ("one-pillar-one-scope", &[PILLARS[1]], 1),
        ("every-pillar-no-scope", &PILLARS, 0),
        ("two-pillars-two-scopes", &[PILLARS[2], PILLARS[3]], 2),
    ];
    let priors: &Priors = &[
        ("as-created", None),
        (
            "after-a-display-name",
            Some(json!({ "displayName": "Written before the no-op" })),
        ),
        (
            "after-a-scope-change",
            Some(json!({ "aggregationConfiguration": [] })),
        ),
        (
            "after-deletion-protection",
            Some(json!({ "deletionProtection": false })),
        ),
    ];

    for (index, (row, pillars, aggregation)) in seeds.iter().enumerate() {
        for (prior_index, (prior_name, prior)) in priors.iter().enumerate() {
            let case = format!("{row}/{prior_name}");
            let arn = stored_profile(
                &client,
                &format!("no-op-profile-{index}-{prior_index}"),
                pillars,
                *aggregation,
                &case,
            )
            .await;
            let stored = Stored::Profile { arn: arn.clone() };

            if let Some(body) = prior {
                let seeded = update_profile(&client, &arn, body.clone()).await;
                assert_eq!(
                    seeded.status, 200,
                    "case {case:?}: the seeding update must be served, got {seeded:?}"
                );
            }
            stored.age(&client, AGE);

            let before = accepted(&get_profile(&client, &arn).await, &case, "GetAgentProfile");
            assert_eq!(
                records_a_modification(&before),
                prior.is_some(),
                "case {case:?}: a profile reports a modification exactly when one was \
                 written, got {before:?}"
            );

            let answered = accepted(
                &update_profile(&client, &arn, json!({})).await,
                &case,
                "UpdateAgentProfile",
            );
            let read_back = accepted(&get_profile(&client, &arn).await, &case, "GetAgentProfile");
            // `UpdateAgentProfileResponse` declares neither `profileType` nor `tags`,
            // which `GetAgentProfileResponse` does
            // (`smithy_query(query="operation",
            // operation="UpdateAgentProfile,GetAgentProfile")`).
            assert_no_modification_recorded(
                &before,
                &answered,
                &read_back,
                &case,
                &["profileType", "tags"],
            );
        }
    }
}

/// The other half of the rule for `UpdateAgentProfile`: one supplied member, at the
/// emptiest value the model allows it, records the modification.
#[tokio::test]
async fn a_supplied_agent_profile_member_records_the_modification_at_its_emptiest_value() {
    // `description` and `businessOverview` are `@length(max: …)` over a pattern that
    // accepts the empty string, `aggregationConfiguration` is `@length(min: 0)`,
    // `displayName` is `@length(min: 3)`, `Pillars` is `@length(min: 1)` and
    // `deletionProtection` is a boolean — so each row carries the least that member can.
    let rows: [(&str, Value); 6] = [
        ("description-empty", json!({ "description": "" })),
        ("business-overview-empty", json!({ "businessOverview": "" })),
        (
            "no-aggregation-configuration",
            json!({ "aggregationConfiguration": [] }),
        ),
        (
            "display-name-at-the-minimum-length",
            json!({ "displayName": "abc" }),
        ),
        ("one-pillar", json!({ "pillars": [PILLARS[0]] })),
        (
            "deletion-protection-cleared",
            json!({ "deletionProtection": false }),
        ),
    ];

    let client = Client::new();

    for (index, (row, body)) in rows.iter().enumerate() {
        let case = (*row).to_string();
        let arn = stored_profile(
            &client,
            &format!("profile-write-{index}"),
            &[PILLARS[1]],
            1,
            &case,
        )
        .await;
        let stored = Stored::Profile { arn: arn.clone() };
        assert_eq!(
            stored.modified_at(&client),
            None,
            "case {case:?}: nothing has modified a profile that has just been created"
        );

        let first = accepted(
            &update_profile(&client, &arn, body.clone()).await,
            &case,
            "UpdateAgentProfile",
        );
        assert_modification_recorded(&first, None, stored.modified_at(&client), &case);

        stored.age(&client, AGE);
        let aged = stored.modified_at(&client);
        let second = accepted(
            &update_profile(&client, &arn, body.clone()).await,
            &case,
            "UpdateAgentProfile",
        );
        assert_modification_recorded(&second, aged, stored.modified_at(&client), &case);
    }
}
