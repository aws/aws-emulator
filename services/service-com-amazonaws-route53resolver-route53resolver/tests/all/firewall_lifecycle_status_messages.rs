// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the lifecycle state the DNS Firewall domain-list,
//! rule-group and rule-group-ASSOCIATION projections report: the pair of
//! `Status` and `StatusMessage` `CreateFirewallDomainList`,
//! `GetFirewallDomainList`, `UpdateFirewallDomains`, `DeleteFirewallDomainList`,
//! `CreateFirewallRuleGroup`, `GetFirewallRuleGroup`, `DeleteFirewallRuleGroup`,
//! `AssociateFirewallRuleGroup`, `GetFirewallRuleGroupAssociation`,
//! `ListFirewallRuleGroupAssociations`, `UpdateFirewallRuleGroupAssociation` and
//! `DisassociateFirewallRuleGroup` answer with.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! shared [`super::harness`], so what is asserted is the HTTP behaviour and not
//! a handler called directly.
//!
//! What these cases pin is the rule the slice implements: a domain list, a rule
//! group and an association each stand in a lifecycle step, and every projection
//! of one reports that step BOTH as its `Status` and as the sentence the step
//! states in `StatusMessage` — the step it LAST reached, so a resource a caller
//! has written states the step that change reached and not the one its create
//! did; a change the service applies inside the call it was asked for leaves the
//! resource in its settled step, so no response reports a step that is still in
//! flight for work that is already done. Putting an association in place is
//! asynchronous, so there the step the caller is shown moves with the clock, and
//! the sentence moves with the step.

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::ops::types::{
    FIREWALL_DOMAIN_LIST_STATUS_VALUES, FIREWALL_DOMAIN_UPDATE_OPERATION_VALUES,
    FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_VALUES, FIREWALL_RULE_GROUP_STATUS_VALUES,
};
use service_com_amazonaws_route53resolver_route53resolver::state::FIREWALL_DOMAIN_OPERATION_DENY;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{emulator, expect_error, ok, settled_association, string_member, Emulator};

/// The sentence DNS Firewall states for a domain list that stands in its
/// created, settled step, the one it states once a write of its domains has
/// landed, and the one it states once a delete has been accepted for the list.
/// All three are fixed text for the step, with no part of the request in them.
const DOMAIN_LIST_CREATED_MESSAGE: &str = "Created Firewall Domain List";
const DOMAIN_LIST_CHANGED_MESSAGE: &str = "Finished domain list update";
const DOMAIN_LIST_DELETING_MESSAGE: &str = "Deleting the Firewall Domain List";

/// The sentence DNS Firewall states for a rule group that stands in its
/// created, settled step, and the one it states once a write of its rules has
/// landed. A delete of the group reports the step the group stood in, so its
/// answer carries whichever of the two the group had reached.
const RULE_GROUP_CREATED_MESSAGE: &str = "Created Firewall Rule Group";
const RULE_GROUP_CHANGED_MESSAGE: &str = "Finished rule group update";

/// The sentence DNS Firewall states while the rule-group-to-VPC association a
/// caller just asked for is still being put in place, the one it states while a
/// change to an association that already stood is still settling, the one it
/// states once the change in flight has finished, and the one it states for an
/// association a disassociate has been accepted for. All four are fixed text for
/// the step, with no part of the request in them.
const ASSOCIATION_CREATING_MESSAGE: &str = "Creating Firewall Rule Group Association";
const ASSOCIATION_CHANGING_MESSAGE: &str = "Updating Firewall Rule Group Association";
const ASSOCIATION_SETTLED_MESSAGE: &str = "Finished rule group association update";
const ASSOCIATION_DELETING_MESSAGE: &str = "Deleting Firewall Rule Group Association";

/// The `FirewallRuleGroupAssociationStatus` an association reports while a
/// change to it is still settling, from the model's enum
/// (`smithy_query(query="shape",shape="FirewallRuleGroupAssociationStatus")`,
/// re-exported as the catalog the cases check against).
const IN_FLIGHT: &str = "UPDATING";

