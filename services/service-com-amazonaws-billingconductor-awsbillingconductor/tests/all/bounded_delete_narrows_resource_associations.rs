// SPDX-License-Identifier: CC0-1.0

//! What a delete bounded to a billing period range does to the resource associations
//! keyed on the item, end to end over the wire: `CreateBillingGroup` ->
//! `CreateCustomLineItem` -> `BatchAssociateResourcesToCustomLineItem` ->
//! `DeleteCustomLineItem` -> `ListCustomLineItems` /
//! `ListResourcesAssociatedToCustomLineItem`.
//!
//! RULE: a resource association is in effect only in billing periods the resource it
//! names applies in, so a write that takes billing periods away from a custom line item
//! takes the same periods out of every association keyed on that item — the ones the item
//! itself holds and the ones other percentage items hold TO it — and drops an association
//! left in effect in no period at all. So no read of a billing period reports an
//! association to an item no version of which that period reports, and a bounded delete
//! that leaves nothing of the item leaves exactly as little behind as one naming no range.
//!
//! The rows span what the rule generalises over rather than one recorded call: where the
//! deleted range falls relative to the periods the association is in effect over
//! (covering them, overlapping their tail, strictly inside them, disjoint from them,
//! beginning before them), how the deleted range is bounded (open-ended / bounded at both
//! ends), how the association's own range is bounded (none at all / open-ended / bounded /
//! starting later than the item), and whether the delete leaves the item applying
//! somewhere or empties it. Each row is read from BOTH sides of the rule: the association
//! the narrowed item holds and the one another item holds to it.

use crate::percentage_custom_line_item_resource_associations::{period, Emulator};
use serde_json::{json, Value};

/// A `CustomLineItemBillingPeriodRange`, as the model shapes it
/// (`smithy_query(query="shape",shape="CustomLineItemBillingPeriodRange")`: `@required
/// InclusiveStartBillingPeriod`, optional `ExclusiveEndBillingPeriod`).
///
/// `pub(crate)` so the sibling suite about the other half of the same rule reads the
/// store through these helpers rather than restating them (see
/// `tests/all/create_custom_line_item_association_periods.rs`).
pub(crate) fn range(inclusive_start: &str, exclusive_end: Option<&str>) -> Value {
    let mut value = json!({ "InclusiveStartBillingPeriod": inclusive_start });
    if let Some(end) = exclusive_end {
        value["ExclusiveEndBillingPeriod"] = json!(end);
    }
    value
}

