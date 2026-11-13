// SPDX-License-Identifier: CC0-1.0

//! The `Filters` bound, end to end, through EVERY listing of this service that
//! takes a `Filters` list.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The one rule under test, asserted over a TABLE of entry counts crossed with every
//! listing that takes a `Filters` list rather than one row: a listing applies at most
//! the shared maximum of `Filters` entries to one call, answers the same selection
//! for every count at or below it, and refuses a longer list with one
//! `BadRequestException` (400) and one sentence — the same way whichever listing is
//! called, since none of the filter lists these listings model declares a length of
//! its own.

use serde_json::{json, Value};

use service_com_amazonaws_resourcegroups_ardi::state::{check_filters, MAX_FILTERS};

use super::page_size_rule::{listings, page_ids, page_request, Listing, MEMBER_GROUP, TASK_GROUP};
use super::resource_group_lifecycle::{application_group, error_code, Emulator};
use super::tag_sync_tasks::{group_arn, role_arn};

/// The region every request of this suite is signed for, and so the region the
/// fixtures live in.
const REGION: &str = "us-east-1";

/// The memberships recorded on [`MEMBER_GROUP`]: two resources of one type and one
/// of another, so a `resource-type` filter selects a strict subset of them.
fn member_arns() -> Vec<String> {
    vec![
        "arn:aws:ec2:us-east-1:000000000000:instance/i-00000000000000001".to_string(),
        "arn:aws:ec2:us-east-1:000000000000:instance/i-00000000000000002".to_string(),
        "arn:aws:s3:::filters-bound-probe-bucket".to_string(),
    ]
}

/// The two `Filters` entries one listing is driven with: `entry` selects the set the
/// table expects, and `alt` is another spelling of the same selection (a superset of
/// `entry`'s values, which narrows nothing further), so a list of several entries
/// carries entries that DIFFER rather than one repeated.
///
/// Sources for the names and the forms: the filter member of each listing's input —
/// `smithy_query(query="shape",shape="GroupFilter,ResourceFilter,
/// ListGroupingStatusesFilter,ListTagSyncTasksFilter")`.
struct Filtering {
    entry: Value,
    alt: Value,
}

/// The filtering each listing of [`listings`] is driven with, by its label.
fn filtering(label: &str) -> Filtering {
    let arns = member_arns();
    match label {
        // `configuration-type` selects the groups whose service configuration
        // declares one of the values; only the application group does.
        "ListGroups" => Filtering {
            entry: json!({
                "Name": "configuration-type",
                "Values": ["AWS::ResourceGroups::ApplicationGroup"],
            }),
            alt: json!({
                "Name": "configuration-type",
                "Values": ["AWS::ResourceGroups::ApplicationGroup", "AWS::ResourceGroups::Generic"],
            }),
        },
        // `resource-type` selects the memberships of the named type: the two
        // instances, not the bucket.
        "ListGroupResources" => Filtering {
            entry: json!({ "Name": "resource-type", "Values": ["AWS::EC2::Instance"] }),
            alt: json!({
                "Name": "resource-type",
                "Values": ["AWS::EC2::Instance", "AWS::Lambda::Function"],
            }),
        },
        // `resource-arn` selects the records of the resources it names.
        "ListGroupingStatuses" => Filtering {
            entry: json!({ "Name": "resource-arn", "Values": [arns[0], arns[2]] }),
            alt: json!({ "Name": "resource-arn", "Values": [arns[0], arns[1], arns[2]] }),
        },
        // A tag-sync task's filter entry names the group whose tasks are listed,
        // in either of the two ways of naming one.
        "ListTagSyncTasks" => Filtering {
            entry: json!({ "GroupName": TASK_GROUP }),
            alt: json!({ "GroupArn": group_arn(REGION, TASK_GROUP) }),
        },
        other => panic!("case {other:?}: this suite has no filtering for that listing"),
    }
}