/// The settled `FirewallDomainListStatus`/`FirewallRuleGroupStatus` both
/// families report once nothing is in flight, out of the model's enums
/// (`smithy_query(query="shape",shape="FirewallDomainListStatus,FirewallRuleGroupStatus")`,
/// re-exported as the `*_STATUS_VALUES` catalogs this suite checks against).
const SETTLED: &str = "COMPLETE";

/// The step a domain list stands in once a delete has been accepted for it.
const DELETING: &str = "DELETING";

/// `StatusMessage`'s modelled maximum length, from the member's `length` trait
/// (0..255) on both `FirewallDomainList$StatusMessage` and
/// `FirewallRuleGroup$StatusMessage`.
const STATUS_MESSAGE_MAX: usize = 255;

/// One row of the create table: the name the case goes by, and what the call
/// hands over.
struct ResourceCase {
    case: &'static str,
    request: Value,
}

/// The rows both families are driven over: the shapes a `Name` the model's
/// pattern `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$` admits takes (letters, digits
/// beside letters, underscore, hyphen, apostrophe, space, the published
/// 64-character maximum and the one-character minimum), with and without
/// `Tags`, and `CreatorRequestId`s of different forms — the inputs the rule has
/// to answer the same way, because the step a resource stands in is not
/// something the request chooses.
fn resource_cases() -> Vec<ResourceCase> {
    vec![
        ResourceCase {
            case: "letters only, no Tags",
            request: json!({ "CreatorRequestId": "req-letters", "Name": "lifecycle-plain" }),
        },
        ResourceCase {
            case: "digits beside letters and underscores, one tag",
            request: json!({
                "CreatorRequestId": "req-mixed-2",
                "Name": "lifecycle_2-v3",
                "Tags": [{ "Key": "team", "Value": "net" }],
            }),
        },
        ResourceCase {
            case: "spaces and an apostrophe, several tags",
            request: json!({
                "CreatorRequestId": "2026-10-29T23:44:50Z",
                "Name": "ops' lifecycle list",
                "Tags": [
                    { "Key": "team", "Value": "net" },
                    { "Key": "env", "Value": "prod" },
                ],
            }),
        },
        ResourceCase {
            case: "one character, an empty tag list",
            request: json!({ "CreatorRequestId": "r", "Name": "z", "Tags": [] }),
        },
        ResourceCase {
            case: "the longest Name the model allows",
            request: json!({ "CreatorRequestId": "req-longest", "Name": "n".repeat(64) }),
        },
    ]
}

/// Assert the projection `resource` reports `status` and the sentence that step
/// states, within the member's modelled length, and that the sentence quotes no
/// part of the request.
fn assert_step(resource: &Value, status: &str, message: &str, statuses: &[&str], case: &str) {
    let reported = string_member(resource, "Status");
    assert_eq!(
        reported, status,
        "case {case:?}: expected Status {status:?}, got {resource}"
    );
    assert!(
        statuses.contains(&reported.as_str()),
        "case {case:?}: Status must be one of the modelled {statuses:?}, got {reported:?}"
    );
    let reported_message = string_member(resource, "StatusMessage");
    assert_eq!(
        reported_message, message,
        "case {case:?}: expected StatusMessage {message:?}, got {resource}"
    );
    assert!(
        !reported_message.is_empty() && reported_message.len() <= STATUS_MESSAGE_MAX,
        "case {case:?}: StatusMessage must satisfy its modelled length 0..{STATUS_MESSAGE_MAX}, got {reported_message:?}"
    );
    // The step is the service's, so its sentence carries nothing the caller
    // chose: not the resource's own name, nor the request id, nor the minted id.
    // A value of one or two characters is skipped — it can sit inside any
    // English sentence by chance, so it identifies nothing.
    for member in ["Name", "CreatorRequestId", "Id"] {
        let value = string_member(resource, member);
        assert!(
            value.len() < 3 || !reported_message.contains(&value),
            "case {case:?}: StatusMessage must not quote {member} ({value:?}), got {reported_message:?}"
        );
    }
}

