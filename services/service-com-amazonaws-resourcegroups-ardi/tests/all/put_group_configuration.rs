// SPDX-License-Identifier: CC0-1.0

//! `PutGroupConfiguration`, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the lifecycle suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * a put is ACCEPTED rather than applied: it answers 202 with the empty document its
//!   modelled output declares, and the configuration it carries is then reported as the
//!   group's proposed one, under the `UPDATING` status and beside the configuration
//!   still in effect, until the span the put started has elapsed — the span is never
//!   zero, so a caller reading straight afterwards still sees the update in flight;
//! * once the span has elapsed the configuration the put carried is the one in effect,
//!   reported under `UPDATE_COMPLETE` with nothing proposed, and every later read
//!   reports the same thing;
//! * a put REPLACES the whole configuration, and a put that carries no configuration
//!   item leaves the group with none at all;
//! * the LAST accepted put decides the outcome: one accepted while an update is still
//!   in flight supersedes it, and a refused one leaves the stored configuration and the
//!   update in flight exactly as they stand;
//! * a group contains either a configuration or a resource query, never both, so each
//!   of the two refuses the other — including while a configuration update is in
//!   flight;
//! * the configuration a put puts in effect is the one every other operation that reads
//!   a group's configuration then reads.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_resourcegroups_ardi::ops::types::GROUP_CONFIGURATION_STATUS_VALUES;
use service_com_amazonaws_resourcegroups_ardi::state::{
    ComAmazonawsResourcegroupsArdiState, GroupKey, APPLICATION_TAG_KEY, CONFIGURATION_IN_EFFECT,
    CONFIGURATION_UPDATING,
};

use super::group_configuration_projection::is_empty_document;
use super::resource_group_lifecycle::{error_code, Emulator, ACCOUNT};

/// The region every row works in unless it is asserting the per-region rule.
const HOME: &str = "us-east-1";

/// `POST /put-group-configuration` with `body` as the request payload.
async fn put_in(emulator: &Emulator, region: &str, body: &Value) -> (u16, Value) {
    emulator
        .request_in(region, "POST", "/put-group-configuration", Some(body))
        .await
}

/// `POST /put-group-configuration` in [`HOME`].
async fn put(emulator: &Emulator, body: &Value) -> (u16, Value) {
    put_in(emulator, HOME, body).await
}

/// A put the caller expects to be accepted: a 202 carrying the empty document its
/// modelled output declares, after which the document the request carried is the
/// group's PROPOSED configuration, under the `UPDATING` status, because the span a put
/// starts is never zero.
async fn accepted(emulator: &Emulator, case: &str, body: &Value) {
    let (status, answered) = put(emulator, body).await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected an accepted put to answer 202, got {status} {answered}",
    );
    assert!(
        is_empty_document(&answered),
        "case {case:?}: PutGroupConfiguration declares no output member, so an accepted \
         put answers the empty document, got {answered}",
    );
    let group = body["Group"].as_str().expect("the request names a group");
    // What the put wrote is read back where the model puts it: the group's own
    // configuration document.
    let (_in_effect, proposed, status_reported) =
        members_of(&reported(emulator, case, group).await);
    assert_eq!(
        proposed,
        body.get("Configuration").cloned().unwrap_or_else(|| json!([])),
        "case {case:?}: expected the configuration the put accepted to be the group's \
         proposed one",
    );
    assert_eq!(
        status_reported, CONFIGURATION_UPDATING,
        "case {case:?}: expected the accepted update to be reported as in flight",
    );
}

/// `POST /get-group-configuration` for `group`.
async fn configuration_of(emulator: &Emulator, group: &str) -> (u16, Value) {
    emulator
        .call("/get-group-configuration", &json!({ "Group": group }))
        .await
}

/// The `GroupConfiguration` document `GetGroupConfiguration` reports for `group`,
/// which must be a 200 carrying one.
async fn reported(emulator: &Emulator, case: &str, group: &str) -> Value {
    let (status, body) = configuration_of(emulator, group).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetGroupConfiguration expected 200, got {status} {body}",
    );
    body.get("GroupConfiguration")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a GroupConfiguration, got {body}"))
}

