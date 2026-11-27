// SPDX-License-Identifier: CC0-1.0

//! The ONE filter-matching rule this service's listings narrow by, driven end to end over
//! HTTP through each listing that shares it.
//!
//! `ListEnabledControls`, `ListEnabledBaselines` and `ListControlOperations` all ask the
//! same question of a `filter` member — does this record's own value for the member appear
//! among the values the caller sent? — so the rule is written once, in `crate::state`, and
//! each handler reaches it there. This suite drives that single path through all three
//! listings, so a change to it that narrowed one listing differently from its siblings
//! fails here.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees.
//!
//! The rule, stated once in [`selected_by_the_rule`] and asserted over a TABLE of inputs
//! rather than one case:
//!
//! * a member the caller omits narrows nothing, in either shape an omission takes (no
//!   `filter` at all, or a `filter` carrying no member);
//! * a member they send keeps the records whose own reported value for it is one of the
//!   values sent — whatever the member is, and however many values it carries, up to the
//!   modelled maximum length of that list;
//! * a record with NO value for a sent member is not selected, so a member no record here
//!   has a value for selects nothing;
//! * the members narrow TOGETHER: a record must satisfy every member sent.
//!
//! Every expected set is computed from the listing's own unfiltered answer by that one
//! rule, so no case repeats an identifier or a status the fixture minted.
//!
//! The value sets and list bounds written out by hand name their source. The enum value
//! sets come from `smithy_query(query="shape", shape="EnablementStatus,DriftStatus,
//! EnabledBaselineDriftStatus,ControlOperationStatus,ControlOperationType")` and the
//! maximum length of each `filter` list from `smithy_query(query="shape",
//! shape="EnabledControlFilter,EnabledBaselineFilter,ControlOperationFilter")` on this
//! crate's `smithy-model/`: 1 for every member of `EnabledControlFilter` and
//! `ControlOperationFilter`, and 5 for `EnabledBaselineFilter`'s `targetIdentifiers`,
//! `baselineIdentifiers` and `parentIdentifiers`. The baselines the fixture enables are
//! the `enablements()` table the sibling `baseline_operations` suite drives
//! `EnableBaseline` with, so no baseline, version or target is written out here.

use smithy_server_runtime::MultiServiceRouter;

use crate::baseline_operations::{enable as enable_baseline, enablements};
use crate::enabled_control_lifecycle::{
    control, enable as enable_control, enabled_control_prefix, entries, ou,
};
use crate::front_door_validation::FRONT_DOOR_CODE;
use crate::landing_zone_lifecycle::{call, emulator, poll_until, set_up_emulator, Answer};

/// One listing that narrows by the shared rule: the path its `@http` trait binds it to,
/// the output member carrying its records, and the record member that identifies one
/// record across calls.
#[derive(Clone, Copy)]
struct Listing {
    path: &'static str,
    records: &'static str,
    identity: &'static str,
}

/// `ListEnabledControls`, whose handler held the third copy of the rule.
const ENABLED_CONTROLS: Listing = Listing {
    path: "/list-enabled-controls",
    records: "enabledControls",
    identity: "arn",
};

/// `ListEnabledBaselines`, whose handler held the second copy.
const ENABLED_BASELINES: Listing = Listing {
    path: "/list-enabled-baselines",
    records: "enabledBaselines",
    identity: "arn",
};

/// `ListControlOperations`, whose handler held the first copy.
const CONTROL_OPERATIONS: Listing = Listing {
    path: "/list-control-operations",
    records: "controlOperations",
    identity: "operationIdentifier",
};

/// One `filter` member a case sends: its name on the wire, the dotted path to the record's
/// own value for it, and the values the case sends.
struct Narrowing {
    member: &'static str,
    path: &'static str,
    values: Vec<String>,
}

/// A `filter` member whose values name an identifier.
struct IdMember {
    member: &'static str,
    path: &'static str,
    /// The modelled maximum length of this filter list.
    max_values: usize,
    /// A well-formed value of the member's type that no seeded record reports.
    absent: String,
    /// Whether any record of this listing reports a value for the member at all: `false`
    /// for a member no record here has one for, which the rule must then select nothing
    /// by.
    records_report_a_value: bool,
}