/// The `FirewallDomainList` of a domain-list response.
fn domain_list(body: &Value) -> Value {
    body.get("FirewallDomainList")
        .expect("the response carries a FirewallDomainList")
        .clone()
}

/// The `FirewallRuleGroup` of a rule-group response.
fn rule_group(body: &Value) -> Value {
    body.get("FirewallRuleGroup")
        .expect("the response carries a FirewallRuleGroup")
        .clone()
}

/// Read one domain list back over the wire.
async fn read_list(router: &MultiServiceRouter, id: &str) -> Value {
    domain_list(
        &ok(
            router,
            "GetFirewallDomainList",
            json!({ "FirewallDomainListId": id }),
        )
        .await,
    )
}

/// Every projection of a domain list — the create, the read, each `Operation`
/// of an update, and the delete — reports the step the list stands in as both
/// `Status` and `StatusMessage`.
///
/// The update rows are the values the model's `FirewallDomainUpdateOperation`
/// declares for a domain list the account defined (re-exported as
/// `FIREWALL_DOMAIN_UPDATE_OPERATION_VALUES`, less the one its doc restricts to
/// "an AutoAllowlist rule's managed domain list only"), each
/// carrying a different number of domains, so the settled step is asserted for
/// a list that grows, one that shrinks and one that is rewritten.
#[tokio::test]
async fn every_domain_list_projection_states_the_step_the_list_stands_in() {
    let router = emulator();

    for ResourceCase { case, request } in resource_cases() {
        let made = domain_list(&ok(&router, "CreateFirewallDomainList", request).await);
        let id = string_member(&made, "Id");

        // A list is "created empty", which needs no import, so the create
        // answers the settled step.
        assert_step(
            &made,
            SETTLED,
            DOMAIN_LIST_CREATED_MESSAGE,
            FIREWALL_DOMAIN_LIST_STATUS_VALUES,
            &format!("{case}: the create"),
        );
        assert_step(
            &read_list(&router, &id).await,
            SETTLED,
            DOMAIN_LIST_CREATED_MESSAGE,
            FIREWALL_DOMAIN_LIST_STATUS_VALUES,
            &format!("{case}: the read"),
        );

        for (index, operation) in FIREWALL_DOMAIN_UPDATE_OPERATION_VALUES
            .iter()
            // "DENY - For an AutoAllowlist rule's managed domain list only": the
            // list this case created is one the account defined, which that
            // operation is not an operation of, so the rows are the operations
            // such a list takes.
            .filter(|operation| **operation != FIREWALL_DOMAIN_OPERATION_DENY)
            .enumerate()
        {
            // A different number of domains per row: one, several, and none.
            let domains: Vec<String> = (0..index)
                .map(|slot| format!("host-{slot}.example.com"))
                .chain(std::iter::once(format!("{operation}.example.com")))
                .collect();
            // The acknowledgement states the step the list stood in as the
            // update was accepted: the step the change reaches is what the read
            // after it reports. So the first update answers the creation step
            // and every later one the finished-change step.
            let accepted = if index == 0 {
                DOMAIN_LIST_CREATED_MESSAGE
            } else {
                DOMAIN_LIST_CHANGED_MESSAGE
            };
            let answered = ok(
                &router,
                "UpdateFirewallDomains",
                json!({
                    "FirewallDomainListId": id.clone(),
                    "Operation": operation,
                    "Domains": domains,
                }),
            )
            .await;
            assert_step(
                &answered,
                SETTLED,
                accepted,
                FIREWALL_DOMAIN_LIST_STATUS_VALUES,
                &format!("{case}: {operation}"),
            );
            // The whole set operation is applied inside the call, so the list
            // is settled by the time it answers — in the step that write
            // reached, which every later read reports.
            assert_step(
                &read_list(&router, &id).await,
                SETTLED,
                DOMAIN_LIST_CHANGED_MESSAGE,
                FIREWALL_DOMAIN_LIST_STATUS_VALUES,
                &format!("{case}: the read after {operation}"),
            );
        }

        // The delete has been accepted, so the list stands in the deleting step
        // and states that step's own sentence.
        let deleted = domain_list(
            &ok(
                &router,
                "DeleteFirewallDomainList",
                json!({ "FirewallDomainListId": id.clone() }),
            )
            .await,
        );
        assert_step(
            &deleted,
            DELETING,
            DOMAIN_LIST_DELETING_MESSAGE,
            FIREWALL_DOMAIN_LIST_STATUS_VALUES,
            &format!("{case}: the delete"),
        );
        expect_error(
            &router,
            "GetFirewallDomainList",
            json!({ "FirewallDomainListId": id }),
            case,
            "ResourceNotFoundException",
        )
        .await;
    }
}

