// SPDX-License-Identifier: CC0-1.0

//! The identity a notification rule is correlated by, over the wire.
//!
//! A rule's identifier is MINTED BY THE SERVICE for that one rule: it is drawn, not
//! computed — independent of the request, of the creation order and of every other rule —
//! and it is reported as the resource segment of that rule's `Arn` and as the `Id` of the
//! rule's entry in a listing. That one pair is the rule's whole wire identity: an `Id` a
//! listing reports rebuilds the ARN that every operation addressing a rule accepts, in
//! that rule's region and nowhere else.
//!
//! `NotificationRuleId` is `@length 1..40` with no pattern
//! (`smithy_query(query="shape",shape="NotificationRuleId")`), and
//! `NotificationRuleSummary.Arn` carries
//! `^arn:aws[^:\s]*:codestar-notifications:[^:\s]+:\d{12}:notificationrule\/(.*\S)?$`, so
//! what is asserted here is the identity RELATION (drawn per rule, reported identically in
//! both slots, usable as a key) over a table of rules — never one minted value, which is
//! per-run by construction.
//!
//! The last suite drives that relation across the account changing UNDER a listing: the
//! mutating paths that take something away (a rule's own subscription, an address of the
//! whole account, a tag key) and a neighbouring rule arriving and leaving decide what a
//! rule WATCHES, who it NOTIFIES, how it is LABELLED and which rules are listed beside
//! it — never the identity a listing reports for a rule that stays.
//!
//! The harness (`Emulator`, `string`, ...) is the one `notification_rule_lifecycle`
//! builds, and the id-form assertion is `rule_id_identity`'s.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::notification_rule_lifecycle::{string, tags_of, target_addresses_of, Emulator};
use crate::rule_id_identity::{assert_canonical_rule_id, id_of_arn};

/// One rule to seed: the rows differ along what the identity relation generalises over —
/// the region the request is signed for, the resource type, the detail type, the event
/// type count and the target count — so the relation is asserted over a SET of rules.
struct IdentityCase {
    label: &'static str,
    region: &'static str,
    resource: &'static str,
    detail_type: &'static str,
    /// Event type ids out of the catalog `ListEventTypes` publishes
    /// (`crate::state::EVENT_TYPE_CATALOG`).
    event_type_ids: &'static [&'static str],
    targets: &'static [(&'static str, &'static str)],
}

const IDENTITY_CASES: &[IdentityCase] = &[
    IdentityCase {
        label: "codecommit repository, one SNS target",
        region: "us-east-1",
        resource: "arn:aws:codecommit:us-east-1:123456789012:CorrelatedRepo",
        detail_type: "BASIC",
        event_type_ids: &["codecommit-repository-pull-request-created"],
        targets: &[("SNS", "arn:aws:sns:us-east-1:123456789012:correlated-a")],
    },
    IdentityCase {
        label: "codebuild project, no target, two event types",
        region: "eu-west-1",
        resource: "arn:aws:codebuild:eu-west-1:123456789012:project/CorrelatedProject",
        detail_type: "FULL",
        event_type_ids: &[
            "codebuild-project-build-state-failed",
            "codebuild-project-build-state-succeeded",
        ],
        targets: &[],
    },
    IdentityCase {
        label: "codepipeline pipeline, two targets",
        region: "ap-southeast-2",
        resource: "arn:aws:codepipeline:ap-southeast-2:123456789012:CorrelatedPipeline",
        detail_type: "BASIC",
        event_type_ids: &["codepipeline-pipeline-manual-approval-needed"],
        targets: &[
            ("SNS", "arn:aws:sns:ap-southeast-2:123456789012:correlated-b"),
            (
                "AWSChatbotSlack",
                "arn:aws:chatbot:us-east-1:123456789012:chat-configuration/slack-channel/correlated",
            ),
        ],
    },
    IdentityCase {
        label: "codedeploy application, second rule of its region",
        region: "us-east-1",
        resource: "arn:aws:codedeploy:us-east-1:123456789012:application:CorrelatedApp",
        detail_type: "FULL",
        event_type_ids: &["codedeploy-application-deployment-succeeded"],
        targets: &[("SNS", "arn:aws:sns:us-east-1:123456789012:correlated-a")],
    },
    IdentityCase {
        label: "codecommit repository again, same resource as row 1",
        region: "eu-west-1",
        resource: "arn:aws:codecommit:us-east-1:123456789012:CorrelatedRepo",
        detail_type: "BASIC",
        event_type_ids: &["codecommit-repository-branches-and-tags-created"],
        targets: &[],
    },
];

