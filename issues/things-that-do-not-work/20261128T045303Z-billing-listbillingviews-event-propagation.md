Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: d6ab12dd13fc1c951c3823d9af85ad5bf611db1c

# No BILLING_GROUP billing view is ever materialised from a Billing Conductor billing group, although the same crate already reads that store for segments

_Filed: 2026-11-28T04:53:03Z_
_Operation: ListBillingViews | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `d6ab12dd13fc1c951c3823d9af85ad5bf611db1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The operation's own modelled documentation states that an account using Billing Conductor has a `BILLING_GROUP` billing view per created billing group, and the `activeTimeRange` member states the listing rule for those views in terms of when the Billing Conductor billing group resource is active. Nothing in this crate ever stores a billing view of that type: `billing_views` has exactly two writers — the store's `Default`, which seeds ONE `PRIMARY` view, and `CreateBillingView`, which always mints `CUSTOM` — so `ListBillingViews` can never report a `BILLING_GROUP` entry, a request narrowing to `billingViewTypes: ["BILLING_GROUP"]` is always an empty page, and `GetBillingView` answers 404 for every billing-group view ARN a client could name.

This is not a feature the crate never attempts: the crate ALREADY reads Billing Conductor's own store for this very account — `state/account_relationships.rs` locks the Billing Conductor state, walks `account_associations` for the caller and resolves each membership's `billing_groups` record to its primary account — and `ListBillingViewSegments` reports that membership as the `PRO_FORMA` domain plus `billingGroupPrimaryAccountId`. So the cross-service read path from Billing Conductor into this service exists and is wired for one read, and the billing-view plane that the model says must show a view per billing group is left unwired. The result is two reads of the same emulator disagreeing about the same billing group: the segment listing reports the account is in a billing group (and shapes its domain to `PRO_FORMA`), while the billing-view listing shows no billing view for that group at all.

Because no such view can exist, the `activeTimeRange` rule quoted below has no implementation either: `matches_filters` only has a case for `is_always_active()` (true for `PRIMARY` only) and otherwise compares the view's stored `created_at`, so there is no code path that consults the billing group's active periods.

## Evidence

Smithy, `ListBillingViews` operation doc (smithy_query(query="doc", shape="ListBillingViews")):
"Every Amazon Web Services account has a unique PRIMARY billing view that represents the billing data available by default. Accounts that use Billing Conductor also have BILLING_GROUP billing views representing pro forma costs associated with each created billing group."

Smithy, `ListBillingViewsRequest$activeTimeRange` doc:
"The time range for the billing views listed. PRIMARY billing view is always listed. BILLING_GROUP billing views are listed for time ranges when the associated billing group resource in Billing Conductor is active. The time range must be within one calendar month."

The only two writers of `billing_views` — services/service-com-amazonaws-billing-awsbilling/src/state.rs:869-895:
```
        let primary = BillingView {
            arn: billing_view_arn(PRIMARY_VIEW_ID),
            ...
            billing_view_type: BILLING_VIEW_TYPE_PRIMARY.to_string(),
```
and services/service-com-amazonaws-billing-awsbilling/src/ops/create_billing_view.rs:349-366:
```
        let view = crate::state::BillingView {
            arn: arn.clone(),
            ...
            billing_view_type: crate::state::BILLING_VIEW_TYPE_CUSTOM.to_string(),
        ...
        state.billing_views.insert(arn.clone(), view);
```
(no other assignment of `billing_view_type` exists in src/: `grep BILLING_GROUP` over src/ finds the type only in the enum value list, ops/types/shared_types.rs:202, and in prose.)

The Billing Conductor read path this crate ALREADY has — services/service-com-amazonaws-billing-awsbilling/src/state/account_relationships.rs:215-239:
```
    let Some(billing_conductor) = billing_conductor::service_state(engine) else {
        return Vec::new();
    };
    let billing_conductor = billing_conductor.lock();
    let Some(memberships) = billing_conductor.account_associations.get(account_id) else {
...
                primary_account_id: billing_conductor
                    .billing_groups
                    .get(&membership.billing_group_arn)
                    .and_then(|group| group.primary_account_id.clone()),
```
and what the sibling read does with it — services/service-com-amazonaws-billing-awsbilling/src/state/billing_view_segments.rs:202-206:
```
            } else if holds.in_billing_group {
                (
                    BILLING_DOMAIN_PRO_FORMA.to_string(),
                    holds.billing_group_primary_account_id.clone(),
                )
```

The `activeTimeRange` filter, which has no BILLING_GROUP case — services/service-com-amazonaws-billing-awsbilling/src/ops/list_billing_views.rs:446-457:
```
    if let Some(range) = &input.active_time_range {
        // A view that names a resource with a lifetime is active over a period
        // once it exists: one created after the period ended was not available
        // during it. ...
        if !view.is_always_active()
            && crate::state::epoch_secs(view.created_at)
                > crate::state::date_time_secs(&range.active_before_inclusive)
        {
            return Ok(false);
        }
    }
```
with services/service-com-amazonaws-billing-awsbilling/src/state.rs:556-558:
```
    pub fn is_always_active(&self) -> bool {
        self.billing_view_type == BILLING_VIEW_TYPE_PRIMARY
    }
```

Test coverage: tests/all/billing_views.rs, tests/all/list_billing_views_account_scope.rs and tests/all/billing_view_active_time_range.rs only ever exercise the seeded `PRIMARY` view and views `CreateBillingView` minted (`CUSTOM`); no test asserts anything about a `BILLING_GROUP` view, and tests/all/billing_view_segment_relationships.rs is the only suite that seeds a Billing Conductor membership — it asserts the SEGMENT projection, never the view listing.

## Steps to Reproduce

1. Stand up an emulator mounting both this service and Billing Conductor (the harness tests/all/billing_view_segment_relationships.rs uses to seed a billing-group membership).
2. Through Billing Conductor, create a billing group whose primary account is the emulator account and associate that account to it for the current billing period.
3. Call ListBillingViewSegments with no arn: the answer reports the account's segment with domain PRO_FORMA and billingGroupPrimaryAccountId set — this service can see the billing group (src/state/account_relationships.rs:215-239).
4. Call ListBillingViews with {"billingViewTypes": ["BILLING_GROUP"]}: billingViews is empty, although the operation doc says "Accounts that use Billing Conductor also have BILLING_GROUP billing views representing pro forma costs associated with each created billing group".
5. Call ListBillingViews with {} : the answer holds only the seeded PRIMARY view (and any CUSTOM view created through the API) — no view for the billing group, so no ARN exists for GetBillingView/ListBillingViewSegments to address it by, and the activeTimeRange rule quoted for BILLING_GROUP views can never apply.
