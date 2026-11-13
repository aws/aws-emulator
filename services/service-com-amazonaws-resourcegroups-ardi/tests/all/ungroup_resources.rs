// SPDX-License-Identifier: CC0-1.0

//! `UngroupResources`, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the lifecycle suite's `Emulator`), so these
//! exercise the whole restJson1 path: routing, boundary validation, the handler,
//! and serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * a call drops exactly the resources its `ResourceArns` names from the addressed
//!   group's static memberships, leaves every other membership of that group and of
//!   every other group alone, and reports in `Succeeded` exactly the memberships THIS
//!   call dropped — so a resource the group does not hold is reported in neither
//!   `Succeeded` nor `Failed`, and a repeated call removes nothing and reports
//!   nothing;
//! * a removal that happened becomes that resource's last grouping action
//!   (`UNGROUP`, `SUCCESS`), replacing the record its grouping left, while a
//!   resource the call did not remove keeps the record it had;
//! * the group is resolved by name or by its own ARN, within the region the request
//!   addressed, and an identifier that names no group of that region is
//!   `NotFoundException` (404);
//! * a group that holds no static membership at all — one populated from a resource
//!   query, or created with no configuration — is `BadRequestException` (400), as is
//!   a request that violates a modelled rule, and a refused call changes nothing.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::resource_group_lifecycle::{application_group, error_code, Emulator};

/// Resource ARNs spanning the forms the modelled `ResourceArn` pattern admits: a
/// `type/id` resource, a `type:id` resource, a region-less and account-less global
/// resource, another partition, a resource whose type this emulator has no
/// CloudFormation-style name for, and a bare-name resource.
pub(crate) const RESOURCES: [&str; 6] = [
    "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
    "arn:aws:lambda:eu-west-1:000000000000:function:report",
    "arn:aws:s3:::ungroup-probe-bucket",
    "arn:aws-us-gov:dynamodb:us-gov-west-1:000000000000:table/Orders",
    "arn:aws:glacier:us-east-1:000000000000:vaults/records",
    "arn:aws:sqs:us-east-1:000000000000:ungroup-probe-queue",
];

/// A resource ARN of a kind the group under test never holds.
const NEVER_A_MEMBER: &str = "arn:aws:ec2:us-east-1:000000000000:instance/i-00000000000000000";