impl IdentityCase {
    fn request(&self, name: &str) -> Value {
        json!({
            "Name": name,
            "Resource": self.resource,
            "DetailType": self.detail_type,
            "EventTypeIds": self.event_type_ids,
            "Targets": self.targets
                .iter()
                .map(|(target_type, address)| json!({
                    "TargetType": target_type,
                    "TargetAddress": address,
                }))
                .collect::<Vec<Value>>(),
        })
    }
}

/// The regions [`IDENTITY_CASES`] spreads its rules over, in the order the rows name them.
fn seeded_regions() -> Vec<&'static str> {
    let mut regions: Vec<&'static str> = Vec::new();
    for case in IDENTITY_CASES {
        if !regions.contains(&case.region) {
            regions.push(case.region);
        }
    }
    regions
}

/// A seeded rule: the request it was created with, and the ARN the create answered.
struct Seeded {
    label: &'static str,
    region: &'static str,
    request: Value,
    arn: String,
}

/// POST a request that must succeed, signed for `region`.
async fn ok_in(emulator: &Emulator, case: &str, region: &str, path: &str, body: &Value) -> Value {
    let (status, parsed) = emulator.post_in(region, path, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {path} in {region}, got {status} with {parsed}"
    );
    parsed
}

/// Create every row of [`IDENTITY_CASES`] in its own region, with `label` prefixing the
/// rule names so two seedings of one engine cannot collide on the per-region name index.
async fn seed(emulator: &Emulator, label: &str) -> Vec<Seeded> {
    let mut seeded = Vec::with_capacity(IDENTITY_CASES.len());
    for (index, case) in IDENTITY_CASES.iter().enumerate() {
        let request = case.request(&format!("{label}-{index}"));
        let created = ok_in(
            emulator,
            case.label,
            case.region,
            "/createNotificationRule",
            &request,
        )
        .await;
        let arn = string(&created, "Arn");
        assert!(
            !arn.is_empty(),
            "case {:?}: create must answer with an ARN, got {created}",
            case.label
        );
        seeded.push(Seeded {
            label: case.label,
            region: case.region,
            request,
            arn,
        });
    }
    seeded
}

/// Every `(Id, Arn)` pair a region's listing reports, following `NextToken` to the end at
/// `max_results` entries a page.
async fn list_pairs_in(
    emulator: &Emulator,
    case: &str,
    region: &str,
    max_results: Option<u32>,
) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request = json!({});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(value) = &token {
            request["NextToken"] = json!(value);
        }
        let listed = ok_in(emulator, case, region, "/listNotificationRules", &request).await;
        for summary in listed
            .get("NotificationRules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            pairs.push((string(&summary, "Id"), string(&summary, "Arn")));
        }
        pages += 1;
        assert!(
            pages <= 64,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
        match listed.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    pairs
}