/// The three members a reported configuration is judged by: the configuration in
/// effect, the proposed one, and the status. An absent list reads as an empty one,
/// so a row that proposes nothing is compared the same way as one that proposes
/// items.
fn members_of(reported: &Value) -> (Value, Value, String) {
    (
        reported
            .get("Configuration")
            .cloned()
            .unwrap_or_else(|| json!([])),
        reported
            .get("ProposedConfiguration")
            .cloned()
            .unwrap_or_else(|| json!([])),
        reported
            .get("Status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// The instant the configuration update the store holds for `name` is owed, or
/// `None` when the store holds no accepted update for that group.
fn due_at(emulator: &Emulator, region: &str, name: &str) -> Option<SystemTime> {
    emulator
        .engine()
        .state::<ComAmazonawsResourcegroupsArdiState>()
        .lock()
        .groups
        .get(&GroupKey::new(region, name))
        .and_then(|record| record.pending_configuration.as_ref())
        .map(|pending| pending.due_at)
}

/// Whether `name` has a configuration update the clock still owes.
fn in_flight(emulator: &Emulator, name: &str) -> bool {
    due_at(emulator, HOME, name).is_some_and(|due| due > SystemTime::now())
}

/// Move the update `name` has in flight into the past, so the clock now owes it.
///
/// The due instant is what the handlers read, so moving it is how this suite reaches
/// the state a caller reaches by waiting — without sleeping for the span.
pub(crate) fn make_update_due(emulator: &Emulator, name: &str) {
    let state = emulator
        .engine()
        .state::<ComAmazonawsResourcegroupsArdiState>();
    let mut store = state.lock();
    let pending = store
        .groups
        .get_mut(&GroupKey::new(HOME, name))
        .and_then(|record| record.pending_configuration.as_mut())
        .expect("a configuration update is in flight");
    pending.due_at = SystemTime::now() - Duration::from_millis(1);
}

/// Every status this service writes for a service configuration is a member of the
/// enum the model generated its value list from, so the statuses on the wire cannot
/// drift from the model.
#[test]
fn every_configuration_status_this_service_writes_is_a_modelled_enum_member() {
    for (case, status) in [
        ("the status of an update in flight", CONFIGURATION_UPDATING),
        (
            "the status of a configuration in effect",
            CONFIGURATION_IN_EFFECT,
        ),
    ] {
        assert!(
            GROUP_CONFIGURATION_STATUS_VALUES.contains(&status),
            "case {case:?}: expected {status} to be one of \
             {GROUP_CONFIGURATION_STATUS_VALUES:?}",
        );
    }
}

/// One row of the put table: the `Configuration` a put carries.
struct PutCase {
    label: &'static str,
    /// The `Configuration` member of the request, or `None` for a request that omits
    /// it altogether.
    configuration: Option<Value>,
    /// Whether the configuration the row carries leaves the group configured at all.
    configures: bool,
}

/// Every configuration form the rules are asserted over. The rows differ along the
/// NUMBER of items (none, one, the modelled maximum of two), whether an item carries
/// parameters, and which configuration type the items declare — including the one
/// that gives a group an application identity.
///
/// The configuration item types and parameter names are AWS service-configuration
/// vocabulary from the "Service configurations for resource groups" reference, which
/// is what `GroupConfigurationItem$Type` and `GroupConfigurationParameter$Name` are
/// documented against; `GroupConfigurationList` is 0..2 long
/// (`smithy_query(query="shape",shape="GroupConfigurationList")`).
fn put_cases() -> Vec<PutCase> {
    vec![
        PutCase {
            label: "one configuration item carrying no parameters",
            configuration: Some(json!([{ "Type": "AWS::ResourceGroups::Generic" }])),
            configures: true,
        },
        PutCase {
            label: "one configuration item carrying two parameters",
            configuration: Some(json!([{
                "Type": "AWS::EC2::HostManagement",
                "Parameters": [
                    { "Name": "allowed-host-families", "Values": ["c5", "m5"] },
                    { "Name": "auto-allocate-host", "Values": ["true"] }
                ]
            }])),
            configures: true,
        },
        PutCase {
            label: "two configuration items, the modelled maximum",
            configuration: Some(json!([
                { "Type": "AWS::EC2::CapacityReservationPool" },
                { "Type": "AWS::ResourceGroups::Generic" }
            ])),
            configures: true,
        },
        PutCase {
            label: "the configuration type that declares an application group",
            configuration: Some(json!([{ "Type": "AWS::ResourceGroups::ApplicationGroup" }])),
            configures: true,
        },
        PutCase {
            label: "an empty configuration list, which configures nothing",
            configuration: Some(json!([])),
            configures: false,
        },
        PutCase {
            label: "no Configuration member at all, which configures nothing",
            configuration: None,
            configures: false,
        },
    ]
}

/// A put is accepted and not applied: the configuration it carries is the proposed
/// one until the span the put started has elapsed, and the one in effect afterwards —
/// which is what every later read reports.
#[tokio::test]
async fn a_put_configuration_is_proposed_and_then_in_effect() {
    for case in put_cases() {
        let label = case.label;
        let name = "put-probe";
        let emulator = Emulator::new();
        // A group created with neither a configuration nor a query: it has none to
        // report until this row's put attaches one.
        emulator.ok("/groups", &json!({ "Name": name })).await;
        let (status, body) = configuration_of(&emulator, name).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {label:?}: a group that holds no configuration has none to get, \
             got {status} {body}",
        );

        let mut request = json!({ "Group": name });
        if let Some(configuration) = case.configuration.clone() {
            request["Configuration"] = configuration;
        }
        accepted(&emulator, label, &request).await;

        // The update is owed at a stored instant that is still in the future when the
        // put answers: the span is never zero, so a caller acting straight after the
        // call sees the update in flight.
        let due = due_at(&emulator, HOME, name)
            .unwrap_or_else(|| panic!("case {label:?}: expected an update in flight"));
        assert!(
            due > SystemTime::now(),
            "case {label:?}: expected the update to fall due later than the put, got {due:?}",
        );

        // While it is in flight it is the PROPOSED configuration, under the UPDATING
        // status; this group had none in effect, so none is reported in effect.
        let proposed = case.configuration.clone().unwrap_or_else(|| json!([]));
        let in_flight_report = reported(&emulator, label, name).await;
        assert_eq!(
            members_of(&in_flight_report),
            (json!([]), proposed.clone(), CONFIGURATION_UPDATING.to_string()),
            "case {label:?}: expected {proposed} proposed under {CONFIGURATION_UPDATING} \
             with nothing in effect, got {in_flight_report}",
        );
        // An update in flight is not an application group yet, whatever it proposes.
        let read = emulator.ok("/get-group", &json!({ "Group": name })).await;
        assert!(
            read["Group"].get("ApplicationTag").is_none(),
            "case {label:?}: a group whose configuration is not in effect yet reports no \
             ApplicationTag, got {read}",
        );

        // Once the clock owes the update, the configuration it carried is the one in
        // effect — and stays so for every later read.
        make_update_due(&emulator, name);
        for read in 0..2 {
            if case.configures {
                let settled = reported(&emulator, label, name).await;
                assert_eq!(
                    members_of(&settled),
                    (
                        proposed.clone(),
                        json!([]),
                        CONFIGURATION_IN_EFFECT.to_string()
                    ),
                    "case {label:?} (read {read}): expected {proposed} in effect under \
                     {CONFIGURATION_IN_EFFECT} with nothing proposed, got {settled}",
                );
            } else {
                let (status, body) = configuration_of(&emulator, name).await;
                assert_eq!(
                    (status, error_code(&body)),
                    (400, "BadRequestException"),
                    "case {label:?} (read {read}): a put that carries no configuration item \
                     leaves the group with none, got {status} {body}",
                );
            }
        }
        assert!(
            !in_flight(&emulator, name),
            "case {label:?}: an update the clock owes is no longer in flight",
        );
        // The group itself is untouched by the configuration it was given.
        let read = emulator.ok("/get-group", &json!({ "Group": name })).await;
        assert_eq!(
            read["Group"]["GroupArn"].as_str(),
            Some(format!("arn:aws:resource-groups:{HOME}:{ACCOUNT}:group/{name}").as_str()),
            "case {label:?}: a put must not change the group's own identity, got {read}",
        );
        // Only the configuration type that declares an application group gives the
        // group the membership tag, and only once that configuration is in effect: the
        // tag binds the one modelled key to the group's application-group ARN.
        let application = proposed.to_string().contains("ApplicationGroup");
        let tag = read["Group"].get("ApplicationTag").cloned();
        match tag {
            Some(tag) if application => {
                let arn = tag[APPLICATION_TAG_KEY].as_str().unwrap_or_else(|| {
                    panic!("case {label:?}: expected an ApplicationTag value, got {tag}")
                });
                let prefix = format!("arn:aws:resource-groups:{HOME}:{ACCOUNT}:group/{name}/");
                assert!(
                    arn.starts_with(&prefix) && arn.len() == prefix.len() + 26,
                    "case {label:?}: expected an application-group ARN under {prefix}, got {arn}",
                );
            }
            other => assert!(
                !application && other.is_none(),
                "case {label:?}: expected an ApplicationTag only for an application group, \
                 got {other:?}",
            ),
        }
    }
}

/// A put replaces the whole configuration, and the LAST accepted one decides the
/// outcome: one accepted while an update is still in flight supersedes it.
#[tokio::test]
async fn the_last_accepted_put_replaces_the_whole_configuration() {
    let name = "superseded";
    let emulator = Emulator::new();
    emulator.ok("/groups", &json!({ "Name": name })).await;

    let first = json!([{ "Type": "AWS::EC2::HostManagement" }]);
    let second = json!([{
        "Type": "AWS::ResourceGroups::Generic",
        "Parameters": [{ "Name": "deletion-protection", "Values": ["UNLESS_EMPTY"] }]
    }]);
    let third = json!([
        { "Type": "AWS::EC2::CapacityReservationPool" },
        { "Type": "AWS::ResourceGroups::Generic" }
    ]);

    // The first put lands, so it is the configuration in effect.
    let case = "the first put, settled";
    accepted(
        &emulator,
        case,
        &json!({ "Group": name, "Configuration": first }),
    )
    .await;
    make_update_due(&emulator, name);
    assert_eq!(
        members_of(&reported(&emulator, case, name).await),
        (
            first.clone(),
            json!([]),
            CONFIGURATION_IN_EFFECT.to_string()
        ),
        "case {case:?}: expected {first} in effect",
    );

    // The second is proposed beside it, and the third supersedes the second while it
    // is still in flight: the configuration in effect is the first one throughout.
    for (case, proposed) in [
        ("the second put, in flight", &second),
        ("the third put, superseding the second", &third),
    ] {
        accepted(
            &emulator,
            case,
            &json!({ "Group": name, "Configuration": proposed }),
        )
        .await;
        assert_eq!(
            members_of(&reported(&emulator, case, name).await),
            (
                first.clone(),
                (*proposed).clone(),
                CONFIGURATION_UPDATING.to_string()
            ),
            "case {case:?}: expected {proposed} proposed beside {first} in effect",
        );
    }

    // The last accepted put is the one that lands, and it REPLACED the whole
    // configuration rather than merging with either earlier one.
    let case = "the third put, settled";
    make_update_due(&emulator, name);
    assert_eq!(
        members_of(&reported(&emulator, case, name).await),
        (
            third.clone(),
            json!([]),
            CONFIGURATION_IN_EFFECT.to_string()
        ),
        "case {case:?}: expected {third} in effect",
    );

    // And a put that carries no configuration item leaves the group with none.
    let case = "a put that attaches nothing";
    accepted(
        &emulator,
        case,
        &json!({ "Group": name, "Configuration": [] }),
    )
    .await;
    make_update_due(&emulator, name);
    let (status, body) = configuration_of(&emulator, name).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case {case:?}: expected the group to hold no configuration, got {status} {body}",
    );
}

/// The puts this service refuses, and why. The rows differ along WHICH rule the
/// request violates: the group it addresses, the region it addresses it in, and the
/// configuration document itself.
fn refused_puts() -> Vec<(&'static str, Value, (u16, &'static str))> {
    vec![
        (
            "a group that does not exist",
            json!({
                "Group": "no-such-group",
                "Configuration": [{ "Type": "AWS::ResourceGroups::Generic" }]
            }),
            (404, "NotFoundException"),
        ),
        (
            "an ARN for another account",
            json!({
                "Group": format!("arn:aws:resource-groups:{HOME}:111122223333:group/configured"),
                "Configuration": [{ "Type": "AWS::ResourceGroups::Generic" }]
            }),
            (404, "NotFoundException"),
        ),
        (
            "no group at all",
            json!({ "Configuration": [{ "Type": "AWS::ResourceGroups::Generic" }] }),
            (400, "BadRequestException"),
        ),
        (
            "two configuration items with the same Type",
            json!({
                "Group": "configured",
                "Configuration": [
                    { "Type": "AWS::ResourceGroups::Generic" },
                    { "Type": "AWS::ResourceGroups::Generic" }
                ]
            }),
            (400, "BadRequestException"),
        ),
        (
            // One past the modelled `GroupConfigurationList` 0..2 length.
            "three configuration items",
            json!({
                "Group": "configured",
                "Configuration": [
                    { "Type": "AWS::ResourceGroups::Generic" },
                    { "Type": "AWS::EC2::HostManagement" },
                    { "Type": "AWS::EC2::CapacityReservationPool" }
                ]
            }),
            (400, "BadRequestException"),
        ),
        (
            "a configuration item Type outside the modelled pattern",
            json!({
                "Group": "configured",
                "Configuration": [{ "Type": "not a configuration type" }]
            }),
            (400, "BadRequestException"),
        ),
    ]
}

