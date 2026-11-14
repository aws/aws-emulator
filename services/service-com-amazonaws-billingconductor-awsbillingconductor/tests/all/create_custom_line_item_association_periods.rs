// SPDX-License-Identifier: CC0-1.0

//! The billing periods `CreateCustomLineItem` puts an `AssociatedValues` entry in effect
//! over, end to end over the wire: `CreateBillingGroup` -> `CreateCustomLineItem`
//! (-> `DeleteCustomLineItem`) -> `CreateCustomLineItem` with
//! `ChargeDetails.Percentage.AssociatedValues` -> `ListCustomLineItems` /
//! `ListCustomLineItemVersions` / `ListResourcesAssociatedToCustomLineItem`.
//!
//! RULE: a resource association is in effect only in billing periods the resource it
//! names is there to be associated in, so the span this create DERIVES from the new
//! item's own `BillingPeriodRange` is stored bounded to the periods the named resource
//! applies in, and an entry naming a resource that applies in none of them is refused
//! with the modelled `ValidationException`. No read of a billing period can then report
//! an association to an item it reports no version of — the same invariant a bounded
//! delete maintains from the other end
//! (`tests/all/bounded_delete_narrows_resource_associations.rs`), whose helpers these
//! rows read the store through.
//!
//! The rows span what the rule generalises over rather than one recorded create: how the
//! new item's own range is bounded (none at all / open-ended / bounded at both ends),
//! where the named item's periods fall relative to it (the same periods, stopping
//! earlier, starting later, with a hole carved out of the middle, sharing none at all),
//! the kind of charge the named item carries (flat / percentage), and the class of
//! resource the entry names (custom line item / billing group) — never along a renaming.

use crate::bounded_delete_narrows_resource_associations::{
    associated_arns, association_size, item_in, range,
};
use crate::cost_report_charges_of_the_generation_in_effect::month_after;
use crate::percentage_custom_line_item_resource_associations::{period, Emulator};
use serde_json::{json, Value};

/// A custom line item of `group` carrying `charge_details` and applying over the billing
/// period range `billing_period_range` names (`None` for the create that names none).
/// The status and body are handed back, so a row expecting a refusal reads them off the
/// same call a row expecting a create does.
async fn create_item(
    emulator: &Emulator,
    group: &str,
    name: &str,
    charge_details: Value,
    billing_period_range: Option<Value>,
) -> (u16, Value) {
    let mut body = json!({
        "Name": name,
        "Description": "an item whose associations are bounded",
        "BillingGroupArn": group,
        "ChargeDetails": charge_details,
    });
    if let Some(billing_period_range) = billing_period_range {
        body["BillingPeriodRange"] = billing_period_range;
    }
    emulator
        .send("POST", "/create-custom-line-item", &body)
        .await
}

