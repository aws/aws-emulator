// SPDX-License-Identifier: CC0-1.0

//! Per-region isolation of resource-group state, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suites' `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The rule under test, asserted over a TABLE of regions and operations rather than
//! one row: this service is REGIONAL, so a group belongs to the region whose
//! endpoint created it. From that follow the two observable halves asserted below:
//!
//! * a group name is unique only WITHIN one region of one account (modelled on
//!   `CreateGroupInput$Name`: "A resource group name must be unique within each
//!   Amazon Web Services Region in your Amazon Web Services account"), so the same
//!   name in another region creates an independent group whose ARN names that
//!   region;
//! * a group of another region is invisible: every operation that addresses one
//!   group resolves an identifier — a bare name or a group ARN — only among the
//!   groups of the region the request addressed, and a listing enumerates only
//!   those.

use serde_json::{json, Value};

use super::group_tags::pct;
use super::resource_group_lifecycle::{error_code, identifier_names, static_group, Emulator, ACCOUNT};

/// The regions a caller's SigV4 credential scope can name, spanning the forms the
/// modelled `GroupArn` region production `[a-z]{2}(-[a-z]+)+-\d{1}` admits: a
/// two-segment region, a three-segment one, several geographies, and a GovCloud
/// region.
///
/// Source: `smithy_query(query="shape",shape="GroupArn")` on this service's model.
const REGIONS: [&str; 5] = [
    "us-east-1",
    "eu-west-1",
    "ap-southeast-2",
    "eu-central-1",
    "us-gov-east-1",
];

/// The ARN every operation of this service reports for the group `name` in `region`.
fn group_arn(region: &str, name: &str) -> String {
    format!("arn:aws:resource-groups:{region}:{ACCOUNT}:group/{name}")
}

/// Every region in the table is one the modelled ARN pattern admits, so the rows
/// below vary the region along the production the model declares rather than along
/// two spellings of one region.
#[test]
fn every_region_in_the_table_is_shaped_like_an_aws_region() {
    for region in REGIONS {
        let (head, digit) = region
            .rsplit_once('-')
            .unwrap_or_else(|| panic!("case {region:?}: a region name carries a trailing -<digit>"));
        assert!(
            digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit(),
            "case {region:?}: expected a one-digit last segment, got {digit:?}",
        );
        let segments: Vec<&str> = head.split('-').collect();
        assert!(
            segments.len() >= 2
                && segments[0].len() == 2
                && segments
                    .iter()
                    .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase())),
            "case {region:?}: expected [a-z]{{2}}(-[a-z]+)+-\\d, got {segments:?}",
        );
    }
}

/// The names the rule is asserted on, differing along what a group name can be:
/// the characters the modelled `CreateGroupName` pattern `^[a-zA-Z0-9_\.-]+$`
/// admits, and its one-character minimum (`length 1..300`).
///
/// Source: `smithy_query(query="shape",shape="CreateGroupName")`.
const NAMES: [&str; 3] = ["demo", "demo.group_1-2", "g"];

