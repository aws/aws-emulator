// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the tag keys EC2 Capacity Manager monitors for the account —
//! `UpdateCapacityManagerMonitoredTagKeys`, which activates and deactivates them, and the
//! `GetCapacityManagerMonitoredTagKeys` that reports them — driven over the wire the service
//! speaks (`ec2Query`, through [`super::ec2_query`]).
//!
//! The rules under test are:
//!
//! 1. REPORTING: the report answers the keys the account monitors and writes nothing — a
//!    fresh account monitors none, and one read never changes what a later read sees;
//! 2. ROUND TRIP: an activation records exactly the keys the request named, answering them
//!    read back out of the store, and a later report agrees — in tag-key order, each key
//!    once, whatever order or duplication the request carried;
//! 3. CLOCK: activation is a move the clock owes, so a key is `activating` to a caller acting
//!    straight after the update and `activated` once the span has passed whether or not
//!    anybody asked in between; a repeat activation does not send a monitored key back to
//!    `activating`;
//! 4. DEACTIVATION: a deactivated key is `deactivating` until its own span passes and is
//!    monitored no more after it, the other keys untouched; a key the account does not
//!    monitor is not affected at all;
//! 5. PAGINATION: the report pages over the keys in tag-key order, every key once, with a
//!    token only while another key follows — and a token this service did not mint is
//!    refused;
//! 6. a `DryRun` request performs nothing and is answered 412 `DryRunOperation`;
//! 7. REFUSALS: a request naming neither list, a tag key outside what EC2 accepts, one key
//!    named for both activation and deactivation, a `ClientToken` past its bound and a set
//!    past this emulator's ceiling are each refused, and a refused request stores nothing;
//! 8. ACCOUNT SCOPE: the monitored keys are a fact of the ACCOUNT, so keys activated in one
//!    Region are reported in every other.
//!
//! Each table row is a distinct input along what its rule generalises over — how many keys
//! the request names, what order and case they are in, how long a key is, which list carries
//! them, which page size the walk uses and which Region the call was signed for — so no row
//! is the specification.

use std::time::{Duration, Instant};

use ec2::state::{
    MAX_CLIENT_TOKEN_CHARS, MAX_MONITORED_TAG_KEYS, MAX_MONITORED_TAG_KEYS_PER_REQUEST,
    MAX_TAG_KEY_CHARS, MONITORED_TAG_KEY_STATUS_ACTIVATED, MONITORED_TAG_KEY_STATUS_ACTIVATING,
    MONITORED_TAG_KEY_STATUS_DEACTIVATING,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message, indexed};
use super::wire::{Param, child_text, document, param, set_items};

/// Regions spanning the partitions and the naming shapes the Region field takes, so no
/// assertion below is a property of one Region string.
const REGIONS: [&str; 4] = ["us-west-2", "eu-central-1", "ap-southeast-1", "us-gov-west-1"];

/// The Region most of the cases below are signed for; the Region the setting belongs to is
/// rule 8's own table.
const REGION: &str = REGIONS[0];

/// How long a poll below is given before it reports that a key never left its transient
/// status — generously past [`ec2::state::TRANSITION_DELAY`], so a slow machine fails
/// nothing, while a status the loop's guard never matches still ends the test instead of
/// hanging.
const POLL_DEADLINE: Duration = Duration::from_secs(10);

/// How long a poll waits between reads, as a client's own waiter does.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One monitored tag key as a response reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Monitored {
    tag_key: String,
    status: String,
    capacity_manager_provided: Option<String>,
    status_message: Option<String>,
    earliest_datapoint_timestamp: Option<String>,
}

/// The `capacityManagerTagKeySet` of a response, in the order it reported it.
fn monitored(case: &str, body: &str) -> Vec<Monitored> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "capacityManagerTagKeySet")
        .into_iter()
        .map(|item| Monitored {
            tag_key: child_text(item, "tagKey").unwrap_or_default(),
            status: child_text(item, "status").unwrap_or_default(),
            capacity_manager_provided: child_text(item, "capacityManagerProvided"),
            status_message: child_text(item, "statusMessage"),
            earliest_datapoint_timestamp: child_text(item, "earliestDatapointTimestamp"),
        })
        .collect()
}

/// Just the tag keys a response reported, in the order it reported them.
fn keys_of(entries: &[Monitored]) -> Vec<String> {
    entries.iter().map(|entry| entry.tag_key.clone()).collect()
}