/// A rule's identifier is DRAWN for that rule, not computed from anything the caller
/// sent: two independent stores answering the very same sequence of create requests mint
/// no identifier in common, and no id is a value its own request carried.
#[tokio::test]
async fn a_rule_id_is_drawn_rather_than_derived_from_the_request() {
    let first_emulator = Emulator::new();
    let second_emulator = Emulator::new();
    let first = seed(&first_emulator, "independence").await;
    let second = seed(&second_emulator, "independence").await;

    let mut ids_per_store: Vec<BTreeSet<String>> = Vec::new();
    for (store, seeded) in [("first", &first), ("second", &second)] {
        let mut ids: BTreeSet<String> = BTreeSet::new();
        for rule in seeded {
            let case = format!("{store} store, {:?}", rule.label);
            let id = id_of_arn(&case, &rule.arn);
            assert_canonical_rule_id(&case, id);
            assert!(
                ids.insert(id.to_string()),
                "case {case:?}: {id:?} was already minted for another rule of this store"
            );
            // Nothing the request carried can be the identifier, so the identifier
            // cannot be read back out of the request either.
            let sent = serde_json::to_string(&rule.request).expect("the request serializes");
            assert!(
                !sent.contains(id),
                "case {case:?}: {id:?} is a value the request already carried ({sent})"
            );
        }
        assert_eq!(
            ids.len(),
            IDENTITY_CASES.len(),
            "case {store:?}: every seeded rule must carry its own id, got {ids:?}"
        );
        ids_per_store.push(ids);
    }

    let shared: Vec<&String> = ids_per_store[0]
        .intersection(&ids_per_store[1])
        .collect();
    assert!(
        shared.is_empty(),
        "case \"the same requests against two stores\": an id must be drawn per rule, not \
         computed from the request, got {shared:?} in both"
    );
}

/// The `Id` a listing reports for a rule rebuilds the ARN that addresses that rule: the
/// reported id is the resource segment of the rule's own ARN, and the ARN re-assembled
/// from it is accepted by every operation that names a rule by ARN.
#[tokio::test]
async fn an_id_from_a_listing_rebuilds_the_arn_that_addresses_the_rule() {
    let emulator = Emulator::new();
    let seeded = seed(&emulator, "correlation").await;

    // Row 1 is the modelled minimum page size, so a rule is correlated across page
    // boundaries too; `None` is a request with no `MaxResults` at all.
    for max_results in [Some(1), None] {
        for region in seeded_regions() {
            let case = format!("{region} at MaxResults={max_results:?}");
            let pairs = list_pairs_in(&emulator, &case, region, max_results).await;
            let wanted: Vec<String> = seeded
                .iter()
                .filter(|rule| rule.region == region)
                .map(|rule| rule.arn.clone())
                .collect();
            assert_eq!(
                pairs.iter().map(|(_, arn)| arn.clone()).collect::<Vec<_>>(),
                wanted,
                "case {case:?}: the listing must name that region's rules, got {pairs:?}"
            );

            for (id, arn) in &pairs {
                assert_canonical_rule_id(&case, id);
                // The ARN a client rebuilds from the reported id, in this region and
                // under the account every emulated ARN carries.
                let rebuilt = ::engine::emulator_arn(
                    "codestar-notifications",
                    region,
                    &format!("notificationrule/{id}"),
                );
                assert_eq!(
                    &rebuilt, arn,
                    "case {case:?}: the reported Id must rebuild the rule's own ARN, got {rebuilt:?} for {arn:?}"
                );

                // Every operation that addresses a rule by ARN resolves the rule the
                // listing named through that rebuilt value.
                let described = ok_in(
                    &emulator,
                    &case,
                    region,
                    "/describeNotificationRule",
                    &json!({ "Arn": rebuilt }),
                )
                .await;
                assert_eq!(
                    string(&described, "Arn"),
                    *arn,
                    "case {case:?}: describe must report the rule the id names, got {described}"
                );
                let subscribed = ok_in(
                    &emulator,
                    &case,
                    region,
                    "/subscribe",
                    &json!({
                        "Arn": rebuilt,
                        "Target": {
                            "TargetType": "SNS",
                            "TargetAddress": format!("arn:aws:sns:{region}:123456789012:by-id-{id}"),
                        },
                    }),
                )
                .await;
                assert_eq!(
                    string(&subscribed, "Arn"),
                    *arn,
                    "case {case:?}: subscribe must report the rule the id names, got {subscribed}"
                );
                ok_in(
                    &emulator,
                    &case,
                    region,
                    "/tagResource",
                    &json!({ "Arn": rebuilt, "Tags": { "correlated": id } }),
                )
                .await;
                let tags = ok_in(
                    &emulator,
                    &case,
                    region,
                    "/listTagsForResource",
                    &json!({ "Arn": rebuilt }),
                )
                .await;
                assert_eq!(
                    tags.get("Tags")
                        .and_then(|tags| tags.get("correlated"))
                        .and_then(Value::as_str),
                    Some(id.as_str()),
                    "case {case:?}: the tag written through the rebuilt ARN must be on that rule, got {tags}"
                );
                ok_in(
                    &emulator,
                    &case,
                    region,
                    "/updateNotificationRule",
                    &json!({ "Arn": rebuilt, "Status": "DISABLED" }),
                )
                .await;
                let described = ok_in(
                    &emulator,
                    &case,
                    region,
                    "/describeNotificationRule",
                    &json!({ "Arn": rebuilt }),
                )
                .await;
                assert_eq!(
                    string(&described, "Status"),
                    "DISABLED",
                    "case {case:?}: the update through the rebuilt ARN must have landed, got {described}"
                );
            }
        }
    }

    // And the id is the key a delete takes: the rule the listing named goes away, the
    // rules beside it stay.
    for region in seeded_regions() {
        let case = format!("deleting by id in {region}");
        let pairs = list_pairs_in(&emulator, &case, region, None).await;
        let (id, arn) = pairs.first().cloned().unwrap_or_else(|| {
            panic!("case {case:?}: the region must hold at least one rule to delete")
        });
        let rebuilt = ::engine::emulator_arn(
            "codestar-notifications",
            region,
            &format!("notificationrule/{id}"),
        );
        let deleted = ok_in(
            &emulator,
            &case,
            region,
            "/deleteNotificationRule",
            &json!({ "Arn": rebuilt }),
        )
        .await;
        assert_eq!(
            string(&deleted, "Arn"),
            arn,
            "case {case:?}: delete must report the rule the id named, got {deleted}"
        );
        let remaining = list_pairs_in(&emulator, &case, region, None).await;
        assert_eq!(
            remaining,
            pairs[1..].to_vec(),
            "case {case:?}: exactly the deleted rule may leave the listing, got {remaining:?}"
        );
    }
}

