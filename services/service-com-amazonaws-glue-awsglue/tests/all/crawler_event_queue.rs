// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the check `CreateCrawler` makes on an S3 target's event queue.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler. The IAM records
//! the check reads are seeded into the SAME shared engine the service reads them from —
//! the store `CreateRole`, `PutRolePolicy` and `AttachRolePolicy` write.
//!
//! The rule under test: an S3 target that names an `EventQueueArn` is crawled FROM that
//! queue, and the crawl runs as the crawler's `Role`, so `CreateCrawler` refuses the
//! create with `InvalidInputException` (HTTP 400) — storing no crawler — when the
//! policies in force on that role do not admit the queue operations the crawl makes on
//! that queue, and creates the identically-shaped crawler whose role does hold them.
//!
//! A refusal that rests on those policies NAMES what they withhold: the queue action the
//! role is missing, the queue it is missing it on and the identity the crawl would run as,
//! in the sentence an unauthorized call is refused with. It asserts nothing about the
//! queue existing, which this check never establishes.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::EVENT_QUEUE_ACTIONS;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::{
    name_key, role_arn, ComAmazonawsIamAwsidentitymanagementv20100508State as IamState,
    InlinePolicy, DEFAULT_PATH,
};

use crate::wire::{call, emulator_on, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// The queue an S3 target is crawled from in these tests.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:{name}")
}

/// Seed `role_name` into the shared engine's IAM store, carrying `documents` as its
/// inline policies — the records `CreateRole` and `PutRolePolicy` write.
fn seed_role(engine: &Arc<engine::InMemoryEngine>, role_name: &str, documents: &[Value]) -> String {
    let state = engine.state::<IamState>();
    let mut store = state.lock();
    let arn = role_arn(DEFAULT_PATH, role_name);
    store.roles.insert(
        name_key(role_name),
        Role {
            path: DEFAULT_PATH.to_string(),
            role_name: role_name.to_string(),
            role_id: format!("AROA{}", role_name.to_uppercase()),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(1_700_000_000),
            assume_role_policy_document: Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": "glue.amazonaws.com" },
                        "Action": "sts:AssumeRole",
                    }],
                })
                .to_string(),
            ),
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let mut policies: BTreeMap<String, InlinePolicy> = BTreeMap::new();
    for (index, document) in documents.iter().enumerate() {
        let policy_name = format!("{role_name}-policy-{index}");
        policies.insert(
            name_key(&policy_name),
            InlinePolicy {
                policy_name,
                policy_document: document.to_string(),
            },
        );
    }
    store
        .role_inline_policies
        .insert(name_key(role_name), policies);
    arn
}

/// An identity policy document allowing `actions` on `resource`.
fn allow(actions: &[&str], resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": actions, "Resource": resource }],
    })
}

/// The sentence an unauthorized call is refused with, as the rejection of an event-mode
/// create has to word it: the identity the crawl would run as, the queue action the
/// role's policies withhold, the queue it would be made on, and which of IAM's rules
/// withheld the grant — an explicit `Deny`, or no grant at all.
///
/// Spelled out here rather than built from the service's own constructor, so the wording
/// this suite pins is the test's and not the code's.
fn expected_refusal(role_arn: &str, action: &str, queue_arn: &str, explicit_deny: bool) -> String {
    let clause = if explicit_deny {
        "with an explicit deny in an identity-based policy".to_string()
    } else {
        format!("because no identity-based policy allows the {action} action")
    };
    format!(
        "User: {role_arn} is not authorized to perform: {action} on resource: \
         {queue_arn} {clause}"
    )
}

/// A `CreateCrawler` body whose single S3 target is crawled from `queue`, in event mode.
fn event_mode_crawler(name: &str, role: &str, queue: &str) -> Value {
    json!({
        "Name": name,
        "Role": role,
        "Targets": {
            "S3Targets": [{ "Path": "s3://crawler-bucket/data/", "EventQueueArn": queue }],
        },
        "RecrawlPolicy": { "RecrawlBehavior": "CRAWL_EVENT_MODE" },
    })
}

/// The refusal one row of the table below earns: the queue action the rejection must
/// name, and whether an explicit `Deny` withheld it rather than an absent grant. `None`
/// where the row's role holds the whole grant and the create must be admitted.
type ExpectedRefusal = Option<(String, bool)>;