/// The status a response reported for `tag_key`, or `None` when it reported no such key.
fn status_of(entries: &[Monitored], tag_key: &str) -> Option<String> {
    entries
        .iter()
        .find(|entry| entry.tag_key == tag_key)
        .map(|entry| entry.status.clone())
}

/// `ActivateTagKey.N` / `DeactivateTagKey.N` form fields, as ec2Query flattens the two
/// lists.
fn activate(keys: &[&str]) -> Vec<Param> {
    indexed("ActivateTagKey", keys)
}

fn deactivate(keys: &[&str]) -> Vec<Param> {
    indexed("DeactivateTagKey", keys)
}

/// One `UpdateCapacityManagerMonitoredTagKeys` call, whatever it answers.
async fn update(router: &MultiServiceRouter, region: &str, params: &[Param]) -> (u16, String) {
    call(
        router,
        region,
        "UpdateCapacityManagerMonitoredTagKeys",
        params,
    )
    .await
}

/// One `UpdateCapacityManagerMonitoredTagKeys` call that has to succeed, and the keys it
/// reported as affected.
async fn update_ok(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    params: &[Param],
) -> Vec<Monitored> {
    let (status, body) = update(router, region, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateCapacityManagerMonitoredTagKeys, \
         got {status} with body {body}"
    );
    monitored(case, &body)
}

/// One `GetCapacityManagerMonitoredTagKeys` call, whatever it answers.
async fn get(router: &MultiServiceRouter, region: &str, params: &[Param]) -> (u16, String) {
    call(router, region, "GetCapacityManagerMonitoredTagKeys", params).await
}

/// The keys `GetCapacityManagerMonitoredTagKeys` reports in `region`, which has to succeed.
async fn report(case: &str, router: &MultiServiceRouter, region: &str) -> Vec<Monitored> {
    let (status, body) = get(router, region, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetCapacityManagerMonitoredTagKeys in \
         {region}, got {status} with body {body}"
    );
    monitored(case, &body)
}

/// Poll the report until `tag_key` leaves `transient`, exactly as a client's own waiter
/// does, and answer the status it then held — `None` once the key is monitored no more.
async fn poll_until_past(
    case: &str,
    router: &MultiServiceRouter,
    tag_key: &str,
    transient: &str,
) -> Option<String> {
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut status = status_of(&report(case, router, REGION).await, tag_key);
    while status.as_deref() == Some(transient) && Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        status = status_of(&report(case, router, REGION).await, tag_key);
    }
    status
}

// ---------------------------------------------------------------------------
// Rule 1: the report writes nothing
// ---------------------------------------------------------------------------

