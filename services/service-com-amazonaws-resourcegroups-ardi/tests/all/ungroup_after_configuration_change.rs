// SPDX-License-Identifier: CC0-1.0

//! Removing static memberships after the group's service configuration changed.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the lifecycle suite's `Emulator`), so these
//! exercise the whole restJson1 path: routing, boundary validation, the handler,
//! and serialization.
//!
//! The rule under test, asserted over a TABLE of configuration changes rather than
//! one row: `UngroupResources` "works only with static groups that you populated
//! using the GroupResources operation"
//! (`smithy_query(query="doc",shape="UngroupResources")`), and a group so populated
//! stays one however its service configuration is replaced afterwards — a put
//! replaces the configuration and carries the memberships nowhere, so every
//! membership the group holds stays removable, while a group that was never
//! populated that way keeps the documented refusal.
//!
//! Adding a membership is the other side of it, and is NOT the same rule: that
//! operation documents the catalog of groups it can be used with
//! (`smithy_query(query="doc",shape="GroupResources")`), so a group whose
//! configuration no longer declares one of those types still refuses an addition
//! while it serves a removal.

use serde_json::{json, Value};

use super::put_group_configuration::make_update_due;
use super::resource_group_lifecycle::{error_code, Emulator};
use super::ungroup_resources::{group_with_members, listed_members, sorted, succeeded, RESOURCES};

/// One row: the configuration the group is created with, the configuration a later
/// put replaces it with, and how many of the group's memberships one call removes.
struct StrandCase {
    label: &'static str,
    /// The `Configuration` the group is created with — one of the types a
    /// static-membership operation serves, so `GroupResources` populates it.
    created: Value,
    /// The `Configuration` the put carries, or `None` for a put that omits the
    /// member altogether (which leaves the group with no configuration).
    replacement: Option<Value>,
    /// How many memberships the group is given.
    members: usize,
    /// How many of them the first removal names.
    ungroup: usize,
    /// Whether the replacement configuration still declares a type
    /// `GroupResources` can be used with — which is what decides whether an
    /// ADDITION is served afterwards.
    replacement_serves_grouping: bool,
}

/// Every configuration change the rule is asserted over. The rows differ along the
/// NUMBER of configuration items the put carries (none at all, one, two), whether
/// the item carries parameters, which type the group was created with, which type
/// the put leaves behind (one no static-membership operation serves, none at all,
/// and another one that does), and how many of the group's memberships one call
/// removes (one of several, some, all).
///
/// The configuration item types and parameter names are AWS service-configuration
/// vocabulary from the "Service configurations for resource groups" reference,
/// which is what `GroupConfigurationItem$Type` and
/// `GroupConfigurationParameter$Name` are documented against; the types a
/// static-membership operation serves are the three `GroupResources` documents
/// (`smithy_query(query="doc",shape="GroupResources")`), and
/// `GroupConfigurationList` is 0..2 long
/// (`smithy_query(query="shape",shape="GroupConfigurationList")`).
fn strand_cases() -> Vec<StrandCase> {
    vec![
        StrandCase {
            label: "a put leaving one type no static-membership operation serves",
            created: json!([{ "Type": "AWS::EC2::HostManagement" }]),
            replacement: Some(json!([{ "Type": "AWS::ResourceGroups::Generic" }])),
            members: 3,
            ungroup: 1,
            replacement_serves_grouping: false,
        },
        StrandCase {
            label: "a put carrying an empty configuration list, which configures nothing",
            created: json!([{ "Type": "AWS::EC2::CapacityReservationPool" }]),
            replacement: Some(json!([])),
            members: 2,
            ungroup: 2,
            replacement_serves_grouping: false,
        },
        StrandCase {
            label: "a put with no Configuration member at all",
            created: json!([{ "Type": "AWS::ResourceGroups::ApplicationGroup" }]),
            replacement: None,
            members: 1,
            ungroup: 1,
            replacement_serves_grouping: false,
        },
        StrandCase {
            label: "a put leaving a type that carries parameters but serves no membership",
            created: json!([{ "Type": "AWS::EC2::HostManagement" }]),
            replacement: Some(json!([{
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [{ "Name": "deletion-protection", "Values": ["DISABLED"] }]
            }])),
            members: 4,
            ungroup: 2,
            replacement_serves_grouping: false,
        },
        StrandCase {
            label: "a put leaving two items, neither serving static membership",
            created: json!([{ "Type": "AWS::EC2::CapacityReservationPool" }]),
            replacement: Some(json!([
                { "Type": "AWS::ResourceGroups::Generic" },
                { "Type": "AWS::NetworkFirewall::RuleGroup" }
            ])),
            members: 6,
            ungroup: 5,
            replacement_serves_grouping: false,
        },
        StrandCase {
            label: "a put replacing one static-membership type with another",
            created: json!([{ "Type": "AWS::EC2::HostManagement" }]),
            replacement: Some(json!([{ "Type": "AWS::ResourceGroups::ApplicationGroup" }])),
            members: 3,
            ungroup: 3,
            replacement_serves_grouping: true,
        },
    ]
}

