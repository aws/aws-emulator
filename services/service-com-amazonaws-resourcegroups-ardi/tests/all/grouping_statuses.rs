// SPDX-License-Identifier: CC0-1.0

//! `ListGroupingStatuses`, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * a resource a `GroupResources` call records is reported once, with the action
//!   that was performed on it (`GROUP`), how that action ended (`SUCCESS`), its own
//!   ARN and the time the action was recorded;
//! * the listing is computed from the store: it covers exactly the addressed
//!   group's records, in resource-ARN order, and answers the same whether the group
//!   is addressed by name or by ARN;
//! * each modelled filter name selects over the member it names, a filter matches a
//!   record when ANY of its values does, and every filter a request carries has to
//!   hold;
//! * paging walks every record exactly once at every page size, and a token is
//!   accepted only by the listing that issued it;
//! * reporting changes nothing: a second call answers the same records with the
//!   same timestamps;
//! * a request that violates a validation rule is `BadRequestException` (400), a group
//!   that is not an application group is `BadRequestException` (400) with the one fixed
//!   sentence for that target, and an identifier that names no stored group of the
//!   region is `NotFoundException` (404).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::resource_group_lifecycle::{application_group, error_code, is_modelled_token, Emulator};

/// Every value the modelled `GroupingStatus` enum declares — the domain of the
/// `status` filter.
///
/// Source: `smithy_query(query="shape",shape="GroupingStatus")` on this service's
/// model, which answers `["SUCCESS", "FAILED", "IN_PROGRESS", "SKIPPED"]`.
const GROUPING_STATUS_VALUES: [&str; 4] = ["SUCCESS", "FAILED", "IN_PROGRESS", "SKIPPED"];

/// Resource ARNs spanning the forms the modelled `ResourceArn` pattern admits: a
/// `type/id` resource, a `type:id` resource, a region-less and account-less global
/// resource, another partition, and a resource whose type this emulator has no
/// CloudFormation-style name for.
fn sample_arns() -> Vec<&'static str> {
    vec![
        "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
        "arn:aws:lambda:eu-west-1:000000000000:function:report",
        "arn:aws:s3:::grouping-statuses-bucket",
        "arn:aws-us-gov:dynamodb:us-gov-west-1:000000000000:table/Orders",
        "arn:aws:glacier:us-east-1:000000000000:vaults/records",
    ]
}

/// Seconds since the epoch, as the restJson1 wire carries a modelled `timestamp`.
fn epoch_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// The `GroupingStatuses` list of a page.
fn items(page: &Value) -> Vec<Value> {
    page["GroupingStatuses"]
        .as_array()
        .unwrap_or_else(|| panic!("GroupingStatuses must be a list, got {page}"))
        .clone()
}

/// The `ResourceArn` of every record on a page, in the order the page lists them.
fn item_arns(page: &Value) -> Vec<String> {
    items(page)
        .iter()
        .map(|item| {
            item["ResourceArn"]
                .as_str()
                .unwrap_or_else(|| panic!("a record must carry ResourceArn, got {item}"))
                .to_string()
        })
        .collect()
}

/// Create `name` and record `arns` as its members. Returns the group's ARN.
///
/// The group is created as an APPLICATION group (see `application_group`): that is
/// the target this operation reports on, and its configuration is also one a
/// static-membership operation serves, since `GroupResources` is the only way a
/// grouping status is recorded.
async fn group_with_members(emulator: &Emulator, name: &str, arns: &[&str]) -> String {
    let created = emulator.ok("/groups", &application_group(name)).await;
    let group_arn = created["Group"]["GroupArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateGroup must answer a GroupArn, got {created}"))
        .to_string();
    // `ResourceArns` is modelled `length 1..10`, so a longer membership is recorded
    // in chunks of ten.
    for chunk in arns.chunks(10) {
        emulator
            .ok(
                "/group-resources",
                &json!({ "Group": name, "ResourceArns": chunk }),
            )
            .await;
    }
    group_arn
}