/// The ARN of the NAMED item: the child an `AssociatedValues` entry of the item created
/// after it names, applying over `billing_period_range`. `flat` picks the charge kind, so
/// the rule is read over both kinds of child.
async fn named_item(
    emulator: &Emulator,
    case: &str,
    group: &str,
    name: &str,
    billing_period_range: &Value,
    flat: bool,
) -> String {
    let charge_details = if flat {
        json!({ "Type": "FEE", "Flat": { "ChargeValue": 100.0 } })
    } else {
        json!({ "Type": "FEE", "Percentage": { "PercentageValue": 25.0 } })
    };
    let (status, body) = create_item(
        emulator,
        group,
        name,
        charge_details,
        Some(billing_period_range.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create of the named item to answer HTTP 200, got {status} \
         with {body}"
    );
    arn_of(&body, case)
}

/// The PARENT: a percentage custom line item applying over `billing_period_range` whose
/// `ChargeDetails.Percentage.AssociatedValues` names `associated`.
async fn create_parent(
    emulator: &Emulator,
    group: &str,
    name: &str,
    billing_period_range: Option<Value>,
    associated: &[String],
) -> (u16, Value) {
    create_item(
        emulator,
        group,
        name,
        json!({
            "Type": "FEE",
            "Percentage": { "PercentageValue": 10.0, "AssociatedValues": associated },
        }),
        billing_period_range,
    )
    .await
}

/// `DeleteCustomLineItem` bounded to the billing period range `billing_period_range`
/// names, asserted to be applied: what carves a hole in the periods the named item
/// applies in BEFORE the parent is created.
async fn delete_periods(emulator: &Emulator, case: &str, arn: &str, billing_period_range: &Value) {
    let (status, body) = emulator
        .send(
            "POST",
            "/delete-custom-line-item",
            &json!({ "Arn": arn, "BillingPeriodRange": billing_period_range }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the bounded delete to answer HTTP 200, got {status} with {body}"
    );
}

/// The `Arn` a create answered with.
fn arn_of(body: &Value, case: &str) -> String {
    body.get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an Arn member, got {body}"))
        .to_string()
}

/// The error code of a refusal, as the protocol carries it.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .or_else(|| body.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `Name`s of the `Fields` a refusal names as at fault.
fn field_names(body: &Value) -> Vec<String> {
    body.get("Fields")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|field| field.get("Name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Does `ListCustomLineItemVersions` report a version of `arn` covering
/// `billing_period`? The other reader of the periods an item applies in, so the two
/// agree about every probed period. The filter's own range is half-open — its
/// `StartBillingPeriod` inclusive and its `EndBillingPeriod` exclusive
/// (`smithy_query(query="shape",shape="ListCustomLineItemVersionsBillingPeriodRangeFilter")`)
/// — so the range that covers one period ends in the month after it.
async fn version_in(emulator: &Emulator, case: &str, arn: &str, billing_period: &str) -> bool {
    let (status, body) = emulator
        .send(
            "POST",
            "/list-custom-line-item-versions",
            &json!({ "Arn": arn, "Filters": { "BillingPeriodRange": {
                "StartBillingPeriod": billing_period,
                "EndBillingPeriod": month_after(billing_period),
            } } }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the version read for {billing_period} to answer HTTP 200, got \
         {status} with {body}"
    );
    !body
        .get("CustomLineItemVersions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .is_empty()
}

/// One row: how the NAMED item's periods were set up, the range the parent create names,
/// and what every probed billing period must report afterwards.
struct Row {
    case: &'static str,
    /// The `BillingPeriodRange` the named item is created with.
    named_range: Value,
    /// A bounded `DeleteCustomLineItem` applied to the named item BEFORE the parent is
    /// created, carving a hole in the periods it applies in; `None` where there is none.
    named_hole: Option<Value>,
    /// The `BillingPeriodRange` the parent create names — the span it derives the
    /// association from. `None` for the create that names none.
    parent_range: Option<Value>,
    /// Is the named item a FLAT item, or a percentage one?
    flat: bool,
    /// (the billing period a read asks about, does a read report a version of the named
    /// item there, is the parent's association to it in effect there)
    probes: Vec<(String, bool, bool)>,
}

/// THE RULE: the association a create stores for an `AssociatedValues` entry is in effect
/// in exactly the billing periods the new item's own range and the named item's own
/// periods SHARE — so a period reporting no version of the named item reports no
/// association to it, and every period both of them apply in does.
#[tokio::test]
async fn a_created_association_is_in_effect_only_where_the_named_item_applies() {
    let rows: Vec<Row> = vec![
        Row {
            case: "a named item applying in every period the new item does",
            named_range: range(&period(-1), None),
            named_hole: None,
            parent_range: Some(range(&period(-1), None)),
            flat: true,
            probes: vec![
                (period(-1), true, true),
                (period(0), true, true),
                (period(6), true, true),
            ],
        },
        Row {
            case: "a named item that stops applying before the new item does",
            named_range: range(&period(-1), Some(&period(0))),
            named_hole: None,
            parent_range: Some(range(&period(-1), None)),
            flat: true,
            probes: vec![
                (period(-1), true, true),
                (period(0), false, false),
                (period(1), false, false),
                (period(12), false, false),
            ],
        },
        Row {
            case: "a named item that starts applying later than the new item",
            named_range: range(&period(0), None),
            named_hole: None,
            parent_range: Some(range(&period(-1), None)),
            flat: false,
            probes: vec![
                (period(-2), false, false),
                (period(-1), false, false),
                (period(0), true, true),
                (period(3), true, true),
            ],
        },
        Row {
            case: "a new item bounded inside the periods the named item applies in",
            named_range: range(&period(-1), None),
            named_hole: None,
            parent_range: Some(range(&period(-1), Some(&period(1)))),
            flat: true,
            probes: vec![
                (period(-1), true, true),
                (period(0), true, true),
                // The new item's own exclusive end: the named item applies there and the
                // association does not reach it.
                (period(1), true, false),
                (period(4), true, false),
            ],
        },
        Row {
            case: "a new item with no range at all beside a named item bounded both ways",
            named_range: range(&period(-1), Some(&period(1))),
            named_hole: None,
            parent_range: None,
            flat: false,
            probes: vec![
                // The period before the one the create landed in: the named item applies
                // there, the new item does not, so no association of it is in effect.
                (period(-1), true, false),
                (period(0), true, true),
                (period(1), false, false),
                (period(2), false, false),
            ],
        },
        Row {
            case: "a named item with a hole carved out of the periods it applies in",
            named_range: range(&period(-1), None),
            named_hole: Some(range(&period(0), Some(&period(1)))),
            parent_range: Some(range(&period(-1), None)),
            flat: true,
            probes: vec![
                (period(-1), true, true),
                (period(0), false, false),
                (period(1), true, true),
                (period(5), true, true),
            ],
        },
        Row {
            case: "a named item whose periods were cut back to the previous one",
            named_range: range(&period(-1), None),
            named_hole: Some(range(&period(0), None)),
            parent_range: Some(range(&period(-1), None)),
            flat: false,
            probes: vec![
                (period(-1), true, true),
                (period(0), false, false),
                (period(2), false, false),
            ],
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let group = emulator.billing_group("payer").await;
        let named = named_item(&emulator, case, &group, "named", &row.named_range, row.flat).await;
        if let Some(hole) = &row.named_hole {
            delete_periods(&emulator, case, &named, hole).await;
        }

        let (status, body) = create_parent(
            &emulator,
            &group,
            "parent",
            row.parent_range.clone(),
            std::slice::from_ref(&named),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create naming the item to answer HTTP 200, got {status} \
             with {body}"
        );
        let parent = arn_of(&body, case);

        for (billing_period, named_reported, associated) in &row.probes {
            let reported = item_in(&emulator, case, &named, billing_period)
                .await
                .is_some();
            assert_eq!(
                reported, *named_reported,
                "case {case:?}: expected the read of {billing_period:?} to report the named \
                 item: {named_reported}, got {reported}"
            );
            // The two readers of the periods an item applies in agree. A reported
            // version carries ONE half-open range (`StartBillingPeriod` /
            // `EndBillingPeriod`), so a generation a bounded delete carved a hole into is
            // reported by the range it applies WITHIN and the period-scoped read above is
            // the exact one — the rows with a hole are left to it.
            if row.named_hole.is_none() {
                let versioned = version_in(&emulator, case, &named, billing_period).await;
                assert_eq!(
                    versioned, *named_reported,
                    "case {case:?}: expected the version read of {billing_period:?} to report \
                     the named item: {named_reported}, got {versioned}"
                );
            }

            let listed = associated_arns(&emulator, case, &parent, billing_period)
                .await
                .unwrap_or_default();
            let holds = listed.contains(&named);
            assert_eq!(
                holds, *associated,
                "case {case:?}: expected the association to the named item to be in effect in \
                 {billing_period:?}: {associated}, got {holds} from {listed:?}"
            );

            // THE RULE, as the invariant the two reads have to agree on.
            assert!(
                *named_reported || !holds,
                "case {case:?}: {billing_period:?} reports no version of the named item, so no \
                 read of that period may report an association to it, got {listed:?}"
            );

            // `AssociationSize` counts the associations in effect in the period it is
            // read for, and a period reporting no version of the PARENT reports no size
            // at all — so the two period-scoped reads of the one association agree.
            let parent_reported = item_in(&emulator, case, &parent, billing_period)
                .await
                .is_some();
            let size = association_size(&emulator, case, &parent, billing_period).await;
            assert_eq!(
                size,
                parent_reported.then_some(i64::from(*associated)),
                "case {case:?}: expected the parent's AssociationSize in {billing_period:?} to \
                 count its one association only where it is in effect, got {size:?}"
            );
        }
    }
}

/// THE RULE where it leaves nothing: an entry naming an item that applies in none of the
/// billing periods the item being created applies in is refused with the modelled
/// `ValidationException`, naming the member at fault, and nothing is minted — rather than
/// stored as an association in effect in no period at all.
#[tokio::test]
async fn a_create_naming_an_item_that_shares_no_period_with_it_is_refused() {
    // (case, the range the named item is created with, a bounded delete applied to it
    // first, the range the create names)
    let rows: Vec<(&str, Value, Option<Value>, Option<Value>)> = vec![
        (
            "a named item that applies in the previous period only",
            range(&period(-1), Some(&period(0))),
            None,
            Some(range(&period(0), None)),
        ),
        (
            "a named item that applies in the previous period only, the create naming no range",
            range(&period(-1), Some(&period(0))),
            None,
            None,
        ),
        (
            "a named item that only starts applying in the current period",
            range(&period(0), None),
            None,
            Some(range(&period(-1), Some(&period(0)))),
        ),
        (
            "a named item cut back to the previous period by a bounded delete",
            range(&period(-1), None),
            Some(range(&period(0), None)),
            Some(range(&period(0), None)),
        ),
        (
            "a named item whose only remaining period the create's range misses",
            range(&period(-1), None),
            Some(range(&period(-1), Some(&period(0)))),
            Some(range(&period(-1), Some(&period(0)))),
        ),
    ];

    for (case, named_range, named_hole, parent_range) in rows {
        let emulator = Emulator::new();
        let group = emulator.billing_group("payer").await;
        let named = named_item(&emulator, case, &group, "named", &named_range, true).await;
        if let Some(hole) = &named_hole {
            delete_periods(&emulator, case, &named, hole).await;
        }

        let (status, body) =
            create_parent(&emulator, &group, "parent", parent_range, std::slice::from_ref(&named))
                .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected the modelled validation error, got {body}"
        );
        assert_eq!(
            body.get("Reason").and_then(Value::as_str),
            Some("ILLEGAL_CHILD_ASSOCIATE_RESOURCE"),
            "case {case:?}: expected the modelled reason for a resource that cannot be a child \
             of this item, got {body}"
        );
        assert_eq!(
            field_names(&body),
            vec!["ChargeDetails.Percentage.AssociatedValues".to_string()],
            "case {case:?}: expected the member at fault named in Fields, got {body}"
        );
        assert_eq!(
            body.get("Arn"),
            None,
            "case {case:?}: a refused create answers no ARN, got {body}"
        );

        // Nothing was minted: no period reports an item named after the refused create,
        // and the named item kept every period it applied in.
        for billing_period in [period(-1), period(0), period(3)] {
            let listed = emulator
                .send(
                    "POST",
                    "/list-custom-line-items",
                    &json!({
                        "BillingPeriod": billing_period,
                        "Filters": { "Names": ["parent"] },
                    }),
                )
                .await
                .1;
            assert_eq!(
                listed.get("CustomLineItems"),
                Some(&json!([])),
                "case {case:?}: expected the refused create to mint no item readable in \
                 {billing_period:?}, got {listed}"
            );
        }
        assert!(
            item_in(&emulator, case, &named, &period(-1))
                .await
                .is_some()
                || item_in(&emulator, case, &named, &period(0)).await.is_some(),
            "case {case:?}: expected the named item itself to survive the refusal"
        );
    }
}

/// One row of the billing-group table: the range the create names and the billing
/// periods the entry naming the group is, and is not, in effect in.
struct GroupRow {
    case: &'static str,
    /// The `BillingPeriodRange` the create names; `None` for the create that names none.
    parent_range: Option<Value>,
    /// The billing periods the association to the group is in effect in.
    in_effect: Vec<String>,
    /// The billing periods it is not — the ones the new item itself applies in none of.
    out_of_effect: Vec<String>,
}

/// THE RULE's boundary: a BILLING GROUP is not an item whose own billing periods bound an
/// association to it. It carries the charges of the accounts it bills in every period of
/// a bill a write may still land in — which is why an item may be created into the
/// previous billing period of a group created in this one — so an entry naming the group
/// is in effect over every period the item being created applies in.
#[tokio::test]
async fn an_entry_naming_the_billing_group_is_in_effect_wherever_the_new_item_applies() {
    let rows: Vec<GroupRow> = vec![
        GroupRow {
            case: "a create reaching back into the previous billing period",
            parent_range: Some(range(&period(-1), None)),
            in_effect: vec![period(-1), period(0), period(4)],
            out_of_effect: vec![period(-2), period(-12)],
        },
        GroupRow {
            case: "a create naming no range at all",
            parent_range: None,
            in_effect: vec![period(0), period(2)],
            out_of_effect: vec![period(-1)],
        },
        GroupRow {
            case: "a create bounded to the previous billing period alone",
            parent_range: Some(range(&period(-1), Some(&period(0)))),
            in_effect: vec![period(-1)],
            out_of_effect: vec![period(0), period(1)],
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let group = emulator.billing_group("payer").await;
        let (status, body) = create_parent(
            &emulator,
            &group,
            "parent",
            row.parent_range,
            std::slice::from_ref(&group),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create naming its own billing group to answer HTTP 200, \
             got {status} with {body}"
        );
        let parent = arn_of(&body, case);

        for (billing_period, expected) in row
            .in_effect
            .iter()
            .map(|billing_period| (billing_period, true))
            .chain(
                row.out_of_effect
                    .iter()
                    .map(|billing_period| (billing_period, false)),
            )
        {
            let listed = associated_arns(&emulator, case, &parent, billing_period)
                .await
                .unwrap_or_default();
            assert_eq!(
                listed.contains(&group),
                expected,
                "case {case:?}: expected the association to the billing group to be in effect in \
                 {billing_period:?}: {expected}, got {listed:?}"
            );
        }
    }
}