/// `n` resource ARNs, taken from the forms the modelled `ResourceArn` pattern
/// admits that the sibling suite spans.
fn members(n: usize) -> Vec<String> {
    RESOURCES[..n].iter().map(|arn| (*arn).to_string()).collect()
}

/// Accept `configuration` as `name`'s new service configuration and let the span
/// the put started elapse, so the configuration it carried is the one in effect.
async fn put_configuration_in_effect(
    emulator: &Emulator,
    case: &str,
    name: &str,
    configuration: Option<&Value>,
) {
    let mut request = json!({ "Group": name });
    if let Some(items) = configuration {
        request["Configuration"] = items.clone();
    }
    let (status, answered) = emulator.call("/put-group-configuration", &request).await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected the put to be accepted with 202, got {status} {answered}",
    );
    // The update falls due a span after the put, and the stored instant is what the
    // handlers read: moving it reaches the state a caller reaches by waiting.
    make_update_due(emulator, name);
}

/// The rule: a group populated through `GroupResources` can have its memberships
/// removed whatever service configuration is in effect when the removal arrives —
/// so a put that drops the configuration the group was populated under strands
/// nothing, and the removal is still idempotent.
#[tokio::test]
async fn memberships_stay_removable_after_the_configuration_that_served_them_is_replaced() {
    for case in strand_cases() {
        let label = case.label;
        let name = "stranded";
        let emulator = Emulator::new();
        let held = members(case.members);
        emulator
            .ok(
                "/groups",
                &json!({ "Name": name, "Configuration": case.created }),
            )
            .await;
        for chunk in held.chunks(10) {
            emulator
                .ok(
                    "/group-resources",
                    &json!({ "Group": name, "ResourceArns": chunk }),
                )
                .await;
        }

        put_configuration_in_effect(&emulator, label, name, case.replacement.as_ref()).await;

        let named = &held[..case.ungroup];
        let expected_remaining = sorted(&held[case.ungroup..]);
        // Twice over: the removal is idempotent, so the repeated call leaves the same
        // group behind — and reports no removal, since the first call made them all —
        // including once the group holds nothing at all.
        for round in ["first call", "repeated call"] {
            let case_round = format!("{label} ({round})");
            let (status, answered) = emulator
                .call(
                    "/ungroup-resources",
                    &json!({ "Group": name, "ResourceArns": named }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case_round:?}: a group populated using GroupResources must still \
                 serve a removal, got {status} {answered}",
            );
            assert_eq!(
                succeeded(&case_round, &answered),
                if round == "first call" {
                    sorted(named)
                } else {
                    // The first call dropped every named membership, so the repeated
                    // one removes nothing and reports no removal.
                    Vec::new()
                },
                "case {case_round:?}: expected Succeeded to be the memberships this call \
                 dropped, got {answered}",
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

        // Whatever the row left behind, the group can be emptied: no membership it
        // holds is beyond removal.
        if case.ungroup < case.members {
            let rest = &held[case.ungroup..];
            emulator
                .ok(
                    "/ungroup-resources",
                    &json!({ "Group": name, "ResourceArns": rest }),
                )
                .await;
        }
        let emptied = listed_members(&emulator, label, name).await;
        assert!(
            emptied.is_empty(),
            "case {label:?}: expected every membership to be removable, got {emptied:?}",
        );

        // The catalog rule the ADDITION documents is untouched: a group whose
        // configuration no longer declares one of the types `GroupResources` serves
        // refuses an addition, even though it served the removals above.
        let (status, answered) = emulator
            .call(
                "/group-resources",
                &json!({ "Group": name, "ResourceArns": [RESOURCES[0]] }),
            )
            .await;
        if case.replacement_serves_grouping {
            assert_eq!(
                status, 200,
                "case {label:?}: a configuration that still declares a supported type must \
                 serve an addition, got {status} {answered}",
            );
        } else {
            assert_eq!(
                (status, error_code(&answered)),
                (400, "BadRequestException"),
                "case {label:?}: expected an addition to a group outside the documented \
                 catalog to be refused, got {status} {answered}",
            );
        }
    }
}

/// A group that was NEVER populated through `GroupResources` keeps the documented
/// refusal once its configuration no longer declares a type a static-membership
/// operation serves: there is no membership it holds and none it ever held.
#[tokio::test]
async fn a_group_that_was_never_grouped_is_still_refused_when_its_configuration_drops_the_type() {
    for (label, replacement) in [
        (
            "a type no static-membership operation serves",
            Some(json!([{ "Type": "AWS::ResourceGroups::Generic" }])),
        ),
        ("an empty configuration list", Some(json!([]))),
        ("no Configuration member at all", None),
    ] {
        let emulator = Emulator::new();
        let name = "never-grouped";
        emulator
            .ok(
                "/groups",
                &json!({
                    "Name": name,
                    "Configuration": [{ "Type": "AWS::EC2::HostManagement" }]
                }),
            )
            .await;
        put_configuration_in_effect(&emulator, label, name, replacement.as_ref()).await;

        let (status, answered) = emulator
            .call(
                "/ungroup-resources",
                &json!({ "Group": name, "ResourceArns": [RESOURCES[0]] }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "BadRequestException"),
            "case {label:?}: expected 400 BadRequestException for a group that holds and \
             held no static membership, got {status} {answered}",
        );
    }
}

/// The removal that empties a group whose configuration was replaced is the one the
/// deletion-protection parameter is judged against: a group whose protection came
/// with the configuration a put dropped is deletable, and one whose put ADDED the
/// protection is refused until this operation empties it.
#[tokio::test]
async fn a_put_that_adds_deletion_protection_is_lifted_by_emptying_the_group() {
    let emulator = Emulator::new();
    let name = "protected-by-put";
    let held = members(2);
    let group_arn = group_with_members(&emulator, name, &held).await;
    assert!(
        group_arn.contains(&format!("group/{name}")),
        "expected the created group's ARN to name it in its resource segment, got {group_arn:?}",
    );
    // The put leaves a configuration that serves no static membership AND protects a
    // group that still contains resources.
    put_configuration_in_effect(
        &emulator,
        "protection added by a put",
        name,
        Some(&json!([{
            "Type": "AWS::ResourceGroups::Generic",
            "Parameters": [{ "Name": "deletion-protection", "Values": ["UNLESS_EMPTY"] }]
        }])),
    )
    .await;

    let (status, refused) = emulator
        .call("/delete-group", &json!({ "Group": name }))
        .await;
    assert_eq!(
        (status, error_code(&refused)),
        (400, "BadRequestException"),
        "a protected group that still holds resources must refuse the delete, got \
         {status} {refused}",
    );

    // The only call that drops a membership must still serve this group, or the
    // protection it now carries could never be lifted.
    emulator
        .ok(
            "/ungroup-resources",
            &json!({ "Group": name, "ResourceArns": held }),
        )
        .await;
    let emptied = listed_members(&emulator, "protection added by a put", name).await;
    assert!(
        emptied.is_empty(),
        "expected the group to hold nothing after the removal, got {emptied:?}",
    );
    let (status, deleted) = emulator
        .call("/delete-group", &json!({ "Group": name }))
        .await;
    assert_eq!(
        status, 200,
        "an emptied group is no longer protected, so the delete must succeed, got \
         {status} {deleted}",
    );
}