/// Every resource a `GroupResources` call records is reported once, with the action
/// performed on it, how it ended, its own ARN and when it was recorded — and the
/// listing is the same whether the group is addressed by name or by its ARN.
#[tokio::test]
async fn every_recorded_membership_is_reported_with_its_action_status_and_time() {
    // (label, group name, the ARNs recorded in it)
    let all = sample_arns();
    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("one membership", "statuses-one", all[..1].to_vec()),
        ("three memberships", "statuses-three", all[..3].to_vec()),
        ("every ARN form", "statuses-all", all.clone()),
        (
            "an ARN recorded twice in one request",
            "statuses-repeated",
            vec![all[0], all[0], all[1]],
        ),
    ];

    for (label, name, arns) in cases {
        let emulator = Emulator::new();
        let before = epoch_now();
        let group_arn = group_with_members(&emulator, name, &arns).await;
        let after = epoch_now();

        let expected: Vec<String> = arns
            .iter()
            .map(|arn| (*arn).to_string())
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect();

        for identifier in [name.to_string(), group_arn.clone()] {
            let page = emulator
                .ok(
                    "/list-grouping-statuses",
                    &json!({ "Group": identifier }),
                )
                .await;
            assert_eq!(
                page["Group"].as_str(),
                Some(group_arn.as_str()),
                "case {label:?} addressed as {identifier:?}: expected Group {group_arn}, \
                 got {page}",
            );
            assert_eq!(
                item_arns(&page),
                expected,
                "case {label:?} addressed as {identifier:?}: expected the recorded ARNs in \
                 ARN order, got {page}",
            );
            assert!(
                page.get("NextToken").is_none_or(Value::is_null),
                "case {label:?} addressed as {identifier:?}: a complete page must carry no \
                 NextToken, got {page}",
            );
            for item in items(&page) {
                assert_eq!(
                    item["Action"].as_str(),
                    Some("GROUP"),
                    "case {label:?}: a recorded membership's Action must be GROUP, got {item}",
                );
                assert_eq!(
                    item["Status"].as_str(),
                    Some("SUCCESS"),
                    "case {label:?}: a recorded membership's Status must be SUCCESS, got {item}",
                );
                for absent in ["ErrorCode", "ErrorMessage"] {
                    assert!(
                        item.get(absent).is_none_or(Value::is_null),
                        "case {label:?}: an action that did not fail must carry no {absent}, \
                         got {item}",
                    );
                }
                let updated = item["UpdatedAt"].as_f64().unwrap_or_else(|| {
                    panic!("case {label:?}: UpdatedAt must be a wire timestamp, got {item}")
                });
                assert!(
                    (before - 1.0..=after + 1.0).contains(&updated),
                    "case {label:?}: UpdatedAt {updated} must be the time the action was \
                     recorded (between {before} and {after}), got {item}",
                );
            }
        }
    }
}

/// A group that holds no recorded action reports an empty listing — not a failure,
/// and not another group's records.
#[tokio::test]
async fn a_group_with_no_recorded_action_reports_an_empty_listing() {
    let emulator = Emulator::new();
    let arns = sample_arns();
    group_with_members(&emulator, "with-members", &arns[..2]).await;
    emulator.ok("/groups", &application_group("untouched")).await;

    // (the group, the ARNs it must report)
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("untouched", Vec::new()),
        (
            "with-members",
            arns[..2]
                .iter()
                .map(|arn| (*arn).to_string())
                .collect::<BTreeSet<String>>()
                .into_iter()
                .collect(),
        ),
    ];
    for (name, expected) in cases {
        let page = emulator
            .ok("/list-grouping-statuses", &json!({ "Group": name }))
            .await;
        assert_eq!(
            item_arns(&page),
            expected,
            "case {name:?}: a group reports its own records and no others, got {page}",
        );
    }
}

/// Each modelled filter name selects over the member it names; a filter matches a
/// record when ANY of its values does; and every filter a request carries has to
/// hold.
#[tokio::test]
async fn filters_select_over_the_member_each_one_names() {
    let emulator = Emulator::new();
    let arns = sample_arns();
    group_with_members(&emulator, "filtered", &arns).await;
    let all: Vec<String> = arns
        .iter()
        .map(|arn| (*arn).to_string())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect();
    let sorted = |selected: &[&str]| -> Vec<String> {
        selected
            .iter()
            .map(|arn| (*arn).to_string())
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect()
    };

    // (label, the Filters list, the ARNs the page must carry). Every status this
    // emulator does not record selects nothing, which is every `GroupingStatus`
    // value but the first — the catalog above, not only the values this group holds.
    let mut cases: Vec<(String, Value, Vec<String>)> = vec![
        ("no filter at all".to_string(), json!([]), all.clone()),
        (
            "the recorded status".to_string(),
            json!([{ "Name": "status", "Values": ["SUCCESS"] }]),
            all.clone(),
        ),
        (
            "two status values, one of them recorded".to_string(),
            json!([{ "Name": "status", "Values": ["FAILED", "SUCCESS"] }]),
            all.clone(),
        ),
        (
            "one resource ARN".to_string(),
            json!([{ "Name": "resource-arn", "Values": [arns[2]] }]),
            sorted(&[arns[2]]),
        ),
        (
            "three resource ARNs".to_string(),
            json!([{ "Name": "resource-arn", "Values": [arns[0], arns[2], arns[4]] }]),
            sorted(&[arns[0], arns[2], arns[4]]),
        ),
        (
            "a resource ARN this group does not hold".to_string(),
            json!([{
                "Name": "resource-arn",
                "Values": ["arn:aws:ec2:us-east-1:000000000000:instance/i-00000000000000000"]
            }]),
            Vec::new(),
        ),
        (
            "a status filter and a resource-arn filter that agree".to_string(),
            json!([
                { "Name": "status", "Values": ["SUCCESS"] },
                { "Name": "resource-arn", "Values": [arns[1]] },
            ]),
            sorted(&[arns[1]]),
        ),
        (
            "a status filter and a resource-arn filter that disagree".to_string(),
            json!([
                { "Name": "status", "Values": ["FAILED"] },
                { "Name": "resource-arn", "Values": [arns[1]] },
            ]),
            Vec::new(),
        ),
    ];
    for status in GROUPING_STATUS_VALUES.iter().skip(1) {
        cases.push((
            format!("the unrecorded status {status}"),
            json!([{ "Name": "status", "Values": [status] }]),
            Vec::new(),
        ));
    }

    for (label, filters, expected) in cases {
        let page = emulator
            .ok(
                "/list-grouping-statuses",
                &json!({ "Group": "filtered", "Filters": filters }),
            )
            .await;
        assert_eq!(
            item_arns(&page),
            expected,
            "case {label:?}: expected {expected:?}, got {page}",
        );
    }
}