/// One row of the table below: what the case is, the identity policy documents the
/// crawler's role carries, and the refusal they earn.
type Row = (String, Vec<Value>, ExpectedRefusal);

/// A role whose policies do not admit the crawl's queue operations cannot back an
/// event-mode crawler, and the identically-shaped request whose role does admit them is
/// created.
///
/// The rows differ along WHAT the role's policies grant: nothing at all, the queue
/// operations on another queue, a subset of them (one row per action the crawl makes,
/// each missing in turn), a `Deny` of one of them beside an `Allow` of all, and the full
/// grant — written as the exact ARN, through a wildcard, and spread over two documents.
///
/// Each refused row also pins WHICH action the refusal names: the one the role's own
/// documents withhold for that row, so a client reading the rejection is told the grant to
/// add instead of being sent looking for a queue that is there.
#[tokio::test]
async fn an_event_mode_crawler_needs_its_role_to_consume_the_event_queue() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let queue = queue_arn("glue-crawler-events");
    let every_action: Vec<&str> = EVENT_QUEUE_ACTIONS.to_vec();
    // The queue call the crawl makes first, which is the one a role granted nothing is
    // refused at.
    let first_action = EVENT_QUEUE_ACTIONS[0];
    let last_action = EVENT_QUEUE_ACTIONS[EVENT_QUEUE_ACTIONS.len() - 1];
    // A document DENYING `action` on the queue, whatever else is granted.
    let deny = |action: &str| {
        json!({
            "Version": "2012-10-17",
            "Statement": [{ "Effect": "Deny", "Action": action, "Resource": queue }],
        })
    };

    // (case, the documents the role carries, the refusal it earns: the action the
    // rejection must name and whether an explicit `Deny` withheld it, or `None` where
    // the create must be admitted)
    let mut rows: Vec<Row> = vec![
        (
            "no policy at all".to_string(),
            Vec::new(),
            Some((first_action.to_string(), false)),
        ),
        (
            "the queue operations on another queue".to_string(),
            vec![allow(&every_action, &queue_arn("some-other-queue"))],
            Some((first_action.to_string(), false)),
        ),
        (
            "an unrelated grant on the queue".to_string(),
            vec![allow(&["sqs:SendMessage"], &queue)],
            Some((first_action.to_string(), false)),
        ),
        (
            "every queue operation, named exactly".to_string(),
            vec![allow(&every_action, &queue)],
            None,
        ),
        (
            "every queue operation, through a wildcard".to_string(),
            vec![allow(&["sqs:*"], &format!("{}*", queue_arn("")))],
            None,
        ),
        (
            "every queue operation, spread over two documents".to_string(),
            vec![
                allow(&every_action[..1], &queue),
                allow(&every_action[1..], &queue),
            ],
            None,
        ),
        (
            "an explicit deny of the first operation beside the grant".to_string(),
            vec![allow(&every_action, &queue), deny(first_action)],
            Some((first_action.to_string(), true)),
        ),
        (
            "an explicit deny of the last operation beside the grant".to_string(),
            vec![allow(&every_action, &queue), deny(last_action)],
            Some((last_action.to_string(), true)),
        ),
    ];
    // One row per action the crawl makes, each one missing from an otherwise complete
    // grant: every action is required, so each omission refuses on its own — and the
    // refusal names the action that omission withheld.
    for missing in EVENT_QUEUE_ACTIONS {
        let granted: Vec<&str> = every_action
            .iter()
            .copied()
            .filter(|action| *action != missing)
            .collect();
        rows.push((
            format!("every queue operation except {missing}"),
            vec![allow(&granted, &queue)],
            Some((missing.to_string(), false)),
        ));
    }

    for (index, (case, documents, refusal)) in rows.iter().enumerate() {
        let role_name = format!("crawler-role-{index}");
        let role = seed_role(&engine, &role_name, documents);
        let crawler = format!("event-crawler-{index}");
        let request = event_mode_crawler(&crawler, &role, &queue);

        let (status, body) = call(&router, "CreateCrawler", &request).await;
        let Some((action, explicit_deny)) = refusal else {
            assert_eq!(
                status, 200,
                "case {case:?}: the role holds the grant, so the create must be admitted, \
                 got {status} with {body}"
            );
            let read = ok(&router, "GetCrawler", &json!({ "Name": crawler })).await;
            assert_eq!(
                read["Crawler"]["Name"].as_str(),
                Some(crawler.as_str()),
                "case {case:?}: the created crawler must be readable, got {read}"
            );
            continue;
        };

        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {}",
            error_code(&body)
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            expected_refusal(&role, action, &queue, *explicit_deny),
            "case {case:?}: the refusal must name the withheld action {action} on {queue} \
             for {role}, got {message:?}"
        );
        // No crawler was stored: the refused create left nothing behind.
        let (status, body) = call(&router, "GetCrawler", &json!({ "Name": crawler })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create must store no crawler, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {}",
            error_code(&body)
        );
    }
}

