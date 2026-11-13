// SPDX-License-Identifier: CC0-1.0

//! The page-size rule, end to end, through one operation of EVERY listing this
//! service serves.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The one rule under test, asserted over a TABLE of page sizes crossed with every
//! listing rather than one row: a listing answers at most `MaxResults` items per
//! page, defaults an absent `MaxResults` to the modelled maximum (the `MaxResults`
//! shape every listing's member targets is `range 1..50`), pages through every
//! selected item exactly once whatever the size, and refuses a size outside that
//! range — the same way whichever listing is called.

use serde_json::{json, Value};

use super::resource_group_lifecycle::{
    application_group, error_code, is_modelled_token, query_encode, Emulator,
};
use super::tag_sync_tasks::role_arn;

/// The largest page any listing of this service answers, and the page size it
/// answers when the caller asks for none.
///
/// Source: the `MaxResults` member of each listing's input, which every one of them
/// targets — `smithy_query(query="operation",operation="ListGroups,ListGroupResources,
/// ListGroupingStatuses,ListTagSyncTasks")` reports `range {min: 1, max: 50}` on all
/// four.
const MAX_PAGE_SIZE: usize = 50;

/// How many items each listing is populated with: one past [`MAX_PAGE_SIZE`], so a
/// call that sends no `MaxResults` has to leave something behind and say so with a
/// `NextToken`.
const ITEMS: usize = MAX_PAGE_SIZE + 1;

/// The group the memberships and the grouping statuses are recorded on. Created as
/// an APPLICATION group, which both admits static memberships and is the target
/// `ListGroupingStatuses` reports on (see `application_group`).
///
/// Shared with the sibling suites of this target that drive the same listings, so
/// one set of fixtures names the group every table row addresses.
pub(crate) const MEMBER_GROUP: &str = "page-00";

/// The group the tag-sync tasks are started on.
pub(crate) const TASK_GROUP: &str = "page-01";

/// Where a listing's `MaxResults` and `NextToken` ride: `ListGroups` binds both to
/// the query string, every other listing of this service takes them in the payload.
#[derive(Debug, Clone, Copy)]
enum PagedIn {
    Query,
    Body,
}

/// One listing of this service: how a page of it is asked for, and how the items of
/// a page are identified.
///
/// Shared with the sibling suites of this target, so every rule that holds of every
/// listing is asserted over ONE table of them.
pub(crate) struct Listing {
    pub(crate) label: &'static str,
    path: &'static str,
    /// The members that address WHAT is listed, without any paging member.
    base: fn() -> Value,
    paged_in: PagedIn,
    /// The output member carrying the page.
    items_member: &'static str,
    /// The member of one item that identifies it.
    id_member: &'static str,
}

/// Every listing of this service, which is every place the page-size rule holds.
pub(crate) fn listings() -> Vec<Listing> {
    vec![
        Listing {
            label: "ListGroups",
            path: "/groups-list",
            base: || json!({}),
            paged_in: PagedIn::Query,
            items_member: "GroupIdentifiers",
            id_member: "GroupName",
        },
        Listing {
            label: "ListGroupResources",
            path: "/list-group-resources",
            base: || json!({ "Group": MEMBER_GROUP }),
            paged_in: PagedIn::Body,
            items_member: "ResourceIdentifiers",
            id_member: "ResourceArn",
        },
        Listing {
            label: "ListGroupingStatuses",
            path: "/list-grouping-statuses",
            base: || json!({ "Group": MEMBER_GROUP }),
            paged_in: PagedIn::Body,
            items_member: "GroupingStatuses",
            id_member: "ResourceArn",
        },
        Listing {
            label: "ListTagSyncTasks",
            path: "/list-tag-sync-tasks",
            base: || json!({}),
            paged_in: PagedIn::Body,
            items_member: "TagSyncTasks",
            id_member: "TaskArn",
        },
    ]
}

/// The resource ARNs the fixtures record as memberships: one per item, each a
/// distinct instance.
fn member_arns() -> Vec<String> {
    (0..ITEMS)
        .map(|index| format!("arn:aws:ec2:us-east-1:000000000000:instance/i-{index:017x}"))
        .collect()
}