/// A PERCENTAGE custom line item — the only kind resources can be associated to —
/// applying over the billing periods `billing_period_range` names.
async fn percentage_item_over(
    emulator: &Emulator,
    case: &str,
    group: &str,
    name: &str,
    billing_period_range: &Value,
) -> String {
    let (status, answered) = emulator
        .send(
            "POST",
            "/create-custom-line-item",
            &json!({
                "Name": name,
                "Description": "an item whose associations are narrowed",
                "BillingGroupArn": group,
                "ChargeDetails": { "Type": "FEE", "Percentage": { "PercentageValue": 10.0 } },
                "BillingPeriodRange": billing_period_range,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create of {name:?} to answer HTTP 200, got {status} with \
         {answered}"
    );
    answered
        .get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an Arn for {name:?}, got {answered}"))
        .to_string()
}

/// `BatchAssociateResourcesToCustomLineItem` of ONE resource to `holder`, over the
/// billing period range the row names, asserted to succeed for that resource.
async fn associate_ok(
    emulator: &Emulator,
    case: &str,
    holder: &str,
    resource: &str,
    billing_period_range: Option<Value>,
) {
    let resources = vec![resource.to_string()];
    let (status, answered) = emulator
        .associate(holder, &resources, billing_period_range)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the association of {resource} to answer HTTP 200, got {status} \
         with {answered}"
    );
    assert_eq!(
        answered.get("FailedAssociatedResources"),
        Some(&json!([])),
        "case {case:?}: expected no failed resource associating {resource}, got {answered}"
    );
}

/// `DeleteCustomLineItem` on `arn` with the `BillingPeriodRange` the row names.
async fn delete(emulator: &Emulator, arn: &str, billing_period_range: &Value) -> (u16, Value) {
    let mut body = json!({ "Arn": arn });
    if !billing_period_range.is_null() {
        body["BillingPeriodRange"] = billing_period_range.clone();
    }
    emulator
        .send("POST", "/delete-custom-line-item", &body)
        .await
}

/// The item as `ListCustomLineItems` reports it for `billing_period`, or `None` when that
/// period reports no version of it.
pub(crate) async fn item_in(
    emulator: &Emulator,
    case: &str,
    arn: &str,
    billing_period: &str,
) -> Option<Value> {
    let (status, body) = emulator
        .send(
            "POST",
            "/list-custom-line-items",
            &json!({ "BillingPeriod": billing_period, "Filters": { "Arns": [arn] } }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the item read for {billing_period} to answer HTTP 200, got \
         {status} with {body}"
    );
    body.get("CustomLineItems")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .cloned()
}

/// The ARNs `ListResourcesAssociatedToCustomLineItem` reports for `holder` in
/// `billing_period`, or `None` when the store holds no such item at all — the read's
/// modelled 404, which is how an item a delete removed outright answers.
pub(crate) async fn associated_arns(
    emulator: &Emulator,
    case: &str,
    holder: &str,
    billing_period: &str,
) -> Option<Vec<String>> {
    let (status, body) = emulator
        .send(
            "POST",
            "/list-resources-associated-to-custom-line-item",
            &json!({ "Arn": holder, "BillingPeriod": billing_period, "MaxResults": 100 }),
        )
        .await;
    if status == 404 {
        return None;
    }
    assert_eq!(
        status, 200,
        "case {case:?}: expected the association read of {holder} for {billing_period} to answer \
         HTTP 200, got {status} with {body}"
    );
    Some(
        body.get("AssociatedResources")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|element| element.get("Arn").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
    )
}

/// The `AssociationSize` `ListCustomLineItems` reports for `arn` in `billing_period`, or
/// `None` when that period reports no version of the item.
pub(crate) async fn association_size(
    emulator: &Emulator,
    case: &str,
    arn: &str,
    billing_period: &str,
) -> Option<i64> {
    item_in(emulator, case, arn, billing_period)
        .await
        .and_then(|element| element.get("AssociationSize").and_then(Value::as_i64))
}

/// One row: how the narrowed item and the associations keyed on it were set up, the
/// range the delete names, and what every probed billing period must report afterwards.
struct Row {
    case: &'static str,
    /// The `BillingPeriodRange` the item the delete narrows is created with.
    item_range: Value,
    /// The `BillingPeriodRange` the two associating calls name — `None` for the call
    /// that names none, which is in effect from the period it lands in on.
    association_range: Option<Value>,
    /// The `BillingPeriodRange` the delete names.
    deleted: Value,
    /// (the billing period a read asks about, does a read report the narrowed item
    /// there, is an association keyed on it in effect there)
    probes: Vec<(String, bool, bool)>,
}

/// THE RULE: the billing periods a delete takes from a custom line item come out of every
/// association keyed on it, so a period that reports no version of the item reports no
/// association to it either — while the periods the delete did not name keep exactly the
/// associations they had.
#[tokio::test]
async fn a_bounded_delete_takes_the_deleted_periods_out_of_every_association_keyed_on_the_item() {
    let rows: Vec<Row> = vec![
        Row {
            case: "an open-ended delete of the periods the association covers",
            item_range: range(&period(-1), None),
            association_range: Some(range(&period(-1), None)),
            deleted: range(&period(0), None),
            probes: vec![
                (period(-1), true, true),
                (period(0), false, false),
                (period(1), false, false),
                (period(6), false, false),
            ],
        },
        Row {
            case: "a bounded delete leaving periods on either side of it",
            item_range: range(&period(-1), None),
            association_range: Some(range(&period(-1), None)),
            deleted: range(&period(0), Some(&period(2))),
            probes: vec![
                (period(-1), true, true),
                (period(0), false, false),
                (period(1), false, false),
                (period(2), true, true),
                (period(6), true, true),
            ],
        },
        Row {
            case: "a delete of a period the association never covered",
            item_range: range(&period(-1), None),
            association_range: None,
            deleted: range(&period(-1), Some(&period(0))),
            probes: vec![
                (period(-1), false, false),
                (period(0), true, true),
                (period(3), true, true),
            ],
        },
        Row {
            case: "a delete that empties the item",
            item_range: range(&period(-1), Some(&period(1))),
            association_range: Some(range(&period(-1), None)),
            deleted: range(&period(-1), None),
            probes: vec![
                (period(-1), false, false),
                (period(0), false, false),
                (period(2), false, false),
            ],
        },
        Row {
            case: "a delete strictly inside the item's and the association's periods",
            item_range: range(&period(-1), Some(&period(6))),
            association_range: Some(range(&period(-1), Some(&period(4)))),
            deleted: range(&period(0), Some(&period(1))),
            probes: vec![
                (period(-1), true, true),
                (period(0), false, false),
                (period(1), true, true),
                (period(3), true, true),
                // The association's own exclusive end, which the delete did not move:
                // the item applies there and holds no association.
                (period(4), true, false),
                (period(6), false, false),
            ],
        },
        Row {
            case: "an association beginning later than the item the delete narrows",
            item_range: range(&period(-1), None),
            association_range: Some(range(&period(2), None)),
            deleted: range(&period(0), Some(&period(3))),
            probes: vec![
                (period(-1), true, false),
                (period(0), false, false),
                (period(2), false, false),
                (period(3), true, true),
                (period(7), true, true),
            ],
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let group = emulator.billing_group("payer").await;
        // The item the delete narrows is a PERCENTAGE item, so it holds an association of
        // its own as well as being the resource another item's association names: both
        // sides of the rule are read off the one delete.
        let narrowed =
            percentage_item_over(&emulator, case, &group, "narrowed", &row.item_range).await;
        let open = range(&period(-1), None);
        let held = percentage_item_over(&emulator, case, &group, "held", &open).await;
        let holder = percentage_item_over(&emulator, case, &group, "holder", &open).await;
        associate_ok(
            &emulator,
            case,
            &narrowed,
            &held,
            row.association_range.clone(),
        )
        .await;
        associate_ok(
            &emulator,
            case,
            &holder,
            &narrowed,
            row.association_range.clone(),
        )
        .await;
        let charge_before = item_in(&emulator, case, &holder, &period(0))
            .await
            .and_then(|element| element.get("ChargeDetails").cloned());

        let (status, answered) = delete(&emulator, &narrowed, &row.deleted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer HTTP 200, got {status} with {answered}"
        );
        assert_eq!(
            answered.get("Arn").and_then(Value::as_str),
            Some(narrowed.as_str()),
            "case {case:?}: expected the delete to answer the item's ARN, got {answered}"
        );

        for (billing_period, item_reported, associated) in &row.probes {
            let reported = item_in(&emulator, case, &narrowed, billing_period)
                .await
                .is_some();
            assert_eq!(
                reported, *item_reported,
                "case {case:?}: expected the read for {billing_period:?} to report the narrowed \
                 item: {item_reported}, got {reported}"
            );

            // The association ANOTHER percentage item holds to the narrowed item.
            let listed = associated_arns(&emulator, case, &holder, billing_period)
                .await
                .unwrap_or_default();
            let holds = listed.contains(&narrowed);
            assert_eq!(
                holds, *associated,
                "case {case:?}: expected the holder's association to the narrowed item to be in \
                 effect in {billing_period:?}: {associated}, got {holds} from {listed:?}"
            );
            let size = association_size(&emulator, case, &holder, billing_period).await;
            assert_eq!(
                size,
                Some(i64::from(*associated)),
                "case {case:?}: expected the holder's AssociationSize in {billing_period:?} to \
                 count its one association only where it is in effect, got {size:?}"
            );

            // The association the narrowed item HOLDS itself, over the same range: an item
            // a period reports no version of is not there to hold a resource in it.
            let held_listed = associated_arns(&emulator, case, &narrowed, billing_period).await;
            let holds_held = held_listed
                .clone()
                .unwrap_or_default()
                .contains(&held);
            assert_eq!(
                holds_held, *associated,
                "case {case:?}: expected the narrowed item's own association to be in effect in \
                 {billing_period:?}: {associated}, got {holds_held} from {held_listed:?}"
            );

            // THE RULE, stated as the invariant the two reads have to agree on.
            assert!(
                *item_reported || !holds && !holds_held,
                "case {case:?}: {billing_period:?} reports no version of the narrowed item, so no \
                 read of that period may report an association keyed on it, got {listed:?} and \
                 {held_listed:?}"
            );

            // An item the delete did not name keeps every billing period it applied in.
            assert!(
                item_in(&emulator, case, &held, billing_period)
                    .await
                    .is_some(),
                "case {case:?}: expected the item the delete did not name to stay readable in \
                 {billing_period:?}"
            );
        }

        // Nothing else about the item that merely held an association to the narrowed one
        // changed.
        let charge_after = item_in(&emulator, case, &holder, &period(0))
            .await
            .and_then(|element| element.get("ChargeDetails").cloned());
        assert_eq!(
            charge_after, charge_before,
            "case {case:?}: expected the holder's charge details to be unchanged, got \
             {charge_after:?}"
        );
    }
}

/// THE RULE where the two deletes meet: a bounded delete covering every billing period
/// the item applied in leaves exactly what a delete naming no range leaves, since both
/// leave no period reporting a version of the item.
#[tokio::test]
async fn a_bounded_delete_that_empties_the_item_leaves_what_the_unbounded_delete_leaves() {
    // (case, the range the narrowed item is created with, the `BillingPeriodRange` the
    // delete names) — `Value::Null` for the request that names none.
    let deletes: [(&str, Value, Value); 3] = [
        ("no range at all", range(&period(-1), None), Value::Null),
        (
            "a range covering every period the item applied in",
            range(&period(-1), None),
            range(&period(-1), None),
        ),
        (
            "a range bounded exactly at the item's own end",
            range(&period(-1), Some(&period(9))),
            range(&period(-1), Some(&period(9))),
        ),
    ];

    for (case, item_range, deleted) in deletes {
        let emulator = Emulator::new();
        let group = emulator.billing_group("payer").await;
        let open = range(&period(-1), None);
        let narrowed = percentage_item_over(&emulator, case, &group, "narrowed", &item_range).await;
        let holder = percentage_item_over(&emulator, case, &group, "holder", &open).await;
        associate_ok(&emulator, case, &holder, &narrowed, Some(open.clone())).await;

        let (status, answered) = delete(&emulator, &narrowed, &deleted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer HTTP 200, got {status} with {answered}"
        );

        for billing_period in [period(-1), period(0), period(4), period(9)] {
            let listed = associated_arns(&emulator, case, &holder, &billing_period)
                .await
                .unwrap_or_default();
            assert!(
                listed.is_empty(),
                "case {case:?}: expected the holder to report no association in \
                 {billing_period:?}, got {listed:?}"
            );
            let size = association_size(&emulator, case, &holder, &billing_period).await;
            assert_eq!(
                size,
                Some(0),
                "case {case:?}: expected the holder's AssociationSize in {billing_period:?} to be \
                 0, got {size:?}"
            );
            assert!(
                item_in(&emulator, case, &narrowed, &billing_period)
                    .await
                    .is_none(),
                "case {case:?}: expected no read to report the deleted item in {billing_period:?}"
            );
        }
    }
}

/// THE RULE on a REFUSED delete: it writes nothing, so the associations keyed on the item
/// are in effect in exactly the periods they were.
#[tokio::test]
async fn a_refused_bounded_delete_leaves_every_association_keyed_on_the_item_in_effect() {
    // (case, the `BillingPeriodRange` the refused delete names) — a start outside the
    // current and previous billing periods, and a range covering no period at all.
    let refusals: [(&str, Value); 4] = [
        ("a start two periods back", range(&period(-2), None)),
        ("a start a year back", range(&period(-12), None)),
        ("a start in a later period", range(&period(3), None)),
        (
            "a range whose end is its own start",
            range(&period(0), Some(&period(0))),
        ),
    ];

    for (case, deleted) in refusals {
        let emulator = Emulator::new();
        let group = emulator.billing_group("payer").await;
        let open = range(&period(-1), None);
        let narrowed = percentage_item_over(&emulator, case, &group, "narrowed", &open).await;
        let held = percentage_item_over(&emulator, case, &group, "held", &open).await;
        let holder = percentage_item_over(&emulator, case, &group, "holder", &open).await;
        associate_ok(&emulator, case, &narrowed, &held, Some(open.clone())).await;
        associate_ok(&emulator, case, &holder, &narrowed, Some(open.clone())).await;

        let (status, body) = delete(&emulator, &narrowed, &deleted).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            body.get("__type").and_then(Value::as_str),
            Some("ValidationException"),
            "case {case:?}: expected the modelled ValidationException, got {body}"
        );

        for billing_period in [period(-1), period(0), period(5)] {
            let listed = associated_arns(&emulator, case, &holder, &billing_period)
                .await
                .unwrap_or_default();
            assert_eq!(
                listed,
                vec![narrowed.clone()],
                "case {case:?}: expected the holder's association to survive the refusal in \
                 {billing_period:?}, got {listed:?}"
            );
            let held_listed = associated_arns(&emulator, case, &narrowed, &billing_period)
                .await
                .unwrap_or_default();
            assert_eq!(
                held_listed,
                vec![held.clone()],
                "case {case:?}: expected the narrowed item's own association to survive the \
                 refusal in {billing_period:?}, got {held_listed:?}"
            );
            assert!(
                item_in(&emulator, case, &narrowed, &billing_period)
                    .await
                    .is_some(),
                "case {case:?}: expected the item to stay readable in {billing_period:?} after \
                 the refusal"
            );
        }
    }
}