/// An identifier names a rule in the region the rule was created in and nowhere else: the
/// ARN rebuilt from one region's reported id resolves no rule in another region, and no
/// other region's listing reports that id.
#[tokio::test]
async fn an_id_names_a_rule_only_in_its_own_region() {
    let emulator = Emulator::new();
    seed(&emulator, "regional").await;

    let mut listed: Vec<(&'static str, Vec<(String, String)>)> = Vec::new();
    for region in seeded_regions() {
        let pairs = list_pairs_in(&emulator, region, region, None).await;
        assert!(
            !pairs.is_empty(),
            "case {region:?}: the seeding must have left this region a rule"
        );
        listed.push((region, pairs));
    }

    for (home, pairs) in &listed {
        for (id, arn) in pairs {
            for (other, other_pairs) in &listed {
                let case = format!("{home}'s rule {id} seen from {other}");
                assert_eq!(
                    other_pairs.iter().any(|(listed_id, _)| listed_id == id),
                    home == other,
                    "case {case:?}: only its own region's listing may report the id, got {other_pairs:?}"
                );
                // The id rebuilds an ARN of the region the rule lives in, so the SAME id
                // under another region's ARN names no rule at all.
                let rebuilt = ::engine::emulator_arn(
                    "codestar-notifications",
                    other,
                    &format!("notificationrule/{id}"),
                );
                let (status, parsed) = emulator
                    .post_in(
                        other,
                        "/describeNotificationRule",
                        &json!({ "Arn": rebuilt }),
                    )
                    .await;
                let expected = if home == other { 200 } else { 404 };
                assert_eq!(
                    status, expected,
                    "case {case:?}: expected {expected} for {rebuilt:?}, got {status} with {parsed}"
                );
                if home == other {
                    assert_eq!(
                        string(&parsed, "Arn"),
                        *arn,
                        "case {case:?}: describe must report the rule the id names, got {parsed}"
                    );
                }
            }
        }
    }
}

// --- the account changing under a listing --------------------------------------------
//
// An identifier is drawn for ONE rule when that rule is created and belongs to it for as
// long as the rule exists. Every later call either changes what the rule watches, who it
// notifies and how it is labelled, or adds and removes rules beside it — and none of that
// may re-identify a rule that stays. The rows below are the mutating paths the suites
// above do not drive: the three that take something AWAY (a rule's own subscription, an
// address of the whole account, a tag key) and a neighbouring rule arriving and leaving.

/// The tag key the churn below puts on, and later takes off, every seeded rule.
const CHURN_TAG_KEY: &str = "churn";

/// The address EVERY rule of a region subscribes to, so one account-wide `DeleteTarget`
/// takes a target off all of them at once.
fn shared_address(region: &str) -> String {
    format!("arn:aws:sns:{region}:123456789012:churn-shared")
}

/// The address ONE rule subscribes to, named after the identity that rule is listed under.
fn own_address(region: &str, id: &str) -> String {
    format!("arn:aws:sns:{region}:123456789012:churn-own-{id}")
}

/// The `CreateNotificationRule` body a neighbour of `region`'s rules arrives with.
fn neighbour_request(region: &str) -> Value {
    json!({
        "Name": format!("churn-neighbour-{region}"),
        "Resource": format!("arn:aws:codebuild:{region}:123456789012:project/ChurnNeighbour"),
        "DetailType": "BASIC",
        // An id out of the catalog `ListEventTypes` publishes
        // (`crate::state::EVENT_TYPE_CATALOG`).
        "EventTypeIds": ["codebuild-project-build-state-failed"],
        "Targets": [],
    })
}

/// One later change to the account, applied to every seeded region in turn.
enum Churn {
    /// `Subscribe` one more address per rule, so the row after it has a subscription of
    /// the rule's own to take away.
    SubscribeOwnTarget,
    /// `Unsubscribe` that address again: the rule loses a target it alone held.
    UnsubscribeOwnTarget,
    /// `DeleteTarget` an address every rule of the region subscribes to, with
    /// `ForceUnsubscribeAll` — the account-wide removal, which drops that subscription
    /// from all of them in one call.
    DeleteSharedTarget,
    /// `UntagResource` the key every rule carries.
    UntagEveryRule,
    /// `CreateNotificationRule`: a neighbour arrives beside the rules already listed.
    CreateNeighbour,
    /// `DeleteNotificationRule`: that neighbour leaves again.
    DeleteNeighbour,
}

impl Churn {
    /// How the row is reported in an assertion.
    fn label(&self) -> &'static str {
        match self {
            Churn::SubscribeOwnTarget => "subscribing a target of the rule's own",
            Churn::UnsubscribeOwnTarget => "unsubscribing that target",
            Churn::DeleteSharedTarget => "deleting an address of the whole account",
            Churn::UntagEveryRule => "untagging every rule",
            Churn::CreateNeighbour => "creating a neighbouring rule",
            Churn::DeleteNeighbour => "deleting that neighbour",
        }
    }
}