/// Seconds since the epoch, as the restJson1 wire carries a modelled `timestamp`.
fn epoch_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// `arns` as owned strings, sorted and deduplicated — the form a membership
/// listing and a `Succeeded` list are compared in.
pub(crate) fn sorted<I, S>(arns: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    arns.into_iter()
        .map(|arn| arn.as_ref().to_string())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// Create `name` as a group that holds static memberships, and record `arns` in it.
///
/// The group is created as an APPLICATION group: it both holds static memberships and
/// is the target `ListGroupingStatuses` reports on, which these suites read the
/// recorded actions back through.
///
/// Returns the group's own stored ARN.
pub(crate) async fn group_with_members(
    emulator: &Emulator,
    name: &str,
    arns: &[String],
) -> String {
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

/// The ARNs `ListGroupResources` reports as `name`'s memberships, sorted.
pub(crate) async fn listed_members(emulator: &Emulator, case: &str, name: &str) -> Vec<String> {
    let listed = emulator
        .ok("/list-group-resources", &json!({ "Group": name }))
        .await;
    sorted(
        listed["ResourceIdentifiers"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected ResourceIdentifiers, got {listed}"))
            .iter()
            .map(|identifier| {
                identifier["ResourceArn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a ResourceArn, got {identifier}")
                    })
                    .to_string()
            }),
    )
}

/// The `Succeeded` list of an answer, sorted.
///
/// The member is required to be on the wire: a call that changed no membership
/// reports that as `[]`, so an answer that leaves `Succeeded` out is a failure here
/// rather than another spelling of "no ARN".
pub(crate) fn succeeded(case: &str, answered: &Value) -> Vec<String> {
    sorted(
        answered["Succeeded"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected Succeeded, got {answered}"))
            .iter()
            .map(|arn| {
                arn.as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected an ARN, got {arn}"))
                    .to_string()
            }),
    )
}

/// One row: the memberships a group is created with, the ARNs one call ungroups,
/// and the memberships that must be left.
struct UngroupCase {
    label: String,
    grouped: Vec<String>,
    ungroup: Vec<String>,
    remaining: Vec<String>,
}

/// Every shape of removal the rule covers: one member of several, a few at once,
/// every member, a resource the group never held (alone and beside members that it
/// does hold), the same member named twice in one request, and the modelled
/// maximum of ten ARNs in one call.
fn ungroup_cases() -> Vec<UngroupCase> {
    let all: Vec<String> = RESOURCES.iter().map(|arn| (*arn).to_string()).collect();
    let pick = |indexes: &[usize]| -> Vec<String> {
        indexes.iter().map(|i| all[*i].clone()).collect()
    };
    // Ten distinct ARNs: the most `ResourceArnList` (`length 1..10`) carries.
    let ten: Vec<String> = (0..10)
        .map(|i| format!("arn:aws:ec2:us-east-1:000000000000:instance/i-{i:017x}"))
        .collect();
    vec![
        UngroupCase {
            label: "one member of several".to_string(),
            grouped: all.clone(),
            ungroup: pick(&[0]),
            remaining: pick(&[1, 2, 3, 4, 5]),
        },
        UngroupCase {
            label: "three members of six, in one call".to_string(),
            grouped: all.clone(),
            ungroup: pick(&[0, 2, 4]),
            remaining: pick(&[1, 3, 5]),
        },
        UngroupCase {
            label: "every member the group holds".to_string(),
            grouped: all.clone(),
            ungroup: all.clone(),
            remaining: Vec::new(),
        },
        UngroupCase {
            label: "a resource the group never held".to_string(),
            grouped: pick(&[0, 1]),
            ungroup: vec![NEVER_A_MEMBER.to_string()],
            remaining: pick(&[0, 1]),
        },
        UngroupCase {
            label: "a member and a resource the group never held, in one call".to_string(),
            grouped: pick(&[0, 1]),
            ungroup: vec![all[0].clone(), NEVER_A_MEMBER.to_string()],
            remaining: pick(&[1]),
        },
        UngroupCase {
            label: "the same member named twice in one call".to_string(),
            grouped: pick(&[0, 1]),
            ungroup: vec![all[0].clone(), all[0].clone()],
            remaining: pick(&[1]),
        },
        UngroupCase {
            label: "the modelled maximum of ten ARNs in one call".to_string(),
            grouped: ten.clone(),
            ungroup: ten.clone(),
            remaining: Vec::new(),
        },
        UngroupCase {
            label: "half of ten memberships".to_string(),
            grouped: ten.clone(),
            ungroup: ten[..5].to_vec(),
            remaining: ten[5..].to_vec(),
        },
    ]
}

/// The rule: a call drops exactly the resources it names, reports in `Succeeded`
/// exactly the memberships it dropped, and is idempotent — repeating it leaves the
/// same group behind and reports no removal, because there is none left to make.
#[tokio::test]
async fn ungrouping_drops_exactly_the_named_resources_and_reports_the_removals_it_made() {
    for case in ungroup_cases() {
        let label = case.label.as_str();
        let name = "ungroup-probe";
        let emulator = Emulator::new();
        group_with_members(&emulator, name, &case.grouped).await;

        let expected_remaining = sorted(&case.remaining);
        // What the FIRST call must report: the memberships the group loses, which is
        // the row's own grouped-minus-remaining difference and not the request list.
        // The repeated call removes nothing, so it reports nothing.
        let expected_succeeded = |round: &str| -> Vec<String> {
            if round == "first call" {
                sorted(&case.grouped)
                    .into_iter()
                    .filter(|arn| !expected_remaining.contains(arn))
                    .collect()
            } else {
                Vec::new()
            }
        };
        // Twice over: a removal is idempotent, so the second call leaves the same
        // group behind — and reports no removal, since the first call made them all.
        for round in ["first call", "repeated call"] {
            let case_round = format!("{label} ({round})");
            let (status, answered) = emulator
                .call(
                    "/ungroup-resources",
                    &json!({ "Group": name, "ResourceArns": case.ungroup }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case_round:?}: expected 200, got {status} {answered}",
            );
            let expected = expected_succeeded(round);
            assert_eq!(
                succeeded(&case_round, &answered),
                expected,
                "case {case_round:?}: expected Succeeded to be the memberships this call \
                 dropped, {expected:?}, got {answered}",
            );
            for empty in ["Failed", "Pending"] {
                let list = answered[empty].as_array();
                assert!(
                    list.is_none_or(Vec::is_empty),
                    "case {case_round:?}: a call this emulator applies in full carries no \
                     {empty} entry, got {answered}",
                );
            }
            // Round trip: the memberships the group now holds are read back through
            // an implemented listing, so the removal is persisted state and not the
            // answer's own claim.
            let stored = listed_members(&emulator, &case_round, name).await;
            assert_eq!(
                stored, expected_remaining,
                "case {case_round:?}: expected the memberships {expected_remaining:?} to be \
                 left, got {stored:?}",
            );
        }
    }
}

/// A removal touches only the group it addresses: another group holding the same
/// resources keeps every one of them, and the addressed group can be addressed by
/// its name or by its own stored ARN.
#[tokio::test]
async fn a_removal_touches_only_the_group_it_addresses() {
    let members: Vec<String> = RESOURCES[..3].iter().map(|a| (*a).to_string()).collect();
    for (label, address_by_arn) in [("by name", false), ("by the group ARN", true)] {
        let emulator = Emulator::new();
        let addressed_arn = group_with_members(&emulator, "addressed", &members).await;
        group_with_members(&emulator, "bystander", &members).await;
        let identifier = if address_by_arn {
            addressed_arn.clone()
        } else {
            "addressed".to_string()
        };

        let answered = emulator
            .ok(
                "/ungroup-resources",
                &json!({ "Group": identifier, "ResourceArns": [members[0].clone()] }),
            )
            .await;
        assert_eq!(
            succeeded(label, &answered),
            sorted([members[0].as_str()]),
            "case {label:?}: expected the named ARN in Succeeded, got {answered}",
        );
        let addressed = listed_members(&emulator, label, "addressed").await;
        assert_eq!(
            addressed,
            sorted(&members[1..]),
            "case {label:?}: expected the addressed group to lose exactly one membership, \
             got {addressed:?}",
        );
        let bystander = listed_members(&emulator, label, "bystander").await;
        assert_eq!(
            bystander,
            sorted(&members),
            "case {label:?}: expected another group's memberships to be untouched, \
             got {bystander:?}",
        );
    }
}

/// A removal that happened becomes that resource's last grouping action — `UNGROUP`,
/// `SUCCESS`, timed at the call — replacing the record its grouping left, while a
/// resource the call did not remove keeps the record it had and a resource the group
/// never held gets none.
#[tokio::test]
async fn a_removal_is_reported_as_the_resources_last_grouping_action() {
    let members: Vec<String> = RESOURCES[..3].iter().map(|a| (*a).to_string()).collect();
    let emulator = Emulator::new();
    group_with_members(&emulator, "statuses", &members).await;

    let before = epoch_now();
    emulator
        .ok(
            "/ungroup-resources",
            &json!({
                "Group": "statuses",
                "ResourceArns": [members[1].clone(), NEVER_A_MEMBER.to_string()],
            }),
        )
        .await;
    let after = epoch_now();

    let page = emulator
        .ok("/list-grouping-statuses", &json!({ "Group": "statuses" }))
        .await;
    let items = page["GroupingStatuses"]
        .as_array()
        .unwrap_or_else(|| panic!("expected GroupingStatuses, got {page}"))
        .clone();
    let listed = sorted(items.iter().map(|item| {
        item["ResourceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("a record must carry ResourceArn, got {item}"))
            .to_string()
    }));
    // One record per resource the group holds or held: the ungrouped member keeps
    // its record (now an UNGROUP), and the resource the group never held has none.
    assert_eq!(
        listed,
        sorted(&members),
        "expected one record per resource the group has held, got {page}",
    );

    // (the resource, the action its last recorded grouping action must be)
    let expected = [
        (members[0].as_str(), "GROUP"),
        (members[1].as_str(), "UNGROUP"),
        (members[2].as_str(), "GROUP"),
    ];
    for (arn, action) in expected {
        let item = items
            .iter()
            .find(|item| item["ResourceArn"].as_str() == Some(arn))
            .unwrap_or_else(|| panic!("case {arn:?}: expected a record, got {page}"));
        assert_eq!(
            item["Action"].as_str(),
            Some(action),
            "case {arn:?}: expected Action {action}, got {item}",
        );
        assert_eq!(
            item["Status"].as_str(),
            Some("SUCCESS"),
            "case {arn:?}: an action this emulator applied in full ends in SUCCESS, got {item}",
        );
        if action == "UNGROUP" {
            let updated = item["UpdatedAt"]
                .as_f64()
                .unwrap_or_else(|| panic!("case {arn:?}: expected a wire timestamp, got {item}"));
            assert!(
                (before - 1.0..=after + 1.0).contains(&updated),
                "case {arn:?}: expected UpdatedAt to be the time of the removal (between \
                 {before} and {after}), got {updated}",
            );
        }
    }
}

/// Emptying a group through this operation lifts the deletion protection its own
/// configuration declares for a group that still contains resources: the delete that
/// was refused then succeeds.
#[tokio::test]
async fn emptying_a_protected_group_lets_it_be_deleted() {
    let emulator = Emulator::new();
    // The `deletion-protection` parameter set to `UNLESS_EMPTY` beside a
    // configuration type that holds static memberships (AWS service-configuration
    // vocabulary, as the sibling `delete_group_protection` suite cites).
    emulator
        .ok(
            "/groups",
            &json!({
                "Name": "protected",
                "Configuration": [
                    { "Type": "AWS::EC2::HostManagement" },
                    {
                        "Type": "AWS::ResourceGroups::Generic",
                        "Parameters": [
                            { "Name": "deletion-protection", "Values": ["UNLESS_EMPTY"] }
                        ]
                    }
                ]
            }),
        )
        .await;
    let members: Vec<String> = RESOURCES[..2].iter().map(|a| (*a).to_string()).collect();
    emulator
        .ok(
            "/group-resources",
            &json!({ "Group": "protected", "ResourceArns": members }),
        )
        .await;

    let (status, refused) = emulator
        .call("/delete-group", &json!({ "Group": "protected" }))
        .await;
    assert_eq!(
        (status, error_code(&refused)),
        (400, "BadRequestException"),
        "a protected group that still holds resources must refuse the delete, got \
         {status} {refused}",
    );

    emulator
        .ok(
            "/ungroup-resources",
            &json!({ "Group": "protected", "ResourceArns": members }),
        )
        .await;
    let emptied = listed_members(&emulator, "protected", "protected").await;
    assert!(
        emptied.is_empty(),
        "expected the group to hold nothing after the removal, got {emptied:?}",
    );
    let (status, deleted) = emulator
        .call("/delete-group", &json!({ "Group": "protected" }))
        .await;
    assert_eq!(
        status, 200,
        "an emptied group is no longer protected, so the delete must succeed, got \
         {status} {deleted}",
    );
}

/// An identifier that names no group of the region the request addressed is
/// `NotFoundException` (404), whatever form it takes — including a group of another
/// region, which is another region's resource.
#[tokio::test]
async fn an_identifier_that_names_no_group_of_the_region_is_not_found() {
    let emulator = Emulator::new();
    let members: Vec<String> = RESOURCES[..2].iter().map(|a| (*a).to_string()).collect();
    group_with_members(&emulator, "present", &members).await;
    // A group of another region, addressed through this one: created in us-west-2,
    // so no identifier of it resolves in us-east-1.
    emulator
        .ok_in("us-west-2", "/groups", &application_group("elsewhere"))
        .await;

    for (label, identifier) in [
        ("a name nothing answers to", "absent".to_string()),
        (
            "an ARN for a group that was never created",
            "arn:aws:resource-groups:us-east-1:000000000000:group/absent".to_string(),
        ),
        (
            "the name of a group of another region",
            "elsewhere".to_string(),
        ),
        (
            "the ARN of a group of another region",
            "arn:aws:resource-groups:us-west-2:000000000000:group/elsewhere".to_string(),
        ),
    ] {
        let (status, answered) = emulator
            .call(
                "/ungroup-resources",
                &json!({ "Group": identifier, "ResourceArns": [members[0].clone()] }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answered)),
            (404, "NotFoundException"),
            "case {label:?}: expected 404 NotFoundException, got {status} {answered}",
        );
    }

    // Nothing was removed by any refused call.
    let stored = listed_members(&emulator, "present", "present").await;
    assert_eq!(
        stored,
        sorted(&members),
        "a refused call must remove no membership, got {stored:?}",
    );
}

/// A group that holds no static membership at all, and a request that violates a
/// modelled rule, are both `BadRequestException` (400) — and a refused call changes
/// nothing.
#[tokio::test]
async fn unsupported_groups_and_invalid_requests_are_refused_as_bad_request() {
    let emulator = Emulator::new();
    let members: Vec<String> = RESOURCES[..2].iter().map(|a| (*a).to_string()).collect();
    group_with_members(&emulator, "served", &members).await;
    // Groups that hold no static membership: each kind of query-populated group,
    // one configured with a type no static-membership operation serves, and one
    // created with no configuration at all.
    emulator
        .ok(
            "/groups",
            &json!({
                "Name": "tag-query",
                "ResourceQuery": {
                    "Type": "TAG_FILTERS_1_0",
                    "Query": "{\"TagFilters\":[{\"Key\":\"Stage\",\"Values\":[\"Test\"]}]}"
                }
            }),
        )
        .await;
    emulator
        .ok(
            "/groups",
            &json!({
                "Name": "stack-query",
                "ResourceQuery": {
                    "Type": "CLOUDFORMATION_STACK_1_0",
                    "Query": "{\"StackIdentifier\":\"arn:aws:cloudformation:us-east-1:000000000000:stack/demo/0\"}"
                }
            }),
        )
        .await;
    emulator
        .ok(
            "/groups",
            &json!({ "Name": "generic", "Configuration": [{ "Type": "AWS::ResourceGroups::Generic" }] }),
        )
        .await;
    emulator.ok("/groups", &json!({ "Name": "bare" })).await;

    // (label, the whole request body)
    let cases: Vec<(String, Value)> = vec![
        (
            "a tag-filter query group".to_string(),
            json!({ "Group": "tag-query", "ResourceArns": [members[0].clone()] }),
        ),
        (
            "a CloudFormation-stack query group".to_string(),
            json!({ "Group": "stack-query", "ResourceArns": [members[0].clone()] }),
        ),
        (
            "a group configured with an unsupported type".to_string(),
            json!({ "Group": "generic", "ResourceArns": [members[0].clone()] }),
        ),
        (
            "a group created with no configuration".to_string(),
            json!({ "Group": "bare", "ResourceArns": [members[0].clone()] }),
        ),
        (
            "a ResourceArns entry that is not an ARN".to_string(),
            json!({ "Group": "served", "ResourceArns": ["not-an-arn"] }),
        ),
        (
            "a ResourceArns entry that is a group name".to_string(),
            json!({ "Group": "served", "ResourceArns": ["served"] }),
        ),
        (
            "a member beside a ResourceArns entry that is not an ARN".to_string(),
            json!({ "Group": "served", "ResourceArns": [members[0].clone(), "not-an-arn"] }),
        ),
        (
            "an empty ResourceArns, below the modelled minimum".to_string(),
            json!({ "Group": "served", "ResourceArns": [] }),
        ),
        (
            "eleven ARNs, past the modelled maximum".to_string(),
            json!({
                "Group": "served",
                "ResourceArns": (0..11)
                    .map(|i| format!("arn:aws:ec2:us-east-1:000000000000:instance/i-{i:017x}"))
                    .collect::<Vec<String>>(),
            }),
        ),
        (
            "no ResourceArns at all".to_string(),
            json!({ "Group": "served" }),
        ),
        (
            "no Group at all".to_string(),
            json!({ "ResourceArns": [members[0].clone()] }),
        ),
        (
            "an empty Group, below the modelled minimum length".to_string(),
            json!({ "Group": "", "ResourceArns": [members[0].clone()] }),
        ),
    ];

    for (label, body) in cases {
        let (status, answered) = emulator.call("/ungroup-resources", &body).await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "BadRequestException"),
            "case {label:?}: expected 400 BadRequestException, got {status} {answered}",
        );
    }

    // Every refusal above left the served group's memberships exactly as they were.
    let stored = listed_members(&emulator, "served", "served").await;
    assert_eq!(
        stored,
        sorted(&members),
        "a refused call must remove no membership, got {stored:?}",
    );
}