/// Paging walks every record exactly once at every page size, the token it hands
/// back is the opaque form the model describes, and a request that asks for no page
/// size is answered at the modelled maximum.
#[tokio::test]
async fn paging_walks_every_record_exactly_once_for_every_page_size() {
    let emulator = Emulator::new();
    // 52 memberships: two more than one default page holds.
    let owned: Vec<String> = (1..=52)
        .map(|i| format!("arn:aws:ec2:us-east-1:000000000000:instance/i-{i:017x}"))
        .collect();
    let arns: Vec<&str> = owned.iter().map(String::as_str).collect();
    group_with_members(&emulator, "paged", &arns).await;
    let expected: Vec<String> = owned.iter().cloned().collect::<BTreeSet<_>>().into_iter().collect();

    for page_size in [1usize, 2, 7, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=expected.len() {
            let mut body = json!({ "Group": "paged", "MaxResults": page_size });
            if let Some(t) = token.as_deref() {
                body["NextToken"] = json!(t);
            }
            let page = emulator.ok("/list-grouping-statuses", &body).await;
            let page_arns = item_arns(&page);
            assert!(
                page_arns.len() <= page_size,
                "case MaxResults={page_size}: a page must hold at most {page_size} records, \
                 got {}",
                page_arns.len(),
            );
            seen.extend(page_arns);
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            match token.as_deref() {
                Some(t) => assert!(
                    is_modelled_token(t),
                    "case MaxResults={page_size}: NextToken {t:?} must match the modelled \
                     ^[a-zA-Z0-9+/]*={{0,2}}$",
                ),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case MaxResults={page_size}: paging must walk every record exactly once, in ARN \
             order",
        );
    }

    // No `MaxResults` at all: the page is bounded at the modelled maximum of 50,
    // and the token walks the rest.
    let first = emulator
        .ok("/list-grouping-statuses", &json!({ "Group": "paged" }))
        .await;
    assert_eq!(
        item_arns(&first).len(),
        50,
        "case absent MaxResults: the default page holds the modelled maximum of 50, got {first}",
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case absent MaxResults: expected a NextToken, got {first}"));
    let second = emulator
        .ok(
            "/list-grouping-statuses",
            &json!({ "Group": "paged", "NextToken": token }),
        )
        .await;
    let walked: Vec<String> = item_arns(&first)
        .into_iter()
        .chain(item_arns(&second))
        .collect();
    assert_eq!(
        walked, expected,
        "case absent MaxResults: two pages must walk every record exactly once",
    );
    assert!(
        second.get("NextToken").is_none_or(Value::is_null),
        "case absent MaxResults: the last page must carry no NextToken, got {second}",
    );
}

/// Reporting changes nothing: a second call answers the same records with the same
/// timestamps, and re-recording a membership replaces its record rather than adding
/// a second one.
#[tokio::test]
async fn reporting_is_repeatable_and_a_repeated_grouping_keeps_one_record_per_resource() {
    let emulator = Emulator::new();
    let arns = sample_arns();
    group_with_members(&emulator, "stable", &arns[..3]).await;

    let first = emulator
        .ok("/list-grouping-statuses", &json!({ "Group": "stable" }))
        .await;
    let second = emulator
        .ok("/list-grouping-statuses", &json!({ "Group": "stable" }))
        .await;
    assert_eq!(
        items(&first),
        items(&second),
        "case repeated read: a reporting operation must answer the same records, including \
         their timestamps",
    );

    // Recording the same ARN again is one resource with one record — the LAST
    // action on it — so the listing's size does not grow.
    emulator
        .ok(
            "/group-resources",
            &json!({ "Group": "stable", "ResourceArns": [arns[0]] }),
        )
        .await;
    let third = emulator
        .ok("/list-grouping-statuses", &json!({ "Group": "stable" }))
        .await;
    assert_eq!(
        item_arns(&third),
        item_arns(&first),
        "case repeated grouping: a re-recorded resource keeps exactly one record, got {third}",
    );
}

/// A request that violates a validation rule is `BadRequestException` (400), while an
/// identifier that names no stored group of the region is the service's one not-found
/// class — asserted in `application_group_target`, which crosses both refusals with
/// every form of identifier.
#[tokio::test]
async fn invalid_requests_are_refused_as_bad_request() {
    let emulator = Emulator::new();
    let arns = sample_arns();
    group_with_members(&emulator, "refusals", &arns[..2]).await;
    group_with_members(&emulator, "other", &arns[..2]).await;

    // A token issued by this group's grouping-status listing, and one issued by
    // another listing of the same group: neither names a resume point in the other.
    let own_page = emulator
        .ok(
            "/list-grouping-statuses",
            &json!({ "Group": "other", "MaxResults": 1 }),
        )
        .await;
    let other_groups_token = own_page["NextToken"]
        .as_str()
        .expect("a one-record page of two records carries a NextToken")
        .to_string();
    let members_page = emulator
        .ok(
            "/list-group-resources",
            &json!({ "Group": "refusals", "MaxResults": 1 }),
        )
        .await;
    let members_token = members_page["NextToken"]
        .as_str()
        .expect("a one-membership page of two memberships carries a NextToken")
        .to_string();

    let cases: Vec<(String, Value)> = vec![
        (
            "no Group at all".to_string(),
            json!({}),
        ),
        (
            "a filter name the model does not declare".to_string(),
            json!({ "Group": "refusals", "Filters": [{ "Name": "nickname", "Values": ["x"] }] }),
        ),
        (
            "a filter name in the wrong case".to_string(),
            json!({ "Group": "refusals", "Filters": [{ "Name": "Status", "Values": ["SUCCESS"] }] }),
        ),
        (
            "a status value outside the modelled enum".to_string(),
            json!({
                "Group": "refusals",
                "Filters": [{ "Name": "status", "Values": ["SUCCESSFUL"] }]
            }),
        ),
        (
            "a status value that is a resource ARN".to_string(),
            json!({
                "Group": "refusals",
                "Filters": [{ "Name": "status", "Values": [arns[0]] }]
            }),
        ),
        (
            "a resource-arn value that is not a resource ARN".to_string(),
            json!({
                "Group": "refusals",
                "Filters": [{ "Name": "resource-arn", "Values": ["SUCCESS"] }]
            }),
        ),
        (
            "a NextToken this service never issued".to_string(),
            json!({ "Group": "refusals", "NextToken": "bm90LWEtdG9rZW4h" }),
        ),
        (
            "a NextToken issued for another group's grouping statuses".to_string(),
            json!({ "Group": "refusals", "NextToken": other_groups_token }),
        ),
        (
            "a NextToken issued by this group's membership listing".to_string(),
            json!({ "Group": "refusals", "NextToken": members_token }),
        ),
        (
            "MaxResults below the modelled range".to_string(),
            json!({ "Group": "refusals", "MaxResults": 0 }),
        ),
        (
            "MaxResults past the modelled range".to_string(),
            json!({ "Group": "refusals", "MaxResults": 51 }),
        ),
        (
            "a negative MaxResults".to_string(),
            json!({ "Group": "refusals", "MaxResults": -1 }),
        ),
        (
            "a filter with no values".to_string(),
            json!({ "Group": "refusals", "Filters": [{ "Name": "status", "Values": [] }] }),
        ),
    ];

    for (label, body) in cases {
        let (status, answered) = emulator.call("/list-grouping-statuses", &body).await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "BadRequestException"),
            "case {label:?}: expected 400 BadRequestException, got {status} {answered}",
        );
    }

    // A refused request reports nothing and changes nothing: the records are still
    // the two that were recorded.
    let page = emulator
        .ok("/list-grouping-statuses", &json!({ "Group": "refusals" }))
        .await;
    assert_eq!(
        item_arns(&page).len(),
        2,
        "a refused request must change no record, got {page}",
    );
}