/// Every projection of a rule group — the create, the read and the delete —
/// reports the step the group stands in as both `Status` and `StatusMessage`.
/// The delete reports the step the group STOOD in when it was accepted: the
/// rule group's machine has no delete step the service stamps, unlike the
/// domain list's.
#[tokio::test]
async fn every_rule_group_projection_states_the_step_the_group_stands_in() {
    let router = emulator();

    for ResourceCase { case, request } in resource_cases() {
        let made = rule_group(&ok(&router, "CreateFirewallRuleGroup", request).await);
        let id = string_member(&made, "Id");

        assert_step(
            &made,
            SETTLED,
            RULE_GROUP_CREATED_MESSAGE,
            FIREWALL_RULE_GROUP_STATUS_VALUES,
            &format!("{case}: the create"),
        );

        let read = rule_group(
            &ok(
                &router,
                "GetFirewallRuleGroup",
                json!({ "FirewallRuleGroupId": id.clone() }),
            )
            .await,
        );
        assert_step(
            &read,
            SETTLED,
            RULE_GROUP_CREATED_MESSAGE,
            FIREWALL_RULE_GROUP_STATUS_VALUES,
            &format!("{case}: the read"),
        );

        let deleted = rule_group(
            &ok(
                &router,
                "DeleteFirewallRuleGroup",
                json!({ "FirewallRuleGroupId": id.clone() }),
            )
            .await,
        );
        assert_step(
            &deleted,
            SETTLED,
            RULE_GROUP_CREATED_MESSAGE,
            FIREWALL_RULE_GROUP_STATUS_VALUES,
            &format!("{case}: the delete"),
        );
        expect_error(
            &router,
            "GetFirewallRuleGroup",
            json!({ "FirewallRuleGroupId": id }),
            case,
            "ResourceNotFoundException",
        )
        .await;
    }
}

/// A rule group whose rules a caller has written states the step that change
/// reached and no longer the one its create did — the domain list's rule
/// ([`every_domain_list_projection_states_the_step_the_list_stands_in`]) over
/// the other resource that carries rules.
///
/// The rows are every write that changes what a group holds: a rule added to
/// it, that rule changed in it, and the rule removed from it again.
#[tokio::test]
async fn a_rule_group_states_the_step_its_last_rule_write_reached() {
    let router = emulator();
    let list = string_member(
        &domain_list(
            &ok(
                &router,
                "CreateFirewallDomainList",
                json!({ "CreatorRequestId": "req-rule-source", "Name": "rule-source" }),
            )
            .await,
        ),
        "Id",
    );
    let group = string_member(
        &rule_group(
            &ok(
                &router,
                "CreateFirewallRuleGroup",
                json!({ "CreatorRequestId": "req-written-group", "Name": "written-group" }),
            )
            .await,
        ),
        "Id",
    );

    let writes = vec![
        (
            "a rule added to the group",
            "CreateFirewallRule",
            json!({
                "CreatorRequestId": "req-written-rule",
                "FirewallRuleGroupId": group.clone(),
                "FirewallDomainListId": list.clone(),
                "Name": "written-rule",
                "Priority": 100,
                "Action": "ALERT",
            }),
        ),
        (
            "that rule changed in the group",
            "UpdateFirewallRule",
            json!({
                "FirewallRuleGroupId": group.clone(),
                "FirewallDomainListId": list.clone(),
                "Priority": 200,
            }),
        ),
        (
            "the rule removed from the group again",
            "DeleteFirewallRule",
            json!({
                "FirewallRuleGroupId": group.clone(),
                "FirewallDomainListId": list.clone(),
            }),
        ),
    ];

    for (case, operation, request) in writes {
        ok(&router, operation, request).await;
        let read = rule_group(
            &ok(
                &router,
                "GetFirewallRuleGroup",
                json!({ "FirewallRuleGroupId": group.clone() }),
            )
            .await,
        );
        assert_step(
            &read,
            SETTLED,
            RULE_GROUP_CHANGED_MESSAGE,
            FIREWALL_RULE_GROUP_STATUS_VALUES,
            case,
        );
    }
}