/// A `filter` member whose values are a modelled enum, with that enum's whole value set.
struct EnumMember {
    member: &'static str,
    path: &'static str,
    values: &'static [&'static str],
}

/// `EnablementStatus`, the status both an enabled control and an enabled baseline report.
const ENABLEMENT_STATUSES: &[&str] = &["SUCCEEDED", "FAILED", "UNDER_CHANGE"];

/// `DriftStatus`, the drift status an enabled control reports.
const DRIFT_STATUSES: &[&str] = &["DRIFTED", "IN_SYNC", "NOT_CHECKING", "UNKNOWN"];

/// `EnabledBaselineDriftStatus`, the drift status an enabled baseline reports.
const BASELINE_DRIFT_STATUSES: &[&str] = &["IN_SYNC", "DRIFTED"];

/// `ControlOperationStatus`, the status a control operation record reports.
const CONTROL_OPERATION_STATUSES: &[&str] = &["SUCCEEDED", "FAILED", "IN_PROGRESS"];

/// `ControlOperationType`, the type a control operation record reports.
const CONTROL_OPERATION_TYPES: &[&str] = &[
    "ENABLE_CONTROL",
    "DISABLE_CONTROL",
    "UPDATE_ENABLED_CONTROL",
    "RESET_ENABLED_CONTROL",
];

/// Where an enabled baseline reports the drift status `inheritanceDriftStatuses` narrows
/// by — a member only the baselines Control Tower assesses drift on have a value for.
const BASELINE_INHERITANCE_DRIFT_PATH: &str = "driftStatusSummary.types.inheritance.status";

/// Where the same enablement reports the drift status `sourceDriftStatuses` narrows by —
/// the other dimension the model declares for a baseline, assessed on the same baselines.
const BASELINE_SOURCE_DRIFT_PATH: &str = "driftStatusSummary.types.source.status";

/// A record's value at a DOTTED path into its summary, or `None` when the record reports
/// no value there.
fn at<'a>(record: &'a serde_json::Value, path: &str) -> Option<&'a str> {
    let mut value = record;
    for member in path.split('.') {
        value = value.get(member)?;
    }
    value.as_str()
}

/// THE RULE under test: a record is selected when, for EVERY member the caller sent, the
/// record's own value for that member is one of the values sent — so a member the caller
/// omitted narrows nothing, a record with no value for a sent member is not selected, and
/// the members sent narrow together.
fn selected_by_the_rule(record: &serde_json::Value, sent: &[Narrowing]) -> bool {
    sent.iter().all(|narrowing| {
        at(record, narrowing.path)
            .is_some_and(|actual| narrowing.values.iter().any(|value| value == actual))
    })
}

/// The request body that sends `narrowings` as this call's `filter`.
fn filter_body(narrowings: &[Narrowing]) -> serde_json::Value {
    let filter: serde_json::Map<String, serde_json::Value> = narrowings
        .iter()
        .map(|narrowing| {
            (
                narrowing.member.to_string(),
                serde_json::json!(narrowing.values),
            )
        })
        .collect();
    serde_json::json!({ "filter": serde_json::Value::Object(filter) })
}

/// The records one listing reports for `body`, asserting the call succeeded.
async fn list(
    router: &MultiServiceRouter,
    listing: Listing,
    body: serde_json::Value,
) -> Vec<serde_json::Value> {
    let answer: Answer = call(router, listing.path, body.clone()).await;
    assert_eq!(
        answer.status, 200,
        "{} {body}: expected 200, got {} body {:?}",
        listing.path, answer.status, answer.body
    );
    entries(answer.get(listing.records))
}

/// The identity of every record, sorted, so a case compares SETS rather than depending on
/// the order identifiers were minted in.
fn identities(listing: Listing, records: &[serde_json::Value]) -> Vec<String> {
    let mut found: Vec<String> = records
        .iter()
        .map(|record| at(record, listing.identity).unwrap_or_default().to_string())
        .collect();
    found.sort();
    found
}