/// The identifiers the filtering of `label` selects, in the order that listing
/// reports them: group-name order for the groups, ARN order for everything else.
/// `task_arn` is the tag-sync task the fixtures started, whose ARN the service mints.
fn selected(label: &str, task_arn: &str) -> Vec<String> {
    let arns = member_arns();
    match label {
        "ListGroups" => vec![MEMBER_GROUP.to_string()],
        "ListGroupResources" => vec![arns[0].clone(), arns[1].clone()],
        "ListGroupingStatuses" => vec![arns[0].clone(), arns[2].clone()],
        "ListTagSyncTasks" => vec![task_arn.to_string()],
        other => panic!("case {other:?}: this suite has no selection for that listing"),
    }
}

/// `count` `Filters` entries for `listing`, alternating the two spellings of its
/// selection so the list is not one entry repeated.
fn entries(listing: &Listing, count: usize) -> Value {
    let filtering = filtering(listing.label);
    Value::Array(
        (0..count)
            .map(|index| match index % 2 {
                0 => filtering.entry.clone(),
                _ => filtering.alt.clone(),
            })
            .collect(),
    )
}

/// The request one filtered call of `listing` makes: its addressing members, plus
/// `entries` as `Filters` when there are any to send.
fn filtered_request(listing: &Listing, entries: Option<Value>) -> (String, Value) {
    let (path, mut body) = page_request(listing, None, None);
    if let Some(entries) = entries {
        body.as_object_mut()
            .expect("a listing's request body is a JSON object")
            .insert("Filters".to_string(), entries);
    }
    (path, body)
}

/// The `message` an error response carries.
fn message(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body}"))
        .to_string()
}

/// An emulator holding something for every listing to select over, and the
/// identifiers each listing of [`listings`] is expected to report for its filtering,
/// in the order it reports them.
///
/// `ListGroups` lists the groups of the region, so the two groups the other listings
/// hang off are themselves what it selects over.
async fn populated() -> (Emulator, Vec<Vec<String>>) {
    let emulator = Emulator::new();
    // The group the memberships and the grouping statuses are recorded on, created
    // as the application group both of those listings serve.
    emulator.ok("/groups", &application_group(MEMBER_GROUP)).await;
    // The group the tag-sync task is started on, with no service configuration, so
    // the `configuration-type` filter above does not select it.
    emulator.ok("/groups", &json!({ "Name": TASK_GROUP })).await;

    let arns = member_arns();
    emulator
        .ok(
            "/group-resources",
            &json!({ "Group": MEMBER_GROUP, "ResourceArns": arns }),
        )
        .await;

    let started = emulator
        .ok(
            "/start-tag-sync-task",
            &json!({
                "Group": TASK_GROUP,
                "RoleArn": role_arn("ResourceGroupsTagSync"),
                "TagKey": "FiltersBound",
                "TagValue": "probe",
            }),
        )
        .await;
    let task_arn = started["TaskArn"]
        .as_str()
        .unwrap_or_else(|| panic!("StartTagSyncTask must answer a TaskArn, got {started}"))
        .to_string();

    let expected = listings()
        .iter()
        .map(|listing| selected(listing.label, &task_arn))
        .collect();
    (emulator, expected)
}

/// Every count of `Filters` entries at or below the shared maximum is applied by
/// every listing, and answers the selection the entries name — so the bound is a
/// bound on the list's LENGTH and on nothing else.
#[tokio::test]
async fn every_listing_applies_every_filters_list_up_to_the_shared_maximum() {
    let (emulator, expected) = populated().await;
    // The counts: the smallest list that filters at all, two in the middle, and the
    // published maximum.
    let counts = [1usize, 2, 7, MAX_FILTERS];

    for (listing, expected_ids) in listings().iter().zip(expected) {
        for count in counts {
            let case = format!("{} with {count} Filters entries", listing.label);
            let (path, body) = filtered_request(listing, Some(entries(listing, count)));
            let page = emulator.ok(&path, &body).await;
            let answered = page_ids(listing, &page);
            assert_eq!(
                answered, expected_ids,
                "case {case:?}: expected the selection {expected_ids:?}, got {answered:?}",
            );
        }
        // A `Filters` list with nothing in it narrows nothing, so the listing still
        // covers everything the selection does.
        let case = format!("{} with an empty Filters list", listing.label);
        let (path, body) = filtered_request(listing, Some(json!([])));
        let page = emulator.ok(&path, &body).await;
        let answered = page_ids(listing, &page);
        for id in &expected_ids {
            assert!(
                answered.contains(id),
                "case {case:?}: an empty list narrows nothing, so {id:?} must still be \
                 listed, got {answered:?}",
            );
        }
    }
}