/// Every domain list DNS Firewall publishes itself stands in the settled step,
/// so a read of one states that step as well: the rule covers the whole
/// catalog, including the entries the lists this account creates never touch.
#[tokio::test]
async fn every_published_domain_list_states_the_step_it_stands_in() {
    let router = emulator();
    let listing = ok(
        &router,
        "ListFirewallDomainLists",
        json!({ "MaxResults": 100 }),
    )
    .await;
    let published: Vec<(String, String)> = listing
        .get("FirewallDomainLists")
        .and_then(Value::as_array)
        .expect("the listing reports the domain lists this account is served")
        .iter()
        // "The owner of the list, used only for lists that are NOT managed by
        // you": the member is what tells a published entry from an own one.
        .filter(|item| item.get("ManagedOwnerName").is_some())
        .map(|item| (string_member(item, "Name"), string_member(item, "Id")))
        .collect();
    assert!(
        !published.is_empty(),
        "case \"the catalog\": the listing must report the lists DNS Firewall publishes, got {listing}"
    );

    for (name, id) in published {
        assert_step(
            &read_list(&router, &id).await,
            SETTLED,
            DOMAIN_LIST_CREATED_MESSAGE,
            FIREWALL_DOMAIN_LIST_STATUS_VALUES,
            &format!("the published list {name}"),
        );
    }
}

/// The step is the resource's own: lists and groups created by different
/// requests, and read at different points in their lives, all state the
/// sentence of the step they stand in — so a list's message is never a group's,
/// and never a value the request supplied.
#[tokio::test]
async fn the_sentence_belongs_to_the_step_and_not_to_one_request() {
    let router = emulator();

    // (what the message was read from, the message) for four lists and four
    // groups, each made by its own request.
    let mut seen: Vec<(String, String)> = Vec::new();
    for index in 0..4 {
        let list = domain_list(
            &ok(
                &router,
                "CreateFirewallDomainList",
                json!({
                    "CreatorRequestId": format!("req-list-{index}"),
                    "Name": format!("list-{index}"),
                }),
            )
            .await,
        );
        let list_id = string_member(&list, "Id");
        let group = rule_group(
            &ok(
                &router,
                "CreateFirewallRuleGroup",
                json!({
                    "CreatorRequestId": format!("req-group-{index}"),
                    "Name": format!("group-{index}"),
                }),
            )
            .await,
        );
        seen.push((
            format!("the create of list {index}"),
            string_member(&list, "StatusMessage"),
        ));
        seen.push((
            format!("the create of group {index}"),
            string_member(&group, "StatusMessage"),
        ));

        // A list read back after a write to it still stands in a settled step,
        // and states the step that write reached.
        ok(
            &router,
            "UpdateFirewallDomains",
            json!({
                "FirewallDomainListId": list_id.clone(),
                "Operation": "ADD",
                "Domains": [format!("d-{index}.example.com")],
            }),
        )
        .await;
        seen.push((
            format!("the read of list {index} after a write"),
            string_member(&read_list(&router, &list_id).await, "StatusMessage"),
        ));
    }

    for (case, message) in &seen {
        let expected = if case.contains("group") {
            RULE_GROUP_CREATED_MESSAGE
        } else if case.contains("after a write") {
            DOMAIN_LIST_CHANGED_MESSAGE
        } else {
            DOMAIN_LIST_CREATED_MESSAGE
        };
        assert_eq!(
            message, expected,
            "case {case:?}: expected the settled step's sentence {expected:?}, got {message:?}"
        );
    }
    assert_eq!(
        seen.len(),
        12,
        "case \"four lists and four groups\": every projection was read, got {seen:?}"
    );
}