/// The distinct values the records report at `path`, sorted, so a case built from them is
/// deterministic.
fn distinct(records: &[serde_json::Value], path: &str) -> Vec<String> {
    let mut values: Vec<String> = records
        .iter()
        .filter_map(|record| at(record, path))
        .map(str::to_string)
        .collect();
    values.sort();
    values.dedup();
    values
}

/// Assert one case: the listing narrowed by `narrowings` reports exactly the records the
/// rule selects out of the whole unfiltered population.
///
/// `at_least` is what makes the case meaningful rather than vacuous — the smallest number
/// of records the fixture must have put in reach of this narrowing.
async fn assert_case(
    router: &MultiServiceRouter,
    listing: Listing,
    population: &[serde_json::Value],
    case: &str,
    narrowings: Vec<Narrowing>,
    at_least: usize,
) {
    let expected: Vec<String> = identities(
        listing,
        &population
            .iter()
            .filter(|record| selected_by_the_rule(record, &narrowings))
            .cloned()
            .collect::<Vec<serde_json::Value>>(),
    );
    assert!(
        expected.len() >= at_least,
        "case {case:?} on {}: the fixture leaves this case vacuous — the rule should \
         select at least {at_least} of the {} seeded records, it selects {expected:?}",
        listing.path,
        population.len()
    );

    let body = filter_body(&narrowings);
    let got = identities(listing, &list(router, listing, body.clone()).await);
    assert_eq!(
        got, expected,
        "case {case:?} on {} ({body}): expected {expected:?}, got {got:?}",
        listing.path
    );
}