/// An emulator holding [`ITEMS`] items in every listing, and the identifiers each
/// listing is expected to walk, in the order it reports them.
///
/// `ListGroups` lists the groups of the region, so the two groups the other listings
/// hang off are themselves part of its [`ITEMS`] groups.
async fn populated() -> (Emulator, Vec<Vec<String>>) {
    let emulator = Emulator::new();
    let mut group_names: Vec<String> = Vec::new();
    for index in 0..ITEMS {
        let name = format!("page-{index:02}");
        let body = if name == MEMBER_GROUP {
            application_group(&name)
        } else {
            json!({ "Name": name })
        };
        emulator.ok("/groups", &body).await;
        group_names.push(name);
    }

    let arns = member_arns();
    // `ResourceArns` is modelled `length 1..10`, so the membership is recorded in
    // chunks of ten.
    for chunk in arns.chunks(10) {
        emulator
            .ok(
                "/group-resources",
                &json!({ "Group": MEMBER_GROUP, "ResourceArns": chunk }),
            )
            .await;
    }

    let mut task_arns: Vec<String> = Vec::new();
    for index in 0..ITEMS {
        let started = emulator
            .ok(
                "/start-tag-sync-task",
                &json!({
                    "Group": TASK_GROUP,
                    "RoleArn": role_arn("ResourceGroupsTagSync"),
                    "TagKey": format!("Key{index:02}"),
                    "TagValue": "paged",
                }),
            )
            .await;
        task_arns.push(
            started["TaskArn"]
                .as_str()
                .unwrap_or_else(|| panic!("StartTagSyncTask must answer a TaskArn, got {started}"))
                .to_string(),
        );
    }

    let mut sorted_arns = arns.clone();
    sorted_arns.sort();
    task_arns.sort();
    group_names.sort();
    // One expected walk per entry of `listings()`, in the same order.
    let expected = vec![group_names, sorted_arns.clone(), sorted_arns, task_arns];
    (emulator, expected)
}

/// The path and payload of one page request: `size` is the `MaxResults` asked for
/// (`None` asks for none) and `token` the `NextToken` to resume after.
pub(crate) fn page_request(
    listing: &Listing,
    size: Option<i64>,
    token: Option<&str>,
) -> (String, Value) {
    let mut body = (listing.base)();
    let mut query: Vec<String> = Vec::new();
    match listing.paged_in {
        PagedIn::Query => {
            if let Some(size) = size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = token {
                query.push(format!("nextToken={}", query_encode(token)));
            }
        }
        PagedIn::Body => {
            let object = body.as_object_mut().expect("a JSON object");
            if let Some(size) = size {
                object.insert("MaxResults".to_string(), json!(size));
            }
            if let Some(token) = token {
                object.insert("NextToken".to_string(), json!(token));
            }
        }
    }
    let path = match query.is_empty() {
        true => listing.path.to_string(),
        false => format!("{}?{}", listing.path, query.join("&")),
    };
    (path, body)
}

/// The identifiers one page carries, in the order the page lists them.
pub(crate) fn page_ids(listing: &Listing, page: &Value) -> Vec<String> {
    page[listing.items_member]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {}: a page must carry {}, got {page}",
                listing.label, listing.items_member,
            )
        })
        .iter()
        .map(|item| {
            item[listing.id_member]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {}: an item must carry {}, got {item}",
                        listing.label, listing.id_member,
                    )
                })
                .to_string()
        })
        .collect()
}

/// Every listing pages through every item exactly once, in its own order, at every
/// page size the modelled range admits — and a call that asks for no `MaxResults`
/// answers at the modelled maximum.
#[tokio::test]
async fn every_listing_pages_at_the_requested_size_and_defaults_to_the_modelled_maximum() {
    let (emulator, expected) = populated().await;
    // The page sizes: the smallest the range admits, two in the middle, the largest
    // it admits, and no `MaxResults` at all — which is the modelled maximum.
    let sizes: [Option<i64>; 5] = [Some(1), Some(2), Some(7), Some(MAX_PAGE_SIZE as i64), None];

    for (listing, expected_ids) in listings().iter().zip(expected) {
        assert_eq!(
            expected_ids.len(),
            ITEMS,
            "case {}: the fixtures must hold {ITEMS} items, got {}",
            listing.label,
            expected_ids.len(),
        );
        for size in sizes {
            let case = format!("{} at MaxResults={size:?}", listing.label);
            // An absent `MaxResults` is the modelled maximum, which is what makes
            // the two spellings one rule.
            let effective = usize::try_from(size.unwrap_or(MAX_PAGE_SIZE as i64))
                .expect("a page size inside the modelled range");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let (path, body) = page_request(listing, size, token.as_deref());
                let page = emulator.ok(&path, &body).await;
                let ids = page_ids(listing, &page);
                assert!(
                    ids.len() <= effective,
                    "case {case:?}: a page must carry at most {effective} items, got {}",
                    ids.len(),
                );
                let remaining = ITEMS - seen.len();
                assert_eq!(
                    ids.len(),
                    effective.min(remaining),
                    "case {case:?}: with {remaining} items left a page must carry \
                     {} of them, got {ids:?}",
                    effective.min(remaining),
                );
                seen.extend(ids);
                token = match page["NextToken"].as_str() {
                    Some(next) => {
                        assert!(
                            is_modelled_token(next),
                            "case {case:?}: NextToken {next:?} is not the modelled form",
                        );
                        Some(next.to_string())
                    }
                    None => None,
                };
                assert_eq!(
                    token.is_some(),
                    seen.len() < ITEMS,
                    "case {case:?}: a NextToken is present exactly while items remain, \
                     got {token:?} after {} of {ITEMS} items",
                    seen.len(),
                );
                pages += 1;
                assert!(
                    pages <= ITEMS + 1,
                    "case {case:?}: paging did not terminate, saw {seen:?}",
                );
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                seen, expected_ids,
                "case {case:?}: paging must walk every item exactly once, in the \
                 listing's own order, got {seen:?}",
            );
            assert_eq!(
                pages,
                ITEMS.div_ceil(effective),
                "case {case:?}: {ITEMS} items at {effective} per page is \
                 {} pages, got {pages}",
                ITEMS.div_ceil(effective),
            );
        }
    }
}