/// A refused put changes nothing: the configuration in effect, and an update in
/// flight beside it, are exactly what they were.
#[tokio::test]
async fn a_refused_put_changes_nothing() {
    let name = "configured";
    let emulator = Emulator::new();
    let settled = json!([{ "Type": "AWS::EC2::HostManagement" }]);
    emulator
        .ok(
            "/groups",
            &json!({ "Name": name, "Configuration": settled }),
        )
        .await;
    let in_flight_configuration = json!([{ "Type": "AWS::ResourceGroups::Generic" }]);
    accepted(
        &emulator,
        "the update every row is refused beside",
        &json!({ "Group": name, "Configuration": in_flight_configuration }),
    )
    .await;
    let before = due_at(&emulator, HOME, name).expect("an update is in flight");

    for (case, request, expected) in refused_puts() {
        let (status, body) = put(&emulator, &request).await;
        assert_eq!(
            (status, error_code(&body)),
            expected,
            "case {case:?}: expected {expected:?}, got {status} {body}",
        );
        assert_eq!(
            due_at(&emulator, HOME, name),
            Some(before),
            "case {case:?}: a refused put must leave the update in flight as it stands",
        );
        assert_eq!(
            members_of(&reported(&emulator, case, name).await),
            (
                settled.clone(),
                in_flight_configuration.clone(),
                CONFIGURATION_UPDATING.to_string()
            ),
            "case {case:?}: a refused put must change no reported configuration",
        );
    }

    // A group lives in the region whose endpoint created it, so a put addressed to
    // another region's endpoint reaches no group at all.
    let (status, body) = put_in(
        &emulator,
        "eu-west-1",
        &json!({ "Group": name, "Configuration": [{ "Type": "AWS::ResourceGroups::Generic" }] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "NotFoundException"),
        "a put in another region addresses no group of this one, got {status} {body}",
    );
    assert_eq!(
        due_at(&emulator, HOME, name),
        Some(before),
        "a put in another region must leave this region's update in flight as it stands",
    );
}

/// A group contains either a configuration or a resource query, never both: each of
/// the two refuses the other, and a configuration update still IN FLIGHT excludes a
/// query just as one in effect does.
#[tokio::test]
async fn a_configuration_and_a_resource_query_exclude_each_other() {
    let query = json!({
        "Type": "TAG_FILTERS_1_0",
        "Query": "{\"TagFilters\":[{\"Key\":\"Stage\",\"Values\":[\"Test\"]}]}"
    });
    let configuration = json!([{ "Type": "AWS::EC2::HostManagement" }]);

    // A queried group takes no configuration.
    let emulator = Emulator::new();
    let name = "queried";
    emulator
        .ok(
            "/groups",
            &json!({ "Name": name, "ResourceQuery": query.clone() }),
        )
        .await;
    let case = "a put against a queried group";
    let (status, body) = put(
        &emulator,
        &json!({ "Group": name, "Configuration": configuration.clone() }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case {case:?}: expected 400 BadRequestException, got {status} {body}",
    );
    assert_eq!(
        due_at(&emulator, HOME, name),
        None,
        "case {case:?}: a refused put must start no update",
    );
    let read = emulator
        .ok("/get-group-query", &json!({ "Group": name }))
        .await;
    assert_eq!(
        read["GroupQuery"]["ResourceQuery"], query,
        "case {case:?}: a refused put must leave the stored query alone, got {read}",
    );
    // A put that carries no configuration item attaches nothing, so the exclusion —
    // which is about a group CONTAINING a configuration — does not reach it, and the
    // group keeps its query and holds no configuration.
    let case = "a put that attaches nothing to a queried group";
    accepted(
        &emulator,
        case,
        &json!({ "Group": name, "Configuration": [] }),
    )
    .await;
    make_update_due(&emulator, name);
    let (status, body) = configuration_of(&emulator, name).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case {case:?}: expected the group to hold no configuration, got {status} {body}",
    );
    let read = emulator
        .ok("/get-group-query", &json!({ "Group": name }))
        .await;
    assert_eq!(
        read["GroupQuery"]["ResourceQuery"], query,
        "case {case:?}: expected the stored query untouched, got {read}",
    );

    // And a group a put configures takes no query — while the update is in flight as
    // well as once it is in effect, because a query accepted in between would land
    // beside the configuration.
    for (case, settle) in [
        ("an update in flight", false),
        ("a configuration in effect", true),
    ] {
        let emulator = Emulator::new();
        let name = "configurable";
        emulator.ok("/groups", &json!({ "Name": name })).await;
        accepted(
            &emulator,
            case,
            &json!({ "Group": name, "Configuration": configuration.clone() }),
        )
        .await;
        if settle {
            make_update_due(&emulator, name);
        }
        let (status, body) = emulator
            .call(
                "/update-group-query",
                &json!({ "Group": name, "ResourceQuery": query.clone() }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "BadRequestException"),
            "case {case:?}: expected UpdateGroupQuery to be refused, got {status} {body}",
        );
        let (status, body) = emulator
            .call("/get-group-query", &json!({ "Group": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused UpdateGroupQuery stores no query, got {status} {body}",
        );
    }
}

/// The configuration a put puts in effect is the one every other operation that reads
/// a group's configuration then reads — and while the update is in flight those
/// operations still read the configuration in effect.
#[tokio::test]
async fn the_configuration_a_put_puts_in_effect_governs_the_operations_that_read_it() {
    let emulator = Emulator::new();
    let name = "governed";
    let member = "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0";
    emulator.ok("/groups", &json!({ "Name": name })).await;
    let group_resources = json!({ "Group": name, "ResourceArns": [member] });

    // A group that holds no configuration takes no static membership.
    let case = "no configuration at all";
    let (status, body) = emulator.call("/group-resources", &group_resources).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case {case:?}: expected GroupResources to be refused, got {status} {body}",
    );

    // A configuration that is only PROPOSED does not serve it either.
    let case = "a static-membership configuration still in flight";
    accepted(
        &emulator,
        case,
        &json!({
            "Group": name,
            "Configuration": [{ "Type": "AWS::EC2::HostManagement" }]
        }),
    )
    .await;
    let (status, body) = emulator.call("/group-resources", &group_resources).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case {case:?}: expected GroupResources to be refused while the configuration is \
         not in effect, got {status} {body}",
    );

    // Once it is in effect, the same call is served.
    let case = "a static-membership configuration in effect";
    make_update_due(&emulator, name);
    let answered = emulator.ok("/group-resources", &group_resources).await;
    assert_eq!(
        answered["Succeeded"],
        json!([member]),
        "case {case:?}: expected the membership to be recorded, got {answered}",
    );

    // A put can also change the rule another operation reads: this configuration
    // protects the group from deletion while it still holds a resource.
    let case = "deletion protection put in effect";
    accepted(
        &emulator,
        case,
        &json!({
            "Group": name,
            "Configuration": [{
                "Type": "AWS::EC2::HostManagement",
                "Parameters": [{ "Name": "deletion-protection", "Values": ["UNLESS_EMPTY"] }]
            }]
        }),
    )
    .await;
    make_update_due(&emulator, name);
    let (status, body) = emulator
        .call("/delete-group", &json!({ "Group": name }))
        .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case {case:?}: expected DeleteGroup to be refused, got {status} {body}",
    );

    // And a put that replaces that configuration takes the protection away with it,
    // while the memberships the group already holds stay where they are.
    let case = "a configuration that neither protects nor serves static membership";
    accepted(
        &emulator,
        case,
        &json!({
            "Group": name,
            "Configuration": [{ "Type": "AWS::ResourceGroups::Generic" }]
        }),
    )
    .await;
    make_update_due(&emulator, name);
    let listed = emulator
        .ok("/list-group-resources", &json!({ "Group": name }))
        .await;
    assert_eq!(
        listed["ResourceIdentifiers"][0]["ResourceArn"].as_str(),
        Some(member),
        "case {case:?}: expected the recorded membership to survive a put, got {listed}",
    );
    let (status, body) = emulator.call("/group-resources", &group_resources).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "BadRequestException"),
        "case {case:?}: expected GroupResources to be refused again, got {status} {body}",
    );
    let deleted = emulator
        .ok("/delete-group", &json!({ "Group": name }))
        .await;
    assert_eq!(
        deleted["Group"]["Name"].as_str(),
        Some(name),
        "case {case:?}: expected the unprotected group to be deletable, got {deleted}",
    );
}