/// The same group name in each region is an independent group: every create
/// succeeds, each group's ARN names its own region, each region reads back its own
/// group, each region's listing holds only its own, and deleting one leaves the
/// others untouched.
#[tokio::test]
async fn the_same_group_name_is_an_independent_group_in_every_region() {
    for name in NAMES {
        let emulator = Emulator::new();

        // One create per region, each carrying a description that names the region,
        // so a later read proves WHICH group answered rather than only that one did.
        for region in REGIONS {
            let description = format!("group in {region}");
            let (status, created) = emulator
                .call_in(
                    region,
                    "/groups",
                    &json!({ "Name": name, "Description": description }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {name:?} in {region}: expected 200, got {status} {created}",
            );
            let expected = group_arn(region, name);
            assert_eq!(
                created["Group"]["GroupArn"].as_str(),
                Some(expected.as_str()),
                "case {name:?} in {region}: expected GroupArn {expected}, got {created}",
            );
        }

        // Each region reads back its OWN group, by name and by its own stored ARN.
        for region in REGIONS {
            let arn = group_arn(region, name);
            let description = format!("group in {region}");
            for identifier in [json!({ "Group": name }), json!({ "Group": arn })] {
                let read = emulator.ok_in(region, "/get-group", &identifier).await;
                assert_eq!(
                    (
                        read["Group"]["GroupArn"].as_str(),
                        read["Group"]["Description"].as_str(),
                    ),
                    (Some(arn.as_str()), Some(description.as_str())),
                    "case {name:?} in {region} ({identifier}): expected the {region} group, \
                     got {read}",
                );
            }
            // And the listing of that region holds exactly its own group.
            let listed = emulator.ok_in(region, "/groups-list", &json!({})).await;
            assert_eq!(
                identifier_names(&listed),
                vec![name.to_string()],
                "case {name:?} in {region}: expected the listing to hold exactly [{name}], \
                 got {listed}",
            );
            let listed_arns: Vec<&str> = listed["GroupIdentifiers"]
                .as_array()
                .expect("GroupIdentifiers is a list")
                .iter()
                .filter_map(|id| id["GroupArn"].as_str())
                .collect();
            assert_eq!(
                listed_arns,
                vec![arn.as_str()],
                "case {name:?} in {region}: a listing must carry only this region's ARNs, \
                 got {listed}",
            );
        }

        // Deleting the group of one region removes that one and no other.
        let (deleted_region, kept) = REGIONS.split_first().expect("the table names a region");
        let deleted = emulator
            .ok_in(deleted_region, "/delete-group", &json!({ "Group": name }))
            .await;
        assert_eq!(
            deleted["Group"]["GroupArn"].as_str(),
            Some(group_arn(deleted_region, name).as_str()),
            "case {name:?}: expected DeleteGroup to report the {deleted_region} group, \
             got {deleted}",
        );
        let (status, gone) = emulator
            .call_in(deleted_region, "/get-group", &json!({ "Group": name }))
            .await;
        assert_eq!(
            (status, error_code(&gone)),
            (404, "NotFoundException"),
            "case {name:?} in {deleted_region}: expected the deleted group to be gone, \
             got {status} {gone}",
        );
        for region in kept {
            let read = emulator.ok_in(region, "/get-group", &json!({ "Group": name })).await;
            assert_eq!(
                read["Group"]["GroupArn"].as_str(),
                Some(group_arn(region, name).as_str()),
                "case {name:?}: deleting in {deleted_region} must leave the {region} group, \
                 got {read}",
            );
        }
    }
}

/// One operation that addresses a single group, as a request addressed to a region
/// that does not hold it: the path, the body (with `{identifier}` substituted) and
/// the refusal it must answer.
struct ForeignCall {
    path: &'static str,
    /// The request body, built from the identifier the caller addresses the group
    /// with — a bare name or a group ARN.
    body: fn(&Value) -> Value,
    /// The status and `__type` the operation answers for a group it cannot resolve.
    /// Every operation here answers this service's one not-found class,
    /// `NotFoundException` (404) — including `ListGroupingStatuses`, whose own error
    /// list does not name the shape and which widens it in, because an identifier
    /// that names no stored group is a resource that does not exist and not a
    /// malformed request.
    refusal: (u16, &'static str),
}

/// Every operation that addresses one group, with the refusal each models.
fn foreign_calls() -> Vec<ForeignCall> {
    vec![
        ForeignCall {
            path: "/get-group",
            body: |id| json!({ "Group": id }),
            refusal: (404, "NotFoundException"),
        },
        ForeignCall {
            path: "/get-group",
            // The deprecated member resolves the same way as `Group`.
            body: |id| json!({ "GroupName": id }),
            refusal: (404, "NotFoundException"),
        },
        ForeignCall {
            path: "/get-group-query",
            body: |id| json!({ "Group": id }),
            refusal: (404, "NotFoundException"),
        },
        ForeignCall {
            path: "/update-group",
            body: |id| json!({ "Group": id, "Description": "Changed from elsewhere" }),
            refusal: (404, "NotFoundException"),
        },
        ForeignCall {
            path: "/group-resources",
            body: |id| {
                json!({
                    "Group": id,
                    "ResourceArns": [format!("arn:aws:ec2:us-east-1:{ACCOUNT}:instance/i-000000000000000b")],
                })
            },
            refusal: (404, "NotFoundException"),
        },
        ForeignCall {
            path: "/list-group-resources",
            body: |id| json!({ "Group": id }),
            refusal: (404, "NotFoundException"),
        },
        ForeignCall {
            path: "/list-grouping-statuses",
            body: |id| json!({ "Group": id }),
            refusal: (404, "NotFoundException"),
        },
        // Last, because a delete that WERE served would take the group with it.
        ForeignCall {
            path: "/delete-group",
            body: |id| json!({ "Group": id }),
            refusal: (404, "NotFoundException"),
        },
    ]
}

/// No operation that addresses one group resolves a group of another region, by
/// name or by that group's own ARN — and the refusals leave the group as it stood.
#[tokio::test]
async fn no_operation_resolves_a_group_of_another_region() {
    let home = "us-east-1";
    let name = "scoped";
    // A second group of the home region, configured so it can hold a static
    // membership (`GroupResources` serves only a configured group, and a group
    // carries either a query or a configuration, never both), so the membership
    // half of the rule is asserted on a group that actually holds one.
    let static_name = "scoped-static";
    let emulator = Emulator::new();
    let member = format!("arn:aws:ec2:{home}:{ACCOUNT}:instance/i-000000000000000a");
    let arn = group_arn(home, name);
    let static_arn = group_arn(home, static_name);

    emulator
        .ok_in(
            home,
            "/groups",
            &json!({
                "Name": name,
                "Description": "Home region group",
                "Tags": { "Stage": "Test" },
                "ResourceQuery": {
                    "Type": "TAG_FILTERS_1_0",
                    "Query": "{\"ResourceTypeFilters\":[\"AWS::AllSupported\"],\"TagFilters\":[]}"
                }
            }),
        )
        .await;
    emulator.ok_in(home, "/groups", &static_group(static_name)).await;
    emulator
        .ok_in(
            home,
            "/group-resources",
            &json!({ "Group": static_name, "ResourceArns": [member.clone()] }),
        )
        .await;
    let before = emulator.ok_in(home, "/get-group", &json!({ "Group": name })).await;

    for region in REGIONS.iter().filter(|region| **region != home) {
        // The identifier forms a caller can address either group with: its bare
        // name, and the group's own stored ARN, which names the home region.
        for identifier in [
            json!(name),
            json!(arn),
            json!(static_name),
            json!(static_arn),
        ] {
            for call in foreign_calls() {
                let body = (call.body)(&identifier);
                let (status, answered) = emulator.call_in(region, call.path, &body).await;
                assert_eq!(
                    (status, error_code(&answered)),
                    call.refusal,
                    "case {identifier} from {region} on {}: expected {:?}, got {status} {answered}",
                    call.path,
                    call.refusal,
                );
            }
        }

        // The tagging operations address the group by ARN alone (the modelled
        // `GroupArnV2`), on their own method and path.
        let tags_path = format!("/resources/{}/tags", pct(&arn));
        for (method, payload) in [
            ("GET", None),
            ("PUT", Some(json!({ "Tags": { "Stage": "Overwritten" } }))),
            ("PATCH", Some(json!({ "Keys": ["Stage"] }))),
        ] {
            let (status, answered) = emulator
                .request_in(region, method, &tags_path, payload.as_ref())
                .await;
            assert_eq!(
                (status, error_code(&answered)),
                (404, "NotFoundException"),
                "case {method} {tags_path} from {region}: expected 404 NotFoundException, \
                 got {status} {answered}",
            );
        }

        // And nothing of the home region shows up in this region's listing.
        let listed = emulator.ok_in(region, "/groups-list", &json!({})).await;
        assert!(
            identifier_names(&listed).is_empty(),
            "case {region:?}: expected an empty listing, got {listed}",
        );
    }
    // Every refusal above left the group exactly as it was: the same projection,
    // the same query, the same tags and the same membership.
    let after = emulator.ok_in(home, "/get-group", &json!({ "Group": name })).await;
    assert_eq!(
        after, before,
        "the refusals must leave the group unchanged, expected {before}, got {after}",
    );
    let query = emulator
        .ok_in(home, "/get-group-query", &json!({ "Group": name }))
        .await;
    assert_eq!(
        query["GroupQuery"]["GroupName"].as_str(),
        Some(name),
        "the refusals must leave the stored query alone, got {query}",
    );
    let tags = emulator
        .request_in(home, "GET", &format!("/resources/{}/tags", pct(&arn)), None)
        .await;
    assert_eq!(
        (tags.0, &tags.1["Tags"]),
        (200, &json!({ "Stage": "Test" })),
        "the refused tagging calls must leave the stored tags alone, got {} {}",
        tags.0,
        tags.1,
    );
    let members = emulator
        .ok_in(home, "/list-group-resources", &json!({ "Group": static_name }))
        .await;
    let listed_members: Vec<&str> = members["Resources"]
        .as_array()
        .expect("Resources is a list")
        .iter()
        .filter_map(|item| item["Identifier"]["ResourceArn"].as_str())
        .collect();
    assert_eq!(
        listed_members,
        vec![member.as_str()],
        "the refusals must leave the membership alone, got {members}",
    );
}

/// A group created with a service `Configuration` is region-scoped the same way:
/// `GetGroupConfiguration` answers only in the region that holds the group.
#[tokio::test]
async fn a_configured_group_is_region_scoped_too() {
    let home = "eu-west-1";
    let name = "scoped-config";
    let emulator = Emulator::new();
    emulator
        .ok_in(
            home,
            "/groups",
            &json!({
                "Name": name,
                "Configuration": [{ "Type": "AWS::ResourceGroups::Generic" }]
            }),
        )
        .await;

    let in_home = emulator
        .ok_in(home, "/get-group-configuration", &json!({ "Group": name }))
        .await;
    assert_eq!(
        in_home["GroupConfiguration"]["Configuration"][0]["Type"].as_str(),
        Some("AWS::ResourceGroups::Generic"),
        "expected the stored configuration in {home}, got {in_home}",
    );

    for region in REGIONS.iter().filter(|region| **region != home) {
        for identifier in [json!(name), json!(group_arn(home, name))] {
            let (status, answered) = emulator
                .call_in(
                    region,
                    "/get-group-configuration",
                    &json!({ "Group": identifier }),
                )
                .await;
            assert_eq!(
                (status, error_code(&answered)),
                (404, "NotFoundException"),
                "case {identifier} from {region}: expected 404 NotFoundException, \
                 got {status} {answered}",
            );
        }
    }
}

/// A region a credential scope names that is NOT shaped like an AWS region falls
/// back to the emulator default region, so two such scopes address the SAME
/// partition of the store — the one `us-east-1` addresses.
#[tokio::test]
async fn a_scope_region_that_is_not_a_region_falls_back_to_one_partition() {
    let emulator = Emulator::new();
    emulator
        .ok_in("nonsense", "/groups", &json!({ "Name": "fallback" }))
        .await;

    // (label, the scope region of the reader)
    let readers = [
        ("the same unshaped scope region", "nonsense"),
        ("another unshaped scope region", "customer-secret-9"),
        ("the emulator default region itself", "us-east-1"),
    ];
    for (label, region) in readers {
        let read = emulator
            .ok_in(region, "/get-group", &json!({ "Group": "fallback" }))
            .await;
        assert_eq!(
            read["Group"]["GroupArn"].as_str(),
            Some(group_arn("us-east-1", "fallback").as_str()),
            "case {label:?} ({region}): expected the default-region group, got {read}",
        );
    }

    // And a reader signing for a region that IS shaped like one sees nothing.
    let (status, answered) = emulator
        .call_in("eu-west-1", "/get-group", &json!({ "Group": "fallback" }))
        .await;
    assert_eq!(
        (status, error_code(&answered)),
        (404, "NotFoundException"),
        "expected 404 NotFoundException from eu-west-1, got {status} {answered}",
    );
}