/// A `MaxResults` outside the range every listing's member declares is refused by
/// every listing: 400 `BadRequestException`, naming the member and the bound the
/// value failed, and listing nothing.
#[tokio::test]
async fn every_listing_refuses_a_max_results_outside_the_modelled_range() {
    let (emulator, expected) = populated().await;
    // (the value, the bound it fails) — one below the published minimum, one past
    // the published maximum, and a negative value, the one a lossy conversion would
    // read as "every item".
    let cases: [(i64, usize); 3] = [(0, 1), (MAX_PAGE_SIZE as i64 + 1, MAX_PAGE_SIZE), (-1, 1)];
    for (value, bound) in cases {
        for listing in listings() {
            let case = format!("{} at MaxResults={value}", listing.label);
            let (path, body) = page_request(&listing, Some(value), None);
            let (status, answered) = emulator.call(&path, &body).await;
            assert_eq!(
                (status, error_code(&answered)),
                (400, "BadRequestException"),
                "case {case:?}: expected 400 BadRequestException, got {status} {answered}",
            );
            let message = answered
                .get("message")
                .or_else(|| answered.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {answered}"))
                .to_string();
            assert!(
                message.to_lowercase().contains("maxresults")
                    && message.contains(&value.to_string())
                    && message.contains(&bound.to_string()),
                "case {case:?}: the refusal must name MaxResults, the value and the \
                 bound {bound}, got {message:?}",
            );
        }
    }
    // A refused page size listed nothing and changed nothing: every listing still
    // walks the same items afterwards.
    for (listing, expected_ids) in listings().iter().zip(expected) {
        let (path, body) = page_request(listing, Some(MAX_PAGE_SIZE as i64), None);
        let page = emulator.ok(&path, &body).await;
        assert_eq!(
            page_ids(listing, &page),
            expected_ids[..MAX_PAGE_SIZE],
            "case {}: a refused request must change no listing",
            listing.label,
        );
    }
}

/// The one page-size rule itself, over the whole domain of the `MaxResults` member
/// — including the values no request can carry, since a page size is read from a
/// signed 32-bit integer and a lossy conversion would read a negative one as "every
/// item".
///
/// The boundary validator refuses a value outside the modelled range before any
/// handler runs (the suite above asserts that over the wire), so this is the one
/// place the shared rule's own refusal is observable.
#[test]
fn the_shared_page_size_rule_spans_the_whole_max_results_domain() {
    use service_com_amazonaws_resourcegroups_ardi::state::page_size;

    // An absent `MaxResults` is the modelled maximum.
    assert_eq!(
        page_size(None),
        Ok(MAX_PAGE_SIZE),
        "case None: an absent MaxResults pages at the modelled maximum",
    );
    // Every size the modelled range admits, including both published bounds.
    for size in [1usize, 2, 7, 49, MAX_PAGE_SIZE] {
        let answered = page_size(Some(size as i32));
        assert_eq!(
            answered,
            Ok(size),
            "case MaxResults={size}: a size inside the modelled range is the page \
             size, got {answered:?}",
        );
    }
    // One past each published bound, and the extremes of the member's own type.
    for value in [0i32, MAX_PAGE_SIZE as i32 + 1, -1, i32::MIN, i32::MAX] {
        let answered = page_size(Some(value));
        assert_eq!(
            answered,
            Err(format!(
                "MaxResults {value} is not valid: it must be between 1 and {MAX_PAGE_SIZE}"
            )),
            "case MaxResults={value}: a size outside the modelled range is refused \
             with the one shared sentence, got {answered:?}",
        );
    }
}
