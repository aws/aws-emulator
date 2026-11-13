// SPDX-License-Identifier: CC0-1.0

//! Pagination tokens are scoped to the REGION that issued them, end to end,
//! through every listing this service serves.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suites' `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The one rule under test, asserted over a TABLE of listings crossed with every
//! ordered pair of regions rather than one row: a listing accepts a `NextToken`
//! only from the listing that issued it, and a listing of this REGIONAL service is
//! identified by the region the request addressed as well as by what it enumerates
//! — so a token minted through one region's endpoint names no resume point in
//! another region's listing and is refused, while in the region that issued it the
//! same token still resumes exactly where its page stopped.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::page_size_rule::{listings, page_ids, page_request, Listing, MEMBER_GROUP, TASK_GROUP};
use super::resource_group_lifecycle::{application_group, error_code, Emulator, ACCOUNT};
use super::tag_sync_tasks::role_arn;

/// The regions the rule is asserted over, spanning the forms the modelled region
/// production `[a-z]{2}(-[a-z]+)+-\d{1}` admits: a two-segment region, another
/// two-segment one in a different geography, a three-segment one, and a GovCloud
/// region.
///
/// Source: `smithy_query(query="shape",shape="GroupArn")` on this service's model.
const REGIONS: [&str; 4] = ["us-east-1", "eu-west-1", "ap-southeast-2", "us-gov-east-1"];

/// How many items every listing holds in every region: two, the smallest fixture
/// that makes a listing hand back a `NextToken` at `MaxResults=1` and still leave
/// an item a foreign resume point could silently drop.
const ITEMS: usize = 2;

/// The resource ARNs recorded as memberships, the SAME set in every region — a
/// resource ARN is not a group's identity, so two regions' groups may hold the
/// same members, which is the case where a foreign resume key names a key this
/// region's listing really holds.
fn member_arns() -> Vec<String> {
    (0..ITEMS)
        .map(|index| format!("arn:aws:ec2:us-east-1:{ACCOUNT}:instance/i-{index:017x}"))
        .collect()
}

/// An emulator holding [`ITEMS`] items in every listing of every region, and, per
/// region, the identifiers each listing walks — one entry per row of `listings()`,
/// in that same order.
async fn populated() -> (Emulator, BTreeMap<&'static str, Vec<Vec<String>>>) {
    let emulator = Emulator::new();
    let arns = member_arns();
    let mut expected: BTreeMap<&'static str, Vec<Vec<String>>> = BTreeMap::new();
    for region in REGIONS {
        // The two groups of this region: the one that holds static memberships and
        // grouping statuses, and the one the tag-sync tasks hang off. Both carry the
        // same name in every region, so a group name is no more unique across
        // regions here than it is in the service.
        emulator
            .ok_in(region, "/groups", &application_group(MEMBER_GROUP))
            .await;
        emulator
            .ok_in(region, "/groups", &json!({ "Name": TASK_GROUP }))
            .await;
        emulator
            .ok_in(
                region,
                "/group-resources",
                &json!({ "Group": MEMBER_GROUP, "ResourceArns": arns }),
            )
            .await;

        let mut task_arns: Vec<String> = Vec::new();
        for index in 0..ITEMS {
            let started = emulator
                .ok_in(
                    region,
                    "/start-tag-sync-task",
                    &json!({
                        "Group": TASK_GROUP,
                        "RoleArn": role_arn("ResourceGroupsTagSync"),
                        "TagKey": format!("Key{index:02}"),
                        "TagValue": "scoped",
                    }),
                )
                .await;
            task_arns.push(
                started["TaskArn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {region}: StartTagSyncTask must answer a TaskArn, got {started}")
                    })
                    .to_string(),
            );
        }
        task_arns.sort();

        let mut group_names = vec![MEMBER_GROUP.to_string(), TASK_GROUP.to_string()];
        group_names.sort();
        // One expected walk per entry of `listings()`, in the same order: the
        // region's groups, its one group's memberships, that group's grouping
        // statuses, and the region's tag-sync tasks.
        expected.insert(
            region,
            vec![group_names, arns.clone(), arns.clone(), task_arns],
        );
    }
    (emulator, expected)
}