/// The sentences a projection of an association may state beside each step.
///
/// One step carries two: the model's enum declares no `CREATING` value, so an
/// association being put in place and a change to one that already stood both
/// report [`IN_FLIGHT`], each with its own sentence. Every other step admits
/// exactly one, which is what lets a reading taken while the clock may or may
/// not have settled the association still be checked.
fn association_sentences_of(status: &str) -> &'static [&'static str] {
    match status {
        SETTLED => &[ASSOCIATION_SETTLED_MESSAGE],
        IN_FLIGHT => &[ASSOCIATION_CREATING_MESSAGE, ASSOCIATION_CHANGING_MESSAGE],
        DELETING => &[ASSOCIATION_DELETING_MESSAGE],
        _ => &[],
    }
}

/// Assert the association pairs the step it reports with a sentence THAT step
/// states — the invariant that holds whichever change is settling.
fn assert_paired_step(association: &Value, case: &str) {
    let status = string_member(association, "Status");
    let message = string_member(association, "StatusMessage");
    let admitted = association_sentences_of(&status);
    assert!(
        admitted.contains(&message.as_str()),
        "case {case:?}: Status {status:?} states one of {admitted:?}, got {message:?} in \
         {association}"
    );
}

/// The `FirewallRuleGroupAssociation` of a response body.
fn association(case: &str, body: &Value) -> Value {
    body.get("FirewallRuleGroupAssociation")
        .unwrap_or_else(|| panic!("case {case:?}: the response carries an association, got {body}"))
        .clone()
}

/// Create one rule group over the wire and answer the id the service minted.
async fn create_rule_group(router: &Emulator, label: &str) -> String {
    let created = ok(
        router,
        "CreateFirewallRuleGroup",
        json!({
            "CreatorRequestId": format!("group-{label}"),
            "Name": format!("group-{label}"),
        }),
    )
    .await;
    string_member(&rule_group(&created), "Id")
}

/// Read the association `id` back through its own reader.
async fn read_association(router: &Emulator, id: &str) -> Value {
    let read = ok(
        router,
        "GetFirewallRuleGroupAssociation",
        json!({ "FirewallRuleGroupAssociationId": id }),
    )
    .await;
    association("the read", &read)
}

/// The association `id` as `ListFirewallRuleGroupAssociations` reports it.
async fn listed_association(router: &Emulator, case: &str, id: &str) -> Value {
    // `MaxResults` ranges 1..100 and no case below makes more associations than
    // that, so one page carries every one of them.
    let listing = ok(
        router,
        "ListFirewallRuleGroupAssociations",
        json!({ "MaxResults": 100 }),
    )
    .await;
    listing
        .get("FirewallRuleGroupAssociations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing reports the associations, got {listing}")
        })
        .iter()
        .find(|entry| string_member(entry, "Id") == id)
        .unwrap_or_else(|| panic!("case {case:?}: the listing must report {id}, got {listing}"))
        .clone()
}