/// The refusal is worded from the REQUEST: whichever way the request names the role —
/// its bare name or its ARN — the identity the rejection names is that role's ARN, the
/// resource it names is the queue the target carried, and nothing in it claims the queue
/// is absent.
#[tokio::test]
async fn the_refusal_is_worded_from_the_request() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let first_action = EVENT_QUEUE_ACTIONS[0];
    // A role granted nothing at all, so every row below is refused at the crawl's first
    // queue call.
    let role_arn = seed_role(&engine, "worded-role", &[]);

    // (case, the `Role` the request carries, the queue the target names)
    let rows = [
        (
            "the role's ARN and a plainly named queue",
            role_arn.clone(),
            queue_arn("worded-events"),
        ),
        (
            "the role's bare NAME and another queue",
            "worded-role".to_string(),
            queue_arn("worded-events-2.fifo"),
        ),
    ];

    for (index, (case, role, queue)) in rows.iter().enumerate() {
        let crawler = format!("worded-crawler-{index}");
        let (status, body) = call(
            &router,
            "CreateCrawler",
            &event_mode_crawler(&crawler, role, queue),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            expected_refusal(&role_arn, first_action, queue, false),
            "case {case:?}: the refusal must name the role's ARN, the withheld action and \
             the queue the request carried, got {message:?}"
        );
        // The check reads policies, not the queue's existence, so it asserts none.
        assert!(
            !message.contains("does not exist"),
            "case {case:?}: the refusal must not claim the queue is absent, got {message:?}"
        );
    }
}