/// RULE 1: a fresh account monitors no tag key in any Region, and asking twice reports the
/// same thing — the read is reporting-only.
#[tokio::test]
async fn a_fresh_account_monitors_no_tag_key_in_any_region() {
    let router = emulator();
    for region in REGIONS {
        let case = format!("a fresh account in {region}");
        let first = report(&case, &router, region).await;
        assert!(
            first.is_empty(),
            "case {case:?}: expected no monitored tag key, got {first:?}"
        );
        let second = report(&case, &router, region).await;
        assert_eq!(
            second, first,
            "case {case:?}: expected a read to leave the monitored keys at {first:?}, \
             got {second:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the activation round-trips
// ---------------------------------------------------------------------------

/// RULE 2: whatever keys an activation names — however many, in whatever order, with
/// whatever duplication and case — the update answers exactly the distinct keys it recorded
/// in tag-key order, each `activating`, and a later report agrees.
#[tokio::test]
async fn an_activation_records_the_keys_the_request_named_and_reads_back() {
    // A tag key at the published bound ("Tag keys … accept a maximum of 127 Unicode
    // characters" — `Tag$Key`, [`MAX_TAG_KEY_CHARS`]), which is a key the issue did not
    // cite.
    let at_the_bound = "k".repeat(MAX_TAG_KEY_CHARS);
    // `(case, the keys the request names, the distinct keys it must record)`.
    let rows: [(&str, Vec<&str>, Vec<&str>); 6] = [
        ("one key", vec!["Project"], vec!["Project"]),
        (
            "several keys, named out of order",
            vec!["Zeta", "Alpha", "middle"],
            vec!["Alpha", "Zeta", "middle"],
        ),
        (
            "the same key twice in one request",
            vec!["Duplicated", "Duplicated"],
            vec!["Duplicated"],
        ),
        (
            "two keys differing only in case, which tag keys are sensitive to",
            vec!["project", "Project"],
            vec!["Project", "project"],
        ),
        (
            "a key carrying the punctuation a tag key admits",
            vec!["cost-centre.01_x/y:z"],
            vec!["cost-centre.01_x/y:z"],
        ),
        (
            "a key at the published length bound",
            vec![at_the_bound.as_str()],
            vec![at_the_bound.as_str()],
        ),
    ];

    for (case, named, expected) in rows {
        let router = emulator();
        let answered = update_ok(case, &router, REGION, &activate(&named)).await;
        assert_eq!(
            keys_of(&answered),
            expected,
            "case {case:?}: expected the update to answer {expected:?}, got {answered:?}"
        );
        for entry in &answered {
            assert_eq!(
                entry.status, MONITORED_TAG_KEY_STATUS_ACTIVATING,
                "case {case:?}: expected {:?} to be {MONITORED_TAG_KEY_STATUS_ACTIVATING}, \
                 got {entry:?}",
                entry.tag_key
            );
            // Every key in the set is here because the account activated it, so none of
            // them is one Capacity Manager provided.
            assert_eq!(
                entry.capacity_manager_provided.as_deref(),
                Some("false"),
                "case {case:?}: expected {:?} to be reported as not Capacity Manager \
                 provided, got {entry:?}",
                entry.tag_key
            );
        }

        // ROUND TRIP: the reader sees what the update left behind, and nothing else.
        let reported = report(case, &router, REGION).await;
        assert_eq!(
            keys_of(&reported),
            expected,
            "case {case:?}: expected the report to answer {expected:?}, got {reported:?}"
        );
    }
}

/// RULE 2 over SEQUENCES of updates: the account monitors exactly what its updates left it
/// monitoring, and each update answers only the keys it affected.
#[tokio::test]
async fn the_monitored_set_follows_the_sequence_of_updates() {
    /// One step of a sequence: what it activates, what it deactivates, the keys it must
    /// answer, and the keys the account must then report.
    struct Step {
        activates: &'static [&'static str],
        deactivates: &'static [&'static str],
        answers: &'static [&'static str],
        monitors: &'static [&'static str],
    }
    // `(case, the steps)`. A deactivated key is still monitored — `deactivating` — until
    // its span passes, which rule 4 covers; here the question is only WHICH keys each step
    // affects and which the account holds.
    let sequences: [(&str, &[Step]); 3] = [
        (
            "two activations in a row",
            &[
                Step {
                    activates: &["Alpha"],
                    deactivates: &[],
                    answers: &["Alpha"],
                    monitors: &["Alpha"],
                },
                Step {
                    activates: &["Beta", "Gamma"],
                    deactivates: &[],
                    answers: &["Beta", "Gamma"],
                    monitors: &["Alpha", "Beta", "Gamma"],
                },
            ],
        ),
        (
            "an activation then a deactivation of one of its keys",
            &[
                Step {
                    activates: &["Alpha", "Beta"],
                    deactivates: &[],
                    answers: &["Alpha", "Beta"],
                    monitors: &["Alpha", "Beta"],
                },
                Step {
                    activates: &[],
                    deactivates: &["Alpha"],
                    answers: &["Alpha"],
                    monitors: &["Alpha", "Beta"],
                },
            ],
        ),
        (
            "one update that activates and deactivates different keys",
            &[
                Step {
                    activates: &["Alpha"],
                    deactivates: &[],
                    answers: &["Alpha"],
                    monitors: &["Alpha"],
                },
                Step {
                    activates: &["Beta"],
                    deactivates: &["Alpha", "NeverMonitored"],
                    answers: &["Alpha", "Beta"],
                    monitors: &["Alpha", "Beta"],
                },
            ],
        ),
    ];

    for (case, steps) in sequences {
        let router = emulator();
        for (index, step) in steps.iter().enumerate() {
            let mut params = activate(step.activates);
            params.extend(deactivate(step.deactivates));
            let answered = update_ok(case, &router, REGION, &params).await;
            assert_eq!(
                keys_of(&answered),
                step.answers,
                "case {case:?} step {index}: expected the update to answer {:?}, \
                 got {answered:?}",
                step.answers
            );
            let reported = report(case, &router, REGION).await;
            assert_eq!(
                keys_of(&reported),
                step.monitors,
                "case {case:?} step {index}: expected the account to monitor {:?}, \
                 got {reported:?}",
                step.monitors
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the clock carries an activation to `activated`
// ---------------------------------------------------------------------------

/// RULE 3: a key an update activated is `activating` straight after the call and
/// `activated` once the span has passed — whether or not anybody polled it, and whether or
/// not another update ran in between — and a repeat activation leaves a key that got there
/// `activated`.
#[tokio::test]
async fn an_activation_reaches_activated_and_a_repeat_does_not_restart_it() {
    // `(case, whether anybody polls while the activation runs, whether another key is
    // activated while it runs)`.
    let rows: [(&str, bool, bool); 4] = [
        ("nobody asks while the activation runs", false, false),
        ("the key is polled while it activates", true, false),
        ("another key is activated while it runs", false, true),
        ("polled and joined by another key", true, true),
    ];

    for (case, polls, joined) in rows {
        let router = emulator();
        let answered = update_ok(case, &router, REGION, &activate(&["Project"])).await;
        assert_eq!(
            status_of(&answered, "Project").as_deref(),
            Some(MONITORED_TAG_KEY_STATUS_ACTIVATING),
            "case {case:?}: expected the update to answer the transient status, \
             got {answered:?}"
        );
        if polls {
            let reported = report(case, &router, REGION).await;
            assert_eq!(
                status_of(&reported, "Project").as_deref(),
                Some(MONITORED_TAG_KEY_STATUS_ACTIVATING),
                "case {case:?}: expected the key still activating straight after the \
                 update, got {reported:?}"
            );
        }
        if joined {
            update_ok(case, &router, REGION, &activate(&["Other"])).await;
        }

        let settled = poll_until_past(case, &router, "Project", MONITORED_TAG_KEY_STATUS_ACTIVATING)
            .await;
        assert_eq!(
            settled.as_deref(),
            Some(MONITORED_TAG_KEY_STATUS_ACTIVATED),
            "case {case:?}: expected the activation to end at \
             {MONITORED_TAG_KEY_STATUS_ACTIVATED}, got {settled:?}"
        );
        // The status is not a one-off reading: it stays `activated`, and activating the
        // same key again does not send it back to `activating`.
        let again = report(case, &router, REGION).await;
        assert_eq!(
            status_of(&again, "Project").as_deref(),
            Some(MONITORED_TAG_KEY_STATUS_ACTIVATED),
            "case {case:?}: expected the key to stay activated, got {again:?}"
        );
        let repeated = update_ok(case, &router, REGION, &activate(&["Project"])).await;
        assert_eq!(
            status_of(&repeated, "Project").as_deref(),
            Some(MONITORED_TAG_KEY_STATUS_ACTIVATED),
            "case {case:?}: expected a repeat activation to leave the key activated, \
             got {repeated:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the clock carries a deactivation out of the monitored set
// ---------------------------------------------------------------------------

/// RULE 4: a deactivated key is `deactivating` and then monitored no more, the other keys
/// untouched — whichever status the key was in when the deactivation reached it.
#[tokio::test]
async fn a_deactivation_ends_the_monitoring_of_exactly_that_key() {
    // `(case, whether the key is left to reach `activated` before it is deactivated)`.
    let rows: [(&str, bool); 2] = [
        ("the key is deactivated while it is still activating", false),
        ("the key is deactivated once it has activated", true),
    ];

    for (case, settle_first) in rows {
        let router = emulator();
        update_ok(case, &router, REGION, &activate(&["Going", "Staying"])).await;
        if settle_first {
            let settled =
                poll_until_past(case, &router, "Going", MONITORED_TAG_KEY_STATUS_ACTIVATING).await;
            assert_eq!(
                settled.as_deref(),
                Some(MONITORED_TAG_KEY_STATUS_ACTIVATED),
                "case {case:?}: expected the key activated before the deactivation, \
                 got {settled:?}"
            );
        }

        let answered = update_ok(case, &router, REGION, &deactivate(&["Going"])).await;
        assert_eq!(
            keys_of(&answered),
            vec!["Going".to_string()],
            "case {case:?}: expected the deactivation to affect only the key it named, \
             got {answered:?}"
        );
        assert_eq!(
            status_of(&answered, "Going").as_deref(),
            Some(MONITORED_TAG_KEY_STATUS_DEACTIVATING),
            "case {case:?}: expected the transient status, got {answered:?}"
        );

        let gone = poll_until_past(case, &router, "Going", MONITORED_TAG_KEY_STATUS_DEACTIVATING)
            .await;
        assert_eq!(
            gone, None,
            "case {case:?}: expected the deactivated key to be monitored no more, got {gone:?}"
        );
        let reported = report(case, &router, REGION).await;
        assert_eq!(
            keys_of(&reported),
            vec!["Staying".to_string()],
            "case {case:?}: expected the other key untouched, got {reported:?}"
        );
    }
}

/// RULE 4: a deactivation of a key the account does not monitor affects nothing, and a
/// re-activation of a key part-way through its deactivation keeps it monitored.
#[tokio::test]
async fn deactivating_an_unmonitored_key_affects_nothing_and_a_reactivation_cancels_one() {
    let case = "a deactivation of a key the account does not monitor";
    let router = emulator();
    update_ok(case, &router, REGION, &activate(&["Known"])).await;
    let answered = update_ok(case, &router, REGION, &deactivate(&["Unknown"])).await;
    assert!(
        answered.is_empty(),
        "case {case:?}: expected no key to be affected, got {answered:?}"
    );
    let reported = report(case, &router, REGION).await;
    assert_eq!(
        keys_of(&reported),
        vec!["Known".to_string()],
        "case {case:?}: expected the monitored set untouched, got {reported:?}"
    );

    let case = "a re-activation part-way through a deactivation";
    let router = emulator();
    update_ok(case, &router, REGION, &activate(&["Kept"])).await;
    update_ok(case, &router, REGION, &deactivate(&["Kept"])).await;
    let answered = update_ok(case, &router, REGION, &activate(&["Kept"])).await;
    assert_ne!(
        status_of(&answered, "Kept").as_deref(),
        Some(MONITORED_TAG_KEY_STATUS_DEACTIVATING),
        "case {case:?}: expected the deactivation cancelled, got {answered:?}"
    );
    // The cancelled deactivation never falls due, so the key is still monitored after the
    // span it would have ended at.
    let settled = poll_until_past(case, &router, "Kept", MONITORED_TAG_KEY_STATUS_ACTIVATING).await;
    assert_eq!(
        settled.as_deref(),
        Some(MONITORED_TAG_KEY_STATUS_ACTIVATED),
        "case {case:?}: expected the key to stay monitored and reach \
         {MONITORED_TAG_KEY_STATUS_ACTIVATED}, got {settled:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: pagination
// ---------------------------------------------------------------------------

/// RULE 5: however small the page, a walk reports every monitored key exactly once in
/// tag-key order, no page carries more than it asked for, and the last page carries no
/// token.
#[tokio::test]
async fn the_report_pages_over_every_monitored_key_once() {
    let keys = ["Alpha", "Beta", "Delta", "Epsilon", "Gamma"];
    let router = emulator();
    update_ok("the seeding activation", &router, REGION, &activate(&keys)).await;
    let expected: Vec<String> = keys.iter().map(|key| (*key).to_string()).collect();

    for page_size in [1usize, 2, 4, 5, 6] {
        let case = format!("a walk with MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One more round than the walk can possibly need, so a token that never clears
        // fails the assertion below instead of looping.
        for _ in 0..=keys.len() + 1 {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(param("NextToken", token));
            }
            let (status, body) = get(&router, REGION, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = monitored(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} keys in a page, got {page:?}"
            );
            walked.extend(keys_of(&page));
            token = super::ec2_query::element(&body, "nextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            token, None,
            "case {case:?}: expected the walk to end with no token, got {token:?}"
        );
        assert_eq!(
            walked, expected,
            "case {case:?}: expected the walk to report {expected:?} once each, got {walked:?}"
        );
    }
}

/// RULE 5: a `NextToken` this service did not mint is refused rather than read as a
/// position, whatever the account monitors.
#[tokio::test]
async fn a_token_this_service_did_not_mint_is_refused() {
    // `(case, the token the request carries)`.
    let rows: [(&str, &str); 3] = [
        ("a token that is not hex at all", "not-a-token"),
        ("hex that decodes to no cursor of this service", "6162636465"),
        ("an empty token", ""),
    ];

    for (case, token) in rows {
        let router = emulator();
        update_ok(case, &router, REGION, &activate(&["Project"])).await;
        let (status, body) = get(&router, REGION, &[param("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
        assert_eq!(
            error_message(&body),
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE,
            "case {case:?}: expected EC2's own pagination-token sentence, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 6: DryRun
// ---------------------------------------------------------------------------

/// RULE 6: `DryRun` is answered 412 `DryRunOperation` on both operations and leaves the
/// monitored keys exactly where they were.
#[tokio::test]
async fn dry_run_reports_and_changes_nothing() {
    /// One dry-run row: whether a key is already monitored, the operation the `DryRun`
    /// names, and the extra parameters that call carries.
    struct Row {
        case: &'static str,
        monitored_before: bool,
        action: &'static str,
        extra: Vec<Param>,
    }
    let rows = vec![
        Row {
            case: "an activation on an account monitoring nothing",
            monitored_before: false,
            action: "UpdateCapacityManagerMonitoredTagKeys",
            extra: activate(&["Project"]),
        },
        Row {
            case: "an activation on an account already monitoring a key",
            monitored_before: true,
            action: "UpdateCapacityManagerMonitoredTagKeys",
            extra: activate(&["Another"]),
        },
        Row {
            case: "a deactivation of the key the account monitors",
            monitored_before: true,
            action: "UpdateCapacityManagerMonitoredTagKeys",
            extra: deactivate(&["Seeded"]),
        },
        Row {
            case: "the report on an account monitoring nothing",
            monitored_before: false,
            action: "GetCapacityManagerMonitoredTagKeys",
            extra: Vec::new(),
        },
        Row {
            case: "the report on an account monitoring a key",
            monitored_before: true,
            action: "GetCapacityManagerMonitoredTagKeys",
            extra: Vec::new(),
        },
    ];

    for Row {
        case,
        monitored_before,
        action,
        extra,
    } in rows
    {
        let router = emulator();
        let expected: Vec<String> = if monitored_before {
            update_ok(case, &router, REGION, &activate(&["Seeded"])).await;
            vec!["Seeded".to_string()]
        } else {
            Vec::new()
        };

        let mut params = vec![param("DryRun", "true")];
        params.extend(extra);
        let (status, body) = call(&router, REGION, action, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from a DryRun {action}, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        let reported = report(case, &router, REGION).await;
        assert_eq!(
            keys_of(&reported),
            expected,
            "case {case:?}: expected a DryRun to leave the monitored keys at {expected:?}, \
             got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 7: refusals
// ---------------------------------------------------------------------------

/// RULE 7: a request the service cannot read as an update is refused with the code EC2
/// words that condition in, and stores nothing.
#[tokio::test]
async fn an_unreadable_update_is_refused_and_stores_nothing() {
    // A tag key one character past the published bound, and a client token one past its
    // own.
    let too_long_key = "k".repeat(MAX_TAG_KEY_CHARS + 1);
    let too_long_token = "t".repeat(MAX_CLIENT_TOKEN_CHARS + 1);
    let too_many: Vec<String> = (0..=MAX_MONITORED_TAG_KEYS_PER_REQUEST)
        .map(|index| format!("key-{index:04}"))
        .collect();
    let too_many: Vec<&str> = too_many.iter().map(String::as_str).collect();
    // `(case, the parameters, the code EC2 answers)`.
    let rows: Vec<(&str, Vec<Param>, &str)> = vec![
        ("neither list at all", Vec::new(), "MissingParameter"),
        (
            "an empty tag key",
            activate(&[""]),
            "InvalidParameterValue",
        ),
        (
            "a tag key one character past the published bound",
            activate(&[too_long_key.as_str()]),
            "InvalidParameterValue",
        ),
        (
            "more keys in one list than the account could monitor",
            activate(&too_many),
            "InvalidParameterValue",
        ),
        (
            "a client token one character past its bound",
            [activate(&["Project"]), vec![param("ClientToken", &too_long_token)]].concat(),
            "InvalidParameterValue",
        ),
        (
            "one key named for activation and deactivation at once",
            [activate(&["Both"]), deactivate(&["Both"])].concat(),
            "InvalidParameterCombination",
        ),
        (
            "one key of several named for both",
            [
                activate(&["Alpha", "Both"]),
                deactivate(&["Both", "Beta"]),
            ]
            .concat(),
            "InvalidParameterCombination",
        ),
    ];

    for (case, params, expected) in rows {
        let router = emulator();
        let (status, body) = update(&router, REGION, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        let reported = report(case, &router, REGION).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: expected a refused update to store nothing, got {reported:?}"
        );
    }
}

/// RULE 7: the account monitors at most [`MAX_MONITORED_TAG_KEYS`] keys, and an update that
/// would take it past that is refused with the set it already holds left alone.
#[tokio::test]
async fn the_monitored_set_has_a_ceiling() {
    let router = emulator();
    // One short of the ceiling, so the row below can test that a key named TWICE costs one
    // place rather than two.
    let filling: Vec<String> = (0..MAX_MONITORED_TAG_KEYS - 1)
        .map(|index| format!("key-{index:04}"))
        .collect();
    let filling: Vec<&str> = filling.iter().map(String::as_str).collect();
    let case = "an account one key short of the ceiling";
    let answered = update_ok(case, &router, REGION, &activate(&filling)).await;
    assert_eq!(
        answered.len(),
        MAX_MONITORED_TAG_KEYS - 1,
        "case {case:?}: expected {} keys recorded, got {}",
        MAX_MONITORED_TAG_KEYS - 1,
        answered.len()
    );

    // The last place, asked for by a request that names the same new key twice: the set
    // keeps one record per key, so that costs one place and is served.
    let case = "a duplicated new key filling the last place";
    let answered = update_ok(case, &router, REGION, &activate(&["last", "last"])).await;
    assert_eq!(
        keys_of(&answered),
        vec!["last".to_string()],
        "case {case:?}: expected the duplicated key recorded once, got {answered:?}"
    );
    let reported = report(case, &router, REGION).await;
    assert_eq!(
        reported.len(),
        MAX_MONITORED_TAG_KEYS,
        "case {case:?}: expected the account at the ceiling, got {} keys",
        reported.len()
    );

    // One key past the ceiling is refused; a key the account ALREADY monitors costs
    // nothing, so re-activating one is still served.
    let case = "one key past the ceiling";
    let (status, body) = update(&router, REGION, &activate(&["one-too-many"])).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "TagLimitExceeded",
        "case {case:?}: expected TagLimitExceeded, got {body}"
    );
    let reported = report(case, &router, REGION).await;
    assert_eq!(
        reported.len(),
        MAX_MONITORED_TAG_KEYS,
        "case {case:?}: expected the refused update to store nothing, got {} keys",
        reported.len()
    );

    let case = "a key the account already monitors";
    let answered = update_ok(case, &router, REGION, &activate(&[filling[0]])).await;
    assert_eq!(
        keys_of(&answered),
        vec![filling[0].to_string()],
        "case {case:?}: expected the re-activation to be served, got {answered:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 8: the account scope
// ---------------------------------------------------------------------------

/// RULE 8: the monitored keys belong to the account, not to the Region the request was
/// signed for, so keys activated in one Region are reported in every other.
#[tokio::test]
async fn the_monitored_keys_are_a_fact_of_the_account_not_of_one_region() {
    for asked_in in REGIONS {
        let router = emulator();
        let case = format!("activated in {asked_in}");
        update_ok(&case, &router, asked_in, &activate(&["Project", "Owner"])).await;
        for region in REGIONS {
            let case = format!("activated in {asked_in}, asking in {region}");
            let reported = report(&case, &router, region).await;
            assert_eq!(
                keys_of(&reported),
                vec!["Owner".to_string(), "Project".to_string()],
                "case {case:?}: expected both keys reported, got {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What the report leaves off
// ---------------------------------------------------------------------------

/// A monitored key carries no status detail and no datapoint bound: nothing here can stall
/// an activation, and nothing ingests a capacity datapoint yet, so neither member is filled
/// with a guess — the same reading `GetCapacityManagerAttributes` gives the account's own
/// datapoint bounds.
#[tokio::test]
async fn a_monitored_key_reports_no_detail_it_does_not_hold() {
    let case = "a monitored key";
    let router = emulator();
    let answered = update_ok(case, &router, REGION, &activate(&["Project"])).await;
    let reported = report(case, &router, REGION).await;
    for entry in answered.iter().chain(reported.iter()) {
        assert_eq!(
            entry.status_message, None,
            "case {case:?}: expected no statusMessage, got {entry:?}"
        );
        assert_eq!(
            entry.earliest_datapoint_timestamp, None,
            "case {case:?}: expected no earliestDatapointTimestamp, got {entry:?}"
        );
    }
}