/// One row of the association table: how the association is made, and what its
/// update changes.
struct AssociationCase {
    case: &'static str,
    /// `Name`, in one of the forms the model's pattern
    /// `^(?!^[0-9]+$)([a-zA-Z0-9\-_' ']+)$` admits.
    name: &'static str,
    /// `CreatorRequestId`, in forms of different shapes.
    creator_request_id: &'static str,
    /// `Priority`, from the band `AssociateFirewallRuleGroup$Priority`
    /// publishes: "the allowed values for Priority are between 100 and 9900".
    priority: i32,
    /// `MutationProtection` as the request names it, or `None` to leave it to
    /// the default the operation documents.
    mutation_protection: Option<&'static str>,
    /// The members the row's update names. An association held against
    /// modification admits only the change that lifts that protection, so what
    /// the update carries differs by row.
    update: Value,
}

/// The rows every association step below is driven over: the inputs the rule
/// has to answer the same way, because the step an association stands in is not
/// something the request chooses.
fn association_cases() -> Vec<AssociationCase> {
    vec![
        AssociationCase {
            case: "the lowest published priority, protection left to its default",
            name: "assoc-low-bound",
            creator_request_id: "req-low",
            priority: 100,
            mutation_protection: None,
            update: json!({ "Priority": 2_000 }),
        },
        AssociationCase {
            case: "a priority inside the band, protection asked off explicitly",
            name: "ops' assoc 2",
            creator_request_id: "2026-11-10T02:49:39Z",
            priority: 5_000,
            mutation_protection: Some("DISABLED"),
            update: json!({ "Name": "renamed_by_the_update", "Priority": 3_300 }),
        },
        AssociationCase {
            case: "the highest published priority, protection asked for",
            name: "assoc_high_bound",
            creator_request_id: "r",
            priority: 9_900,
            mutation_protection: Some("ENABLED"),
            update: json!({ "MutationProtection": "DISABLED" }),
        },
    ]
}

/// Every projection of a rule-group association — the acknowledgement, the
/// reader a client polls, the listing, the update's answer and the
/// disassociate's — reports the step the association stands in as both `Status`
/// and `StatusMessage`.
///
/// Putting an association in place is asynchronous, so the step the
/// acknowledgement reports is the one that change is under way in, the step the
/// readers report once the move the clock owes has fallen due is the settled
/// one, and a later change to the association is under way in a step of its own.
#[tokio::test]
async fn every_association_projection_states_the_step_it_stands_in() {
    let router = emulator();

    for (index, row) in association_cases().into_iter().enumerate() {
        let case = row.case;
        let group = create_rule_group(&router, &format!("step-{index}")).await;
        let vpc = format!("vpc-step-{index}");
        router.hold_vpcs([&vpc]);

        let mut request = json!({
            "CreatorRequestId": row.creator_request_id,
            "FirewallRuleGroupId": group,
            "VpcId": vpc,
            "Priority": row.priority,
            "Name": row.name,
        });
        if let Some(protection) = row.mutation_protection {
            request["MutationProtection"] = json!(protection);
        }

        let made = association(
            case,
            &ok(&router, "AssociateFirewallRuleGroup", request).await,
        );
        assert_step(
            &made,
            IN_FLIGHT,
            ASSOCIATION_CREATING_MESSAGE,
            FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_VALUES,
            &format!("{case}: the associate"),
        );
        let id = string_member(&made, "Id");

        // A read taken before the move the clock owes has fallen due reports
        // whichever step the association stands in by then — always with that
        // step's own sentence.
        assert_paired_step(
            &read_association(&router, &id).await,
            &format!("{case}: the read while the association is being made"),
        );

        // Once the change has settled, the reader a client polls and the listing
        // both state the settled step.
        settled_association(&router, &id, case).await;
        assert_step(
            &read_association(&router, &id).await,
            SETTLED,
            ASSOCIATION_SETTLED_MESSAGE,
            FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_VALUES,
            &format!("{case}: the read once the association settled"),
        );
        assert_step(
            &listed_association(&router, case, &id).await,
            SETTLED,
            ASSOCIATION_SETTLED_MESSAGE,
            FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_VALUES,
            &format!("{case}: the listing once the association settled"),
        );

        // A change to an association that already stood: the transient step is
        // about THIS change, so it states the change's own sentence and not the
        // one the association came up under.
        let mut update = row.update.clone();
        update["FirewallRuleGroupAssociationId"] = json!(id.clone());
        let updated = association(
            case,
            &ok(&router, "UpdateFirewallRuleGroupAssociation", update).await,
        );
        assert_step(
            &updated,
            IN_FLIGHT,
            ASSOCIATION_CHANGING_MESSAGE,
            FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_VALUES,
            &format!("{case}: the update"),
        );

        settled_association(&router, &id, case).await;
        assert_step(
            &read_association(&router, &id).await,
            SETTLED,
            ASSOCIATION_SETTLED_MESSAGE,
            FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_VALUES,
            &format!("{case}: the read once the update settled"),
        );

        // The association the disassociate took out is on its way out, so it
        // states the removing step and that step's own sentence.
        let removed = association(
            case,
            &ok(
                &router,
                "DisassociateFirewallRuleGroup",
                json!({ "FirewallRuleGroupAssociationId": id }),
            )
            .await,
        );
        assert_step(
            &removed,
            DELETING,
            ASSOCIATION_DELETING_MESSAGE,
            FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_VALUES,
            &format!("{case}: the disassociate"),
        );
    }
}