/// The `message` a refusal carries.
fn message(answered: &Value) -> String {
    answered
        .get("message")
        .or_else(|| answered.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a message, got {answered}"))
        .to_string()
}

/// A page of `listing` in `region`, requiring the modelled 200.
async fn page(
    emulator: &Emulator,
    region: &str,
    listing: &Listing,
    size: Option<i64>,
    token: Option<&str>,
) -> Value {
    let (path, body) = page_request(listing, size, token);
    emulator.ok_in(region, &path, &body).await
}

/// Every listing refuses a `NextToken` issued through ANOTHER region's endpoint,
/// resumes the one its own region issued, and is left listing exactly what it held.
#[tokio::test]
async fn every_listing_refuses_a_token_issued_in_another_region() {
    let (emulator, expected) = populated().await;
    for (index, listing) in listings().iter().enumerate() {
        for issuer in REGIONS {
            let walk = &expected[issuer][index];
            assert_eq!(
                walk.len(),
                ITEMS,
                "case {} in {issuer}: the fixtures must hold {ITEMS} items, got {walk:?}",
                listing.label,
            );

            // The token this region's own listing hands back for its first page.
            let first = page(&emulator, issuer, listing, Some(1), None).await;
            let token = first["NextToken"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {} in {issuer}: a one-item page of {ITEMS} items carries a \
                         NextToken, got {first}",
                        listing.label,
                    )
                })
                .to_string();
            assert_eq!(
                page_ids(listing, &first),
                walk[..1],
                "case {} in {issuer}: the first page must carry this region's first item",
                listing.label,
            );

            // In the region that issued it, it resumes where that page stopped.
            let resumed = page(&emulator, issuer, listing, Some(1), Some(&token)).await;
            assert_eq!(
                page_ids(listing, &resumed),
                walk[1..],
                "case {} in {issuer}: its own token must resume after the first item, \
                 got {resumed}",
                listing.label,
            );

            // In every OTHER region, the same token was issued by no listing of that
            // region, so it is refused rather than honoured as a resume point.
            for resumer in REGIONS.iter().filter(|region| **region != issuer) {
                let case = format!("{} from {issuer} on {resumer}", listing.label);
                let (path, body) = page_request(listing, Some(1), Some(&token));
                let (status, answered) = emulator.call_in(resumer, &path, &body).await;
                assert_eq!(
                    (status, error_code(&answered)),
                    (400, "BadRequestException"),
                    "case {case:?}: expected 400 BadRequestException, got {status} {answered}",
                );
                let reported = message(&answered);
                assert!(
                    reported.contains("NextToken") && reported.contains("not valid"),
                    "case {case:?}: the refusal must say the NextToken is not valid, \
                     got {reported:?}",
                );

                // And the refusal listed nothing and changed nothing: this region's
                // listing still walks every item it held.
                let whole = page(&emulator, resumer, listing, None, None).await;
                assert_eq!(
                    page_ids(listing, &whole),
                    expected[*resumer][index],
                    "case {case:?}: a refused token must leave this region's listing \
                     exactly as it was, got {whole}",
                );
            }
        }
    }
}

/// One row of the codec table: the region and listing a token is minted for, and
/// the resume key it carries.
///
/// The rows differ along what identifies a listing of this service — the region,
/// the listing name, and (for a per-group listing) the group, whose name spans the
/// characters the modelled `CreateGroupName` pattern `^[a-zA-Z0-9_\.-]+$` admits
/// (`smithy_query(query="shape",shape="CreateGroupName")`) — and along the resume
/// keys those listings carry: a group name, a task ARN, a resource ARN.
const CODEC_ROWS: [(&str, &str, &str); 8] = [
    ("us-east-1", "groups", "page-00"),
    ("eu-west-1", "groups", "page-00"),
    ("ap-southeast-2", "groups", "page-00"),
    ("us-gov-east-1", "tag-sync-tasks", "arn:aws:resource-groups:us-gov-east-1:000000000000:group/page-01/01jabcdefghijklmnopqrstuv/tag-sync-task"),
    ("us-east-1", "group-resources/page-00", "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"),
    ("eu-west-1", "group-resources/page-00", "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"),
    ("us-east-1", "grouping-statuses/page-00", "arn:aws:s3:::scoped-bucket"),
    ("us-gov-east-1", "grouping-statuses/page.00_a-b", "arn:aws:s3:::scoped-bucket"),
];

/// The token codec itself, over the table above: a token round-trips through the
/// region and listing it was minted for, and decodes for no other pair — which is
/// the rule the handlers' refusals rest on, asserted here where every pair is
/// reachable.
#[test]
fn a_page_token_decodes_only_for_the_region_and_listing_that_minted_it() {
    use service_com_amazonaws_resourcegroups_ardi::state::{decode_page_token, encode_page_token};

    for (region, listing, key) in CODEC_ROWS {
        let token = encode_page_token(region, listing, key);
        assert_eq!(
            decode_page_token(region, listing, &token).as_deref(),
            Some(key),
            "case ({region}, {listing}): a token must round-trip its resume key",
        );
        for (other_region, other_listing, _) in CODEC_ROWS {
            if (other_region, other_listing) == (region, listing) {
                continue;
            }
            let decoded = decode_page_token(other_region, other_listing, &token);
            assert_eq!(
                decoded, None,
                "case ({region}, {listing}) read as ({other_region}, {other_listing}): \
                 a token of another listing must decode to nothing, got {decoded:?}",
            );
        }
        // A region whose name merely STARTS the issuing one, and one the issuing
        // name starts, each name a different listing too.
        for other_region in [&region[..region.len() - 1], &format!("{region}-1")[..]] {
            let decoded = decode_page_token(other_region, listing, &token);
            assert_eq!(
                decoded, None,
                "case ({region}, {listing}) read as ({other_region}, {listing}): a \
                 region that only shares a prefix is another region, got {decoded:?}",
            );
        }
    }
}
