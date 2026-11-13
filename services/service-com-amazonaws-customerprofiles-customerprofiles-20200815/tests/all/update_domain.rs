// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateDomain`.
//!
//! The rule under test: an update of a domain writes every member the request carries to
//! the stored domain, leaves every member it omits exactly as it was, clears the two
//! members documented to be cleared by an empty string, moves `LastUpdatedAt` to the
//! instant of the call while `CreatedAt` stays where the create put it, and answers from
//! that record — so a later `GetDomain` reports the same thing. A name the store does not
//! hold is `ResourceNotFoundException` (404) whose `Message` member is reported as JSON
//! `null` — this operation's refusal words no sentence — and creates nothing.
//!
//! Every test drives the service through the real HTTP surface — the same
//! `MultiServiceRouter` the emulator binary mounts — and asserts the rule over a SET of
//! inputs, differing along which members are present, their form (scalar, string, map,
//! nested structure) and the bounds of the ranges the model declares.

use serde_json::{json, Value};

use crate::domain_profile_lifecycle::{error_code, Emulator};

/// The configuration every row starts from, so what an update leaves alone is visible.
/// `DefaultEncryptionKey` and `DeadLetterQueueUrl` are set here because an update's
/// empty string has to have something to clear.
fn baseline() -> Value {
    json!({
        "DefaultExpirationDays": 90,
        "DefaultEncryptionKey": "arn:aws:kms:us-east-1:111122223333:key/baseline",
        "DeadLetterQueueUrl": "https://sqs.us-east-1.amazonaws.com/111122223333/baseline",
        "Matching": { "Enabled": false },
        "Tags": { "env": "dev" },
    })
}

/// The members of the baseline configuration, as they read back when an update does not
/// touch them.
const BASELINE_KEY: &str = "arn:aws:kms:us-east-1:111122223333:key/baseline";
const BASELINE_QUEUE: &str = "https://sqs.us-east-1.amazonaws.com/111122223333/baseline";

/// Assert every `(json pointer, expected)` expectation against one response body.
fn assert_members(case: &str, label: &str, body: &Value, expected: &[Expectation]) {
    for (pointer, want) in expected {
        let actual = body.pointer(pointer).cloned();
        assert_eq!(
            actual.as_ref(),
            want.as_ref(),
            "case {case:?} ({label}): expected {pointer} to be {want:?}, got {actual:?} in {body:?}"
        );
    }
}