/// Assert the rule over one listing: every identifier member and every value of every
/// enum member, against the listing's own unfiltered population.
async fn the_rule_holds_over(
    router: &MultiServiceRouter,
    listing: Listing,
    id_members: &[IdMember],
    enum_members: &[EnumMember],
) {
    let population = list(router, listing, serde_json::json!({})).await;
    assert!(
        !population.is_empty(),
        "{}: the fixture seeded no records, so nothing below could be narrowed",
        listing.path
    );
    let all = identities(listing, &population);

    // A member the caller omits narrows nothing, in either shape an omission takes.
    for (case, body) in [
        ("no filter at all", serde_json::json!({})),
        ("a filter carrying no member", serde_json::json!({ "filter": {} })),
    ] {
        let got = identities(listing, &list(router, listing, body.clone()).await);
        assert_eq!(
            got, all,
            "case {case:?} on {} ({body}): expected every seeded record, got {got:?}",
            listing.path
        );
    }

    for id_member in id_members {
        let reported = distinct(&population, id_member.path);
        assert_eq!(
            !reported.is_empty(),
            id_member.records_report_a_value,
            "{} {}: expected records reporting a value for this member: {}, got {reported:?}",
            listing.path,
            id_member.member,
            id_member.records_report_a_value
        );
        assert!(
            !reported.contains(&id_member.absent),
            "{} {}: {:?} is meant to be a value no record reports, got {reported:?}",
            listing.path,
            id_member.member,
            id_member.absent
        );

        let narrowing = |values: Vec<String>| Narrowing {
            member: id_member.member,
            path: id_member.path,
            values,
        };

        // A value no record reports selects nothing — including for a member no record
        // here has a value for at all, which is why `records_report_a_value` is pinned
        // above.
        assert_case(
            router,
            listing,
            &population,
            &format!("{} on a value no record reports", id_member.member),
            vec![narrowing(vec![id_member.absent.clone()])],
            0,
        )
        .await;

        if reported.is_empty() {
            continue;
        }

        // The modelled minimum length: one value the records do report.
        assert_case(
            router,
            listing,
            &population,
            &format!("{} on one value the records report", id_member.member),
            vec![narrowing(vec![reported[0].clone()])],
            1,
        )
        .await;

        if id_member.max_values >= 2 {
            let several: Vec<String> = reported
                .iter()
                .take(id_member.max_values)
                .cloned()
                .collect();
            if several.len() >= 2 {
                assert_case(
                    router,
                    listing,
                    &population,
                    &format!(
                        "{} on {} values the records report",
                        id_member.member,
                        several.len()
                    ),
                    vec![narrowing(several.clone())],
                    several.len(),
                )
                .await;
            }

            // Up to the modelled MAXIMUM length, with one value no record reports among
            // them: the absent value adds nothing to what the rest select.
            let mut to_the_bound: Vec<String> = reported
                .iter()
                .take(id_member.max_values - 1)
                .cloned()
                .collect();
            let present = to_the_bound.len();
            to_the_bound.push(id_member.absent.clone());
            assert_case(
                router,
                listing,
                &population,
                &format!(
                    "{} on {} values, one of which no record reports",
                    id_member.member,
                    to_the_bound.len()
                ),
                vec![narrowing(to_the_bound)],
                present,
            )
            .await;
        }
    }

    for enum_member in enum_members {
        let reported = distinct(&population, enum_member.path);
        assert!(
            reported
                .iter()
                .all(|value| enum_member.values.contains(&value.as_str())),
            "{} {}: the records report {reported:?}, which is not inside the modelled \
             value set {:?}",
            listing.path,
            enum_member.member,
            enum_member.values
        );
        assert!(
            !reported.is_empty(),
            "{} {}: no record reports a value for this member, so its rows below would be \
             vacuous",
            listing.path,
            enum_member.member
        );

        // Every value the enum declares, whether the records carry it or not: a value they
        // do selects exactly the records reporting it, one they do not selects nothing.
        for value in enum_member.values {
            let carried = reported.iter().any(|found| found == value);
            assert_case(
                router,
                listing,
                &population,
                &format!(
                    "{} on {value}, which {} record reports",
                    enum_member.member,
                    if carried { "some" } else { "no" }
                ),
                vec![Narrowing {
                    member: enum_member.member,
                    path: enum_member.path,
                    values: vec![(*value).to_string()],
                }],
                usize::from(carried),
            )
            .await;
        }
    }

    // The members narrow TOGETHER: an identifier and a status at once select their
    // intersection, and a conjunction one member selects nothing from selects nothing.
    let Some(id_member) = id_members
        .iter()
        .find(|id_member| id_member.records_report_a_value)
    else {
        return;
    };
    let Some(enum_member) = enum_members.first() else {
        return;
    };
    let id_value = distinct(&population, id_member.path)[0].clone();
    let enum_reported = distinct(&population, enum_member.path);
    let together = |enum_value: &str| {
        vec![
            Narrowing {
                member: id_member.member,
                path: id_member.path,
                values: vec![id_value.clone()],
            },
            Narrowing {
                member: enum_member.member,
                path: enum_member.path,
                values: vec![enum_value.to_string()],
            },
        ]
    };

    assert_case(
        router,
        listing,
        &population,
        &format!(
            "{} and {} together, both on values the records report",
            id_member.member, enum_member.member
        ),
        together(&enum_reported[0]),
        1,
    )
    .await;

    if let Some(absent) = enum_member
        .values
        .iter()
        .find(|value| !enum_reported.iter().any(|found| found == *value))
    {
        assert_case(
            router,
            listing,
            &population,
            &format!(
                "{} on a value the records report and {} on one they do not",
                id_member.member, enum_member.member
            ),
            together(absent),
            0,
        )
        .await;
    }
}

/// The control/target pairs this suite enables, so a single filter value selects SEVERAL
/// records for some members and exactly one for others: one control on two targets, one
/// target carrying two controls, and a third control on a target of its own. Every pair is
/// distinct, because a control is enabled at most once per target. The controls are
/// entries of the catalog `EnableControl` resolves a `controlIdentifier` against
/// (`service_..._awscontroltowerapis::state::CONTROL_CATALOG`).
fn control_enablements() -> Vec<(String, String)> {
    vec![
        (control("AWS-GR_ENCRYPTED_VOLUMES"), ou("fa01")),
        (control("AWS-GR_ENCRYPTED_VOLUMES"), ou("fa02")),
        (control("AWS-GR_RESTRICT_ROOT_USER"), ou("fa01")),
        (control("AWS-GR_RESTRICTED_COMMON_PORTS"), ou("fa03")),
    ]
}