/// An association's sentence belongs to the step and not to one request:
/// associations made by different requests, at different priorities, in
/// different VPCs and with different names all state the same sentence once they
/// stand in the same step, and one listing pairs each of them with ITS own step.
#[tokio::test]
async fn an_associations_sentence_belongs_to_its_step_and_not_to_one_request() {
    let router = emulator();

    // (the association's name, its priority) — four associations of one account,
    // each made by its own request into its own VPC.
    let rows: &[(&str, i32)] = &[
        ("listed-first", 150),
        ("listed second 2", 3_300),
        ("listed_third", 7_777),
        ("l", 9_800),
    ];

    let mut made: Vec<(String, String)> = Vec::new();
    for (index, (name, priority)) in rows.iter().enumerate() {
        let case = format!("the association {name:?}");
        let group = create_rule_group(&router, &format!("sentence-{index}")).await;
        let vpc = format!("vpc-sentence-{index}");
        router.hold_vpcs([&vpc]);
        let acknowledged = association(
            &case,
            &ok(
                &router,
                "AssociateFirewallRuleGroup",
                json!({
                    "CreatorRequestId": format!("req-sentence-{index}"),
                    "FirewallRuleGroupId": group,
                    "VpcId": vpc,
                    "Priority": priority,
                    "Name": name,
                }),
            )
            .await,
        );
        assert_step(
            &acknowledged,
            IN_FLIGHT,
            ASSOCIATION_CREATING_MESSAGE,
            FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_VALUES,
            &format!("{case}: the associate"),
        );
        made.push((case, string_member(&acknowledged, "Id")));
    }

    // One listing, read while the associations are in whatever step the clock
    // has left them in: every entry is paired with its own step.
    for (case, id) in &made {
        assert_paired_step(
            &listed_association(&router, case, id).await,
            &format!("{case}: the listing while the associations are being made"),
        );
    }

    // Once each has settled, they all state the SAME sentence — the step's, not
    // the association's — through the reader and through the listing alike.
    for (case, id) in &made {
        settled_association(&router, id, case).await;
    }
    for (case, id) in &made {
        assert_step(
            &read_association(&router, id).await,
            SETTLED,
            ASSOCIATION_SETTLED_MESSAGE,
            FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_VALUES,
            &format!("{case}: the read once every association settled"),
        );
        assert_step(
            &listed_association(&router, case, id).await,
            SETTLED,
            ASSOCIATION_SETTLED_MESSAGE,
            FIREWALL_RULE_GROUP_ASSOCIATION_STATUS_VALUES,
            &format!("{case}: the listing once every association settled"),
        );
    }
    assert_eq!(
        made.len(),
        rows.len(),
        "case \"four associations\": every association was projected, got {made:?}"
    );
}