/// One member of a response, named by its JSON pointer, and what it must read as —
/// `None` when the member must be absent altogether.
type Expectation = (&'static str, Option<Value>);

/// One row of the table below: the case's name, the update body it sends, and what the
/// domain must read as afterwards.
type UpdateCase = (&'static str, Value, Vec<Expectation>);

/// An update writes the members it carries and leaves the rest of the domain alone, and
/// the record it wrote is what a later `GetDomain` answers from. The rows differ along
/// WHICH members the update carries, their form (integer at each published bound of
/// `DefaultExpirationDays`, string, empty string, map, nested structure) and the
/// no-member case.
#[tokio::test]
async fn update_domain_writes_only_the_members_the_request_carries() {
    let emulator = Emulator::new();

    // (case, update body, expectations as (json pointer, expected value))
    let cases: Vec<UpdateCase> = vec![
        (
            // The published minimum of `DefaultExpirationDays` (range 1..1098).
            "expiration-at-minimum",
            json!({ "DefaultExpirationDays": 1 }),
            vec![
                ("/DefaultExpirationDays", Some(json!(1))),
                ("/DefaultEncryptionKey", Some(json!(BASELINE_KEY))),
                ("/DeadLetterQueueUrl", Some(json!(BASELINE_QUEUE))),
                ("/Tags/env", Some(json!("dev"))),
                ("/Matching/Enabled", Some(json!(false))),
            ],
        ),
        (
            // The published maximum of the same range.
            "expiration-at-maximum",
            json!({ "DefaultExpirationDays": 1098 }),
            vec![
                ("/DefaultExpirationDays", Some(json!(1098))),
                ("/DefaultEncryptionKey", Some(json!(BASELINE_KEY))),
                ("/DeadLetterQueueUrl", Some(json!(BASELINE_QUEUE))),
            ],
        ),
        (
            "clear-the-encryption-key",
            json!({ "DefaultEncryptionKey": "" }),
            vec![
                ("/DefaultEncryptionKey", None),
                ("/DeadLetterQueueUrl", Some(json!(BASELINE_QUEUE))),
                ("/DefaultExpirationDays", Some(json!(90))),
            ],
        ),
        (
            "clear-the-dead-letter-queue",
            json!({ "DeadLetterQueueUrl": "" }),
            vec![
                ("/DeadLetterQueueUrl", None),
                ("/DefaultEncryptionKey", Some(json!(BASELINE_KEY))),
            ],
        ),
        (
            "replace-both-strings",
            json!({
                "DefaultEncryptionKey": "arn:aws:kms:us-east-1:111122223333:key/updated",
                "DeadLetterQueueUrl": "https://sqs.us-east-1.amazonaws.com/111122223333/updated",
            }),
            vec![
                (
                    "/DefaultEncryptionKey",
                    Some(json!("arn:aws:kms:us-east-1:111122223333:key/updated")),
                ),
                (
                    "/DeadLetterQueueUrl",
                    Some(json!("https://sqs.us-east-1.amazonaws.com/111122223333/updated")),
                ),
                ("/DefaultExpirationDays", Some(json!(90))),
            ],
        ),
        (
            "no-member-at-all",
            json!({}),
            vec![
                ("/DefaultExpirationDays", Some(json!(90))),
                ("/DefaultEncryptionKey", Some(json!(BASELINE_KEY))),
                ("/DeadLetterQueueUrl", Some(json!(BASELINE_QUEUE))),
                ("/Tags/env", Some(json!("dev"))),
                ("/Matching/Enabled", Some(json!(false))),
            ],
        ),
        (
            "nested-matching-configuration",
            json!({
                "Matching": {
                    "Enabled": true,
                    "JobSchedule": { "DayOfTheWeek": "MONDAY", "Time": "17:00" },
                },
            }),
            vec![
                ("/Matching/Enabled", Some(json!(true))),
                ("/Matching/JobSchedule/DayOfTheWeek", Some(json!("MONDAY"))),
                ("/Matching/JobSchedule/Time", Some(json!("17:00"))),
                ("/DefaultExpirationDays", Some(json!(90))),
                ("/Tags/env", Some(json!("dev"))),
            ],
        ),
        (
            "replace-the-tag-map",
            json!({ "Tags": { "env": "prod", "team": "cx" } }),
            vec![
                ("/Tags/env", Some(json!("prod"))),
                ("/Tags/team", Some(json!("cx"))),
                ("/DefaultExpirationDays", Some(json!(90))),
                ("/DefaultEncryptionKey", Some(json!(BASELINE_KEY))),
            ],
        ),
    ];

    for (index, (case, update, expected)) in cases.iter().enumerate() {
        let domain = format!("upd{index}");
        let created = emulator
            .send("POST", &format!("/domains/{domain}"), Some(baseline()))
            .await;
        assert_eq!(
            created.0, 200,
            "case {case:?}: expected CreateDomain 200, got {} {:?}",
            created.0, created.1
        );

        let (status, updated) = emulator
            .send("PUT", &format!("/domains/{domain}"), Some(update.clone()))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateDomain 200, got {status} {updated:?}"
        );
        assert_eq!(
            updated.get("DomainName").and_then(Value::as_str),
            Some(domain.as_str()),
            "case {case:?}: expected the updated domain to be named {domain:?}, got {updated:?}"
        );
        assert_members(case, "UpdateDomain response", &updated, expected);

        // The update answered from the record it wrote, so a read of that record must
        // report exactly the same configuration.
        let (status, fetched) = emulator.send("GET", &format!("/domains/{domain}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetDomain 200, got {status} {fetched:?}"
        );
        assert_members(case, "GetDomain after the update", &fetched, expected);
        assert_eq!(
            fetched.get("CreatedAt"),
            created.1.get("CreatedAt"),
            "case {case:?}: an update must not move CreatedAt - expected {:?}, got {:?}",
            created.1.get("CreatedAt"),
            fetched.get("CreatedAt")
        );
    }
}

/// An update moves `LastUpdatedAt` to the instant of the call — "the timestamp of when
/// the domain was most recently edited" — and leaves `CreatedAt` where the create put it.
/// Asserted over a sequence of updates, so the invariant is that the edit timestamp only
/// ever advances.
#[tokio::test]
async fn update_domain_advances_last_updated_at_and_never_created_at() {
    let emulator = Emulator::new();
    let (status, created) = emulator
        .send("POST", "/domains/stamps", Some(baseline()))
        .await;
    assert_eq!(status, 200, "CreateDomain: expected 200, got {status} {created:?}");

    let created_at = created
        .get("CreatedAt")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("CreateDomain: no CreatedAt in {created:?}"));
    let mut previous = created
        .get("LastUpdatedAt")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("CreateDomain: no LastUpdatedAt in {created:?}"));

    for days in [1_i64, 30, 1098] {
        // The timestamp comes from the clock, so the calls are separated enough for the
        // advance to be observable at the sub-second precision the wire carries.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let case = format!("DefaultExpirationDays={days}");
        let (status, updated) = emulator
            .send(
                "PUT",
                "/domains/stamps",
                Some(json!({ "DefaultExpirationDays": days })),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateDomain 200, got {status} {updated:?}"
        );
        assert_eq!(
            updated.get("CreatedAt").and_then(Value::as_f64),
            Some(created_at),
            "case {case:?}: expected CreatedAt to stay {created_at}, got {:?}",
            updated.get("CreatedAt")
        );
        let last_updated_at = updated
            .get("LastUpdatedAt")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: no LastUpdatedAt in {updated:?}"));
        assert!(
            last_updated_at > previous,
            "case {case:?}: expected LastUpdatedAt to advance past {previous}, got {last_updated_at}"
        );
        previous = last_updated_at;
    }
}