/// Enable every control and every baseline the cases narrow over.
async fn seed(router: &MultiServiceRouter) {
    for (control_identifier, target_identifier) in control_enablements() {
        let _ = enable_control(
            router,
            serde_json::json!({
                "controlIdentifier": control_identifier,
                "targetIdentifier": target_identifier,
            }),
        )
        .await;
    }
    for row in enablements() {
        let answer = enable_baseline(router, &row).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: enable-baseline expected 200, got {} body {:?}",
            row.case, answer.status, answer.body
        );
    }
}

/// A well-formed enabled-control ARN of the form this service mints, naming an enablement
/// nothing created.
fn absent_enabled_control_arn() -> String {
    format!("{}00000000000000000000", enabled_control_prefix())
}

/// RULE: every listing of this service narrows by the one shared filter rule — an omitted
/// member narrows nothing, a sent member keeps the records whose own value it holds, a
/// record with no value for a sent member is not selected, and the members narrow
/// together.
#[tokio::test]
async fn every_listing_narrows_by_the_one_shared_filter_rule() {
    let router = set_up_emulator().await;
    seed(&router).await;

    // The seeded records reach their terminal status on the emulator's own clock, so the
    // case looks again until every one of them has rather than waiting a fixed time: the
    // status and drift rows below must not race the clock.
    poll_until(
        "every seeded record is in its terminal status",
        "every seeded enablement reads SUCCEEDED",
        || async {
            let settled = |records: Vec<serde_json::Value>| {
                !records.is_empty()
                    && records
                        .iter()
                        .all(|record| at(record, "statusSummary.status") == Some("SUCCEEDED"))
            };
            settled(list(&router, ENABLED_CONTROLS, serde_json::json!({})).await)
                && settled(list(&router, ENABLED_BASELINES, serde_json::json!({})).await)
        },
    )
    .await;

    the_rule_holds_over(
        &router,
        ENABLED_CONTROLS,
        &[
            IdMember {
                member: "controlIdentifiers",
                path: "controlIdentifier",
                max_values: 1,
                // A catalogued control this fixture enables nowhere.
                absent: control("AWS-GR_RDS_STORAGE_ENCRYPTED"),
                records_report_a_value: true,
            },
            IdMember {
                member: "parentIdentifiers",
                path: "parentIdentifier",
                max_values: 1,
                absent: absent_enabled_control_arn(),
                // A control enabled directly on its target inherits from nothing, so no
                // record here has a parent for this member to match.
                records_report_a_value: false,
            },
        ],
        &[
            EnumMember {
                member: "statuses",
                path: "statusSummary.status",
                values: ENABLEMENT_STATUSES,
            },
            EnumMember {
                member: "driftStatuses",
                path: "driftStatusSummary.driftStatus",
                values: DRIFT_STATUSES,
            },
            EnumMember {
                member: "inheritanceDriftStatuses",
                path: "driftStatusSummary.types.inheritance.status",
                values: DRIFT_STATUSES,
            },
            EnumMember {
                member: "resourceDriftStatuses",
                path: "driftStatusSummary.types.resource.status",
                values: DRIFT_STATUSES,
            },
        ],
    )
    .await;

    the_rule_holds_over(
        &router,
        CONTROL_OPERATIONS,
        &[
            IdMember {
                member: "controlIdentifiers",
                path: "controlIdentifier",
                max_values: 1,
                absent: control("AWS-GR_RDS_STORAGE_ENCRYPTED"),
                records_report_a_value: true,
            },
            IdMember {
                member: "targetIdentifiers",
                path: "targetIdentifier",
                max_values: 1,
                absent: ou("ff99"),
                records_report_a_value: true,
            },
            IdMember {
                member: "enabledControlIdentifiers",
                path: "enabledControlIdentifier",
                max_values: 1,
                absent: absent_enabled_control_arn(),
                records_report_a_value: true,
            },
        ],
        &[
            EnumMember {
                member: "statuses",
                path: "status",
                values: CONTROL_OPERATION_STATUSES,
            },
            EnumMember {
                member: "controlOperationTypes",
                path: "operationType",
                values: CONTROL_OPERATION_TYPES,
            },
        ],
    )
    .await;

    // An enabled baseline carries an inheritance drift status only for the baselines
    // Control Tower assesses drift on, so this population has records with a value for
    // that member and records without — which is what makes the absent-value half of the
    // rule observable through a filter on it rather than only through `parentIdentifiers`.
    let baselines = list(&router, ENABLED_BASELINES, serde_json::json!({})).await;
    let with_drift = baselines
        .iter()
        .filter(|record| at(record, BASELINE_INHERITANCE_DRIFT_PATH).is_some())
        .count();
    assert!(
        with_drift > 0 && with_drift < baselines.len(),
        "the baseline fixture must seed both enablements that report an inheritance drift \
         status and enablements that do not, got {with_drift} of {} reporting one",
        baselines.len()
    );

    the_rule_holds_over(
        &router,
        ENABLED_BASELINES,
        &[
            IdMember {
                member: "targetIdentifiers",
                path: "targetIdentifier",
                max_values: 5,
                // A target of the right form that nothing is enabled on.
                absent: "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-zz99-9z"
                    .to_string(),
                records_report_a_value: true,
            },
            IdMember {
                member: "baselineIdentifiers",
                path: "baselineIdentifier",
                max_values: 5,
                // A baseline ARN of the catalogued form that nothing is enabled from.
                absent: "arn:aws:controltower:us-west-2::baseline/ZZZZZZZZZZZZZZZZ".to_string(),
                records_report_a_value: true,
            },
            IdMember {
                member: "parentIdentifiers",
                path: "parentIdentifier",
                max_values: 5,
                absent: "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-zz88-8z"
                    .to_string(),
                // Every enablement here is applied to its target directly, so none has a
                // parent for this member to match.
                records_report_a_value: false,
            },
        ],
        &[
            EnumMember {
                member: "statuses",
                path: "statusSummary.status",
                values: ENABLEMENT_STATUSES,
            },
            EnumMember {
                member: "inheritanceDriftStatuses",
                path: BASELINE_INHERITANCE_DRIFT_PATH,
                values: BASELINE_DRIFT_STATUSES,
            },
            EnumMember {
                member: "sourceDriftStatuses",
                path: BASELINE_SOURCE_DRIFT_PATH,
                values: BASELINE_DRIFT_STATUSES,
            },
        ],
    )
    .await;
}