/// Every later change the identity relation is asserted across, in the order they are
/// applied: the rows differ in WHAT they change (a rule's own subscription, an address of
/// the account, a label, the population of the listing), in whether they add or remove,
/// and in whether they touch the listed rule itself or only the account around it.
const CHURN_ROWS: &[Churn] = &[
    Churn::SubscribeOwnTarget,
    Churn::UnsubscribeOwnTarget,
    Churn::DeleteSharedTarget,
    Churn::UntagEveryRule,
    Churn::CreateNeighbour,
    Churn::DeleteNeighbour,
];

/// Assert, for every rule of `pairs`, whether the address `address` names for it is among
/// the targets that rule's describe reports.
async fn assert_subscribed(
    emulator: &Emulator,
    case: &str,
    region: &str,
    pairs: &[(String, String)],
    address: impl Fn(&str) -> String,
    subscribed: bool,
) {
    for (id, arn) in pairs {
        let described = ok_in(
            emulator,
            case,
            region,
            "/describeNotificationRule",
            &json!({ "Arn": arn }),
        )
        .await;
        let addresses = target_addresses_of(&described);
        let wanted = address(id);
        assert_eq!(
            addresses.contains(&wanted),
            subscribed,
            "case {case:?}: {wanted:?} must{} be a target of {arn}, got {addresses:?}",
            if subscribed { "" } else { " no longer" }
        );
    }
}