/// Turning on rule-based matching or the data store with an update is asynchronous work,
/// exactly as it is on a create: the call reports the transient state
/// (`RuleBasedMatching$Status` `IN_PROGRESS` — "the system is creating the rule-based
/// matching rule" — and a data-store readiness below 100%), and the terminal state is
/// reached once the span the clock owes is up. Each piece of work has its own span, so
/// configuring one does not move the other. Asserted on a domain that has been ready for
/// a while, so the transient state can only come from the call under test.
#[tokio::test]
async fn an_update_that_turns_on_matching_restarts_the_asynchronous_work() {
    let emulator = Emulator::new();
    emulator.create_domain("restart", 90).await;

    // Past the span the create owed, so the domain is settled before the update.
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;

    let (status, updated) = emulator
        .send(
            "PUT",
            "/domains/restart",
            Some(json!({
                "RuleBasedMatching": {
                    "Enabled": true,
                    "MatchingRules": [{ "Rule": ["EmailAddress"] }],
                    "MaxAllowedRuleLevelForMatching": 1,
                },
                "DataStore": { "Enabled": true },
            })),
        )
        .await;
    assert_eq!(
        status, 200,
        "UpdateDomain: expected 200, got {status} {updated:?}"
    );
    assert_eq!(
        updated.pointer("/RuleBasedMatching/Status").and_then(Value::as_str),
        Some("IN_PROGRESS"),
        "case \"at-update\": expected the rule to be building, got {:?}",
        updated.get("RuleBasedMatching")
    );
    assert_eq!(
        updated.pointer("/RuleBasedMatching/MatchingRules"),
        Some(&json!([{ "Rule": ["EmailAddress"] }])),
        "case \"at-update\": expected the submitted matching rules, got {:?}",
        updated.get("RuleBasedMatching")
    );
    assert!(
        updated
            .pointer("/DataStore/Readiness/ProgressPercentage")
            .and_then(Value::as_i64)
            .is_some_and(|percentage| (0..100).contains(&percentage)),
        "case \"at-update\": expected data-store readiness below 100%, got {:?}",
        updated.get("DataStore")
    );

    // The transition is owed by the clock, so waiting past the span is all it takes.
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;

    let (_, settled) = emulator.send("GET", "/domains/restart", None).await;
    assert_eq!(
        settled.pointer("/RuleBasedMatching/Status").and_then(Value::as_str),
        Some("ACTIVE"),
        "case \"after-the-span\": expected the rule to be ready, got {:?}",
        settled.get("RuleBasedMatching")
    );
    assert_eq!(
        settled
            .pointer("/DataStore/Readiness/ProgressPercentage")
            .and_then(Value::as_i64),
        Some(100),
        "case \"after-the-span\": expected the data store to be ready, got {:?}",
        settled.get("DataStore")
    );

    // Each piece of work has its own span: an update that only reconfigures the data
    // store must not drag the settled matching rule back to a transient status, and the
    // store it did reconfigure starts over.
    let (status, only_store) = emulator
        .send(
            "PUT",
            "/domains/restart",
            Some(json!({ "DataStore": { "Enabled": true } })),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"data-store-only\": expected UpdateDomain 200, got {status} {only_store:?}"
    );
    assert_eq!(
        only_store.pointer("/RuleBasedMatching/Status").and_then(Value::as_str),
        Some("ACTIVE"),
        "case \"data-store-only\": a settled rule must stay settled, got {:?}",
        only_store.get("RuleBasedMatching")
    );
    assert!(
        only_store
            .pointer("/DataStore/Readiness/ProgressPercentage")
            .and_then(Value::as_i64)
            .is_some_and(|percentage| (0..100).contains(&percentage)),
        "case \"data-store-only\": expected the store's readiness to start over, got {:?}",
        only_store.get("DataStore")
    );
}