/// RULE (the bound around the one above): a `filter` list longer than the modelled maximum
/// is refused at the front door, so the shared matching rule is only ever reached with a
/// list inside its modelled length — one value for every member of `EnabledControlFilter`
/// and `ControlOperationFilter`, five for `EnabledBaselineFilter`'s identifier members.
#[tokio::test]
async fn a_filter_list_past_its_modelled_maximum_is_refused() {
    let router = emulator();

    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "two controlIdentifiers, one past EnabledControlFilter's maximum of 1",
            ENABLED_CONTROLS.path,
            serde_json::json!({
                "filter": {
                    "controlIdentifiers": [
                        control("AWS-GR_ENCRYPTED_VOLUMES"),
                        control("AWS-GR_RESTRICT_ROOT_USER"),
                    ]
                }
            }),
        ),
        (
            "two statuses, one past ControlOperationFilter's maximum of 1",
            CONTROL_OPERATIONS.path,
            serde_json::json!({ "filter": { "statuses": ["SUCCEEDED", "FAILED"] } }),
        ),
        (
            "six targetIdentifiers, one past EnabledBaselineFilter's maximum of 5",
            ENABLED_BASELINES.path,
            serde_json::json!({
                "filter": {
                    "targetIdentifiers": (0..6)
                        .map(|index| format!(
                            "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-ya1{index}-1a"
                        ))
                        .collect::<Vec<String>>()
                }
            }),
        ),
    ];

    for (case, path, body) in cases {
        let refused = call(&router, path, body.clone()).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some(FRONT_DOOR_CODE)),
            "case {case:?} on {path} ({body}): expected 400 {FRONT_DOOR_CODE}, got {:?} \
             body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
    }
}