/// The check is about the queue the TARGET names, so it applies to each S3 target of a
/// create and to the dead-letter queue beside the event queue — and a target naming no
/// queue at all is crawled without one, so nothing is checked for it.
#[tokio::test]
async fn the_check_covers_every_queue_a_target_names() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let events = queue_arn("target-events");
    let dead_letter = queue_arn("target-dead-letter");
    let actions: Vec<&str> = EVENT_QUEUE_ACTIONS.to_vec();

    // A role granted the queue operations on the EVENT queue only.
    let role = seed_role(&engine, "partial-role", &[allow(&actions, &events)]);

    // A target naming no queue is crawled without one: nothing is checked, so the
    // create is admitted even though the role grants nothing for any queue.
    let plain = seed_role(&engine, "no-queue-role", &[]);
    ok(
        &router,
        "CreateCrawler",
        &json!({
            "Name": "plain-crawler",
            "Role": plain,
            "Targets": { "S3Targets": [{ "Path": "s3://crawler-bucket/plain/" }] },
        }),
    )
    .await;

    // The dead-letter queue of the same target is a queue the crawl consumes too, so a
    // role with no grant on it is refused.
    let (status, body) = call(
        &router,
        "CreateCrawler",
        &json!({
            "Name": "dlq-crawler",
            "Role": role,
            "Targets": {
                "S3Targets": [{
                    "Path": "s3://crawler-bucket/data/",
                    "EventQueueArn": events,
                    "DlqEventQueueArn": dead_letter,
                }],
            },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a dead-letter queue the role cannot consume\": expected 400, got {status} \
         with {body}"
    );
    assert!(
        error_message(&body).contains(&dead_letter),
        "case \"a dead-letter queue the role cannot consume\": the refusal must name that \
         queue, got {:?}",
        error_message(&body)
    );

    // A SECOND S3 target's queue is checked as well, not only the first.
    let (status, body) = call(
        &router,
        "CreateCrawler",
        &json!({
            "Name": "second-target-crawler",
            "Role": role,
            "Targets": {
                "S3Targets": [
                    { "Path": "s3://crawler-bucket/first/", "EventQueueArn": events },
                    { "Path": "s3://crawler-bucket/second/", "EventQueueArn": dead_letter },
                ],
            },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a second target's queue\": expected 400, got {status} with {body}"
    );

    // And the crawler whose every named queue the role can consume is created.
    let both = seed_role(
        &engine,
        "both-queues-role",
        &[allow(&actions, &events), allow(&actions, &dead_letter)],
    );
    ok(
        &router,
        "CreateCrawler",
        &json!({
            "Name": "both-queues-crawler",
            "Role": both,
            "Targets": {
                "S3Targets": [{
                    "Path": "s3://crawler-bucket/data/",
                    "EventQueueArn": events,
                    "DlqEventQueueArn": dead_letter,
                }],
            },
        }),
    )
    .await;
    let read = ok(
        &router,
        "GetCrawler",
        &json!({ "Name": "both-queues-crawler" }),
    )
    .await;
    assert_eq!(
        read["Crawler"]["Targets"]["S3Targets"][0]["EventQueueArn"].as_str(),
        Some(events.as_str()),
        "the created crawler must report the queue it was created with, got {read}"
    );
}

/// The number of queues one create may have decided about is bounded
/// (`MAX_CHECKED_EVENT_QUEUES`): a create naming that many DISTINCT queues is served, one
/// naming more is refused with the operation's modelled `InvalidInputException`, and the
/// same queue named by several targets counts once.
#[tokio::test]
async fn the_number_of_checked_event_queues_is_bounded() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    // A role granted the crawl's queue operations on every queue of this account, so a
    // refusal below can only be the ceiling and never a missing grant.
    let role = seed_role(
        &engine,
        "every-queue-role",
        &[allow(&["sqs:*"], &format!("{}*", queue_arn("")))],
    );

    let ceiling = service_com_amazonaws_glue_awsglue::state::MAX_CHECKED_EVENT_QUEUES;
    let targets = |count: usize| {
        let s3_targets: Vec<Value> = (0..count)
            .map(|index| {
                json!({
                    "Path": format!("s3://crawler-bucket/{index}/"),
                    "EventQueueArn": queue_arn(&format!("bounded-{index}")),
                })
            })
            .collect();
        json!({ "S3Targets": s3_targets })
    };

    // At the ceiling: served.
    ok(
        &router,
        "CreateCrawler",
        &json!({
            "Name": "at-the-ceiling",
            "Role": role,
            "Targets": targets(ceiling),
        }),
    )
    .await;

    // One past it: refused, and nothing stored.
    let (status, body) = call(
        &router,
        "CreateCrawler",
        &json!({
            "Name": "past-the-ceiling",
            "Role": role,
            "Targets": targets(ceiling + 1),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one queue past the ceiling\": expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case \"one queue past the ceiling\": expected InvalidInputException, got {}",
        error_code(&body)
    );
    let (status, body) = call(
        &router,
        "CreateCrawler",
        &json!({ "Name": "past-the-ceiling" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one queue past the ceiling\": a refused create stores nothing, got \
         {status} with {body}"
    );

    // The SAME queue named by more targets than the ceiling is one queue to decide
    // about, so it is served.
    let repeated: Vec<Value> = (0..ceiling + 10)
        .map(|index| {
            json!({
                "Path": format!("s3://crawler-bucket/repeat-{index}/"),
                "EventQueueArn": queue_arn("bounded-0"),
            })
        })
        .collect();
    ok(
        &router,
        "CreateCrawler",
        &json!({
            "Name": "one-queue-many-targets",
            "Role": role,
            "Targets": { "S3Targets": repeated },
        }),
    )
    .await;
}

/// A `Role` that names no role this account holds grounds no refusal: the check refuses
/// only what the account's own IAM records SHOW, so such a create goes ahead.
#[tokio::test]
async fn a_role_this_account_does_not_hold_is_not_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let queue = queue_arn("unknown-role-events");

    for (case, role) in [
        (
            "a role ARN of another account",
            "arn:aws:iam::210987654321:role/foreign-crawler".to_string(),
        ),
        (
            "a role name this account holds no role under",
            "never-created".to_string(),
        ),
    ] {
        let name = format!("unchecked-{}", case.replace(' ', "-"));
        let (status, body) = call(
            &router,
            "CreateCrawler",
            &event_mode_crawler(&name, &role, &queue),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a role nothing can be shown about grounds no refusal, got \
             {status} with {body}"
        );
    }
}