/// An update of a domain the store does not hold is `ResourceNotFoundException` (404) and
/// creates nothing: the operation edits an existing domain, and a domain's name cannot be
/// changed after it is created. The rows are names no create was ever called for,
/// including one that was created and then deleted.
#[tokio::test]
async fn updating_an_absent_domain_is_resource_not_found() {
    let emulator = Emulator::new();
    emulator.create_domain("alive", 90).await;

    let (status, deleted) = emulator.send("DELETE", "/domains/gone", None).await;
    assert_eq!(
        status, 404,
        "DeleteDomain of a name nothing created: expected 404, got {status} {deleted:?}"
    );

    let (status, body) = emulator
        .send("POST", "/domains/gone", Some(baseline()))
        .await;
    assert_eq!(status, 200, "CreateDomain gone: expected 200, got {status} {body:?}");
    let (status, body) = emulator.send("DELETE", "/domains/gone", None).await;
    assert_eq!(status, 200, "DeleteDomain gone: expected 200, got {status} {body:?}");

    for case in ["never-created", "gone", "ALIVE"] {
        let (status, body) = emulator
            .send(
                "PUT",
                &format!("/domains/{case}"),
                Some(json!({ "DefaultExpirationDays": 7 })),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected UpdateDomain 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        // This operation's refusal words no sentence: the `Message` member is reported as
        // JSON null. The envelope is asserted over a table of absent names in
        // `tests/all/update_domain_not_found_envelope.rs`.
        assert_eq!(
            body.get("Message"),
            Some(&Value::Null),
            "case {case:?}: expected the update's refusal to report a null Message, got {body:?}"
        );
        let (status, read) = emulator.send("GET", &format!("/domains/{case}"), None).await;
        assert_eq!(
            (status, read.get("Message").and_then(Value::as_str)),
            (404, Some(format!("Domain {case} is not found").as_str())),
            "case {case:?}: expected a read of the same absent domain to carry the domain \
             sentence, got {status} {read:?}"
        );
        // The read above is also proof the refused update did not conjure the domain: it
        // still answers 404.
    }

    // The domain that does exist is untouched by any of the refusals above.
    let (status, alive) = emulator.send("GET", "/domains/alive", None).await;
    assert_eq!(
        (status, alive.get("DefaultExpirationDays").and_then(Value::as_i64)),
        (200, Some(90)),
        "the existing domain must be unchanged, got {status} {alive:?}"
    );
}

/// A value outside a modelled constraint is refused as `BadRequestException` (400), and
/// the stored domain keeps the value it had. The range `DefaultExpirationDays` declares
/// (1..1098) is exercised on both published bounds and one step past each of them.
#[tokio::test]
async fn update_domain_refuses_a_value_outside_the_modelled_range() {
    let emulator = Emulator::new();
    emulator.create_domain("bounds", 90).await;

    // (case, DefaultExpirationDays, expected status)
    let cases: [(&str, i64, u16); 4] = [
        ("one-below-the-minimum", 0, 400),
        ("the-minimum", 1, 200),
        ("the-maximum", 1098, 200),
        ("one-above-the-maximum", 1099, 400),
    ];

    for (case, days, expected) in cases {
        let (status, body) = emulator
            .send(
                "PUT",
                "/domains/bounds",
                Some(json!({ "DefaultExpirationDays": days })),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {body:?}"
        );
        if expected != 200 {
            // `BadRequestException` is the only client-input error this operation models
            // for a value outside a constraint.
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body:?}"
            );
        }

        let (_, fetched) = emulator.send("GET", "/domains/bounds", None).await;
        let stored = fetched.get("DefaultExpirationDays").and_then(Value::as_i64);
        let want = if expected == 200 { Some(days) } else { None };
        if let Some(want) = want {
            assert_eq!(
                stored,
                Some(want),
                "case {case:?}: expected the accepted value to be stored, got {fetched:?}"
            );
        } else {
            assert_ne!(
                stored,
                Some(days),
                "case {case:?}: a refused value must not be stored, got {fetched:?}"
            );
        }
    }
}