/// A `Filters` list past the shared maximum is refused by every listing: 400
/// `BadRequestException`, naming the member, the count and the bound — and every
/// listing refuses it with the one same answer.
#[tokio::test]
async fn every_listing_refuses_a_filters_list_past_the_shared_maximum() {
    let (emulator, expected) = populated().await;
    // One past the published maximum, well past it, and a list far larger than any
    // listing could apply per item.
    for count in [MAX_FILTERS + 1, MAX_FILTERS * 3, 1_000] {
        // (the status, the error code, the message) of each listing, which the rule
        // requires to be the same answer.
        let mut answers: Vec<(u16, String, String)> = Vec::new();
        for listing in listings() {
            let case = format!("{} with {count} Filters entries", listing.label);
            let (path, body) = filtered_request(&listing, Some(entries(&listing, count)));
            let (status, answered) = emulator.call(&path, &body).await;
            let code = error_code(&answered).to_string();
            assert_eq!(
                (status, code.as_str()),
                (400, "BadRequestException"),
                "case {case:?}: expected 400 BadRequestException, got {status} {answered}",
            );
            let text = message(&case, &answered);
            assert!(
                text.to_lowercase().contains("filters")
                    && text.contains(&count.to_string())
                    && text.contains(&MAX_FILTERS.to_string()),
                "case {case:?}: the refusal must name Filters, the count and the bound \
                 {MAX_FILTERS}, got {text:?}",
            );
            answers.push((status, code, text));
        }
        let first = &answers[0];
        for (answer, listing) in answers.iter().zip(listings()) {
            assert_eq!(
                answer, first,
                "case {count} Filters entries: every listing must refuse the list with \
                 one answer, but {} answered {answer:?}",
                listing.label,
            );
        }
    }

    // A refused list listed nothing and changed nothing: every listing still answers
    // the same selection afterwards.
    for (listing, expected_ids) in listings().iter().zip(expected) {
        let (path, body) = filtered_request(listing, Some(entries(listing, 1)));
        let page = emulator.ok(&path, &body).await;
        let answered = page_ids(listing, &page);
        assert_eq!(
            answered, expected_ids,
            "case {}: a refused request must change no listing, got {answered:?}",
            listing.label,
        );
    }
}

/// The one shared `Filters` rule itself, over the whole domain of an entry count —
/// including the counts no fixture builds a request for, since the bound is what
/// keeps one request's work finite.
///
/// None of the modelled filter lists declares a length, so the boundary validator
/// refuses no count at all (the suites above assert the handlers' refusal over the
/// wire) and this is the one place the shared rule's own sentence is observable.
#[test]
fn the_shared_filters_rule_spans_the_whole_entry_count_domain() {
    // Every count at or below the bound, from a list with nothing in it to the
    // published maximum: this rule bounds how much work one request can ask for, so
    // it says nothing about the small end.
    for count in 0..=MAX_FILTERS {
        let answered = check_filters(count);
        assert_eq!(
            answered,
            Ok(()),
            "case {count} entries: a list at or below the bound is applied, got {answered:?}",
        );
    }
    // One past the bound, well past it, and a count far past anything a fixture
    // sends.
    for count in [MAX_FILTERS + 1, MAX_FILTERS * 3, 1_000, usize::MAX] {
        let answered = check_filters(count);
        assert_eq!(
            answered,
            Err(format!(
                "Filters carries {count} filters, which is more than the {MAX_FILTERS} \
                 this emulator applies in one call"
            )),
            "case {count} entries: a list past the bound is refused with the one shared \
             sentence, got {answered:?}",
        );
    }
}