/// RULE: a notification rule's identifier is drawn for that rule when it is created and
/// is what every listing reports for it — as `NotificationRuleSummary.Id`, byte-for-byte
/// the resource segment of that same entry's `Arn` — for as long as the rule exists. A
/// later call decides what the rule watches, who it notifies, how it is labelled and
/// WHICH rules a listing names; it never re-identifies a rule the listing still names,
/// and a rule that leaves the account takes its own identity and no other with it.
#[tokio::test]
async fn a_later_change_to_the_account_never_re_identifies_a_listed_rule() {
    let emulator = Emulator::new();
    let seeded = seed(&emulator, "churn").await;

    // What each region's listing reports before any change: the pairs the seeding minted,
    // in creation order.
    let mut expected: Vec<(&'static str, Vec<(String, String)>)> = Vec::new();
    for region in seeded_regions() {
        let case = format!("{region} before any change");
        let pairs = list_pairs_in(&emulator, &case, region, None).await;
        assert_eq!(
            pairs.len(),
            seeded.iter().filter(|rule| rule.region == region).count(),
            "case {case:?}: the seeding must leave this region its own rules, got {pairs:?}"
        );
        expected.push((region, pairs));
    }

    // The state the removal rows take away: an address every rule of the region
    // subscribes to, and a tag key every rule carries.
    for (region, pairs) in &expected {
        let case = format!("seeding the removable state of {region}");
        for (id, arn) in pairs {
            ok_in(
                &emulator,
                &case,
                region,
                "/subscribe",
                &json!({
                    "Arn": arn,
                    "Target": {
                        "TargetType": "SNS",
                        "TargetAddress": shared_address(region),
                    },
                }),
            )
            .await;
            ok_in(
                &emulator,
                &case,
                region,
                "/tagResource",
                &json!({
                    "Arn": arn,
                    "Tags": Value::Object(
                        [(CHURN_TAG_KEY.to_string(), json!(id))].into_iter().collect(),
                    ),
                }),
            )
            .await;
        }
    }

    for churn in CHURN_ROWS {
        for (region, pairs) in expected.iter_mut() {
            let region = *region;
            let case = format!("{} in {region}", churn.label());
            let listed = pairs.clone();

            match churn {
                Churn::SubscribeOwnTarget => {
                    for (id, arn) in &listed {
                        ok_in(
                            &emulator,
                            &case,
                            region,
                            "/subscribe",
                            &json!({
                                "Arn": arn,
                                "Target": {
                                    "TargetType": "SNS",
                                    "TargetAddress": own_address(region, id),
                                },
                            }),
                        )
                        .await;
                    }
                    assert_subscribed(
                        &emulator,
                        &case,
                        region,
                        &listed,
                        |id| own_address(region, id),
                        true,
                    )
                    .await;
                }
                Churn::UnsubscribeOwnTarget => {
                    for (id, arn) in &listed {
                        ok_in(
                            &emulator,
                            &case,
                            region,
                            "/unsubscribe",
                            &json!({ "Arn": arn, "TargetAddress": own_address(region, id) }),
                        )
                        .await;
                    }
                    assert_subscribed(
                        &emulator,
                        &case,
                        region,
                        &listed,
                        |id| own_address(region, id),
                        false,
                    )
                    .await;
                }
                Churn::DeleteSharedTarget => {
                    ok_in(
                        &emulator,
                        &case,
                        region,
                        "/deleteTarget",
                        &json!({
                            "TargetAddress": shared_address(region),
                            "ForceUnsubscribeAll": true,
                        }),
                    )
                    .await;
                    assert_subscribed(
                        &emulator,
                        &case,
                        region,
                        &listed,
                        |_| shared_address(region),
                        false,
                    )
                    .await;
                }
                Churn::UntagEveryRule => {
                    for (_, arn) in &listed {
                        ok_in(
                            &emulator,
                            &case,
                            region,
                            &crate::list_tags_for_resource::untag_path(arn, &[CHURN_TAG_KEY]),
                            &json!({}),
                        )
                        .await;
                        let tags = ok_in(
                            &emulator,
                            &case,
                            region,
                            "/listTagsForResource",
                            &json!({ "Arn": arn }),
                        )
                        .await;
                        assert!(
                            !tags_of(&tags).contains_key(CHURN_TAG_KEY),
                            "case {case:?}: {CHURN_TAG_KEY:?} must be off {arn}, got {tags}"
                        );
                    }
                }
                Churn::CreateNeighbour => {
                    let created = ok_in(
                        &emulator,
                        &case,
                        region,
                        "/createNotificationRule",
                        &neighbour_request(region),
                    )
                    .await;
                    let arn = string(&created, "Arn");
                    let id = id_of_arn(&case, &arn).to_string();
                    assert_canonical_rule_id(&case, &id);
                    assert!(
                        !listed.iter().any(|(held, _)| held == &id),
                        "case {case:?}: a new rule must be drawn an identity no listed rule \
                         holds, got {id:?} among {listed:?}"
                    );
                    pairs.push((id, arn));
                }
                Churn::DeleteNeighbour => {
                    let (id, arn) = pairs.pop().unwrap_or_else(|| {
                        panic!("case {case:?}: the row before must have left a neighbour")
                    });
                    let deleted = ok_in(
                        &emulator,
                        &case,
                        region,
                        "/deleteNotificationRule",
                        &json!({ "Arn": arn }),
                    )
                    .await;
                    assert_eq!(
                        string(&deleted, "Arn"),
                        arn,
                        "case {case:?}: delete must report the rule it removed, got {deleted}"
                    );
                    let remaining = list_pairs_in(&emulator, &case, region, None).await;
                    assert!(
                        !remaining.iter().any(|(held, _)| held == &id),
                        "case {case:?}: the deleted rule's identity {id:?} must leave the \
                         listing, got {remaining:?}"
                    );
                }
            }

            // The identity the listing reports after the change, read unfiltered and then
            // one entry at a time: the same pairs, in the same order, each `Id` still the
            // resource segment of its own entry's `Arn`.
            for max_results in [None, Some(1)] {
                let paged = format!("{case} at MaxResults={max_results:?}");
                let after = list_pairs_in(&emulator, &paged, region, max_results).await;
                assert_eq!(
                    &after,
                    &*pairs,
                    "case {paged:?}: the change may only decide WHICH rules are listed, got \
                     {after:?}"
                );
                for (id, arn) in &after {
                    assert_canonical_rule_id(&paged, id);
                    assert_eq!(
                        id_of_arn(&paged, arn),
                        id.as_str(),
                        "case {paged:?}: an entry's Id must be its own Arn's resource segment, \
                         got {id:?} for {arn:?}"
                    );
                }
            }
        }
    }
}
