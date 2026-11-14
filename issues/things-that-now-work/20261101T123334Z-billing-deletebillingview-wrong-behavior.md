Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0b7c7f06678596bb74c7aa80db04e65e20adb2f

# A forced delete removes the view but leaves every derived view's source list pointing at it, the exact dangling association AssociateSourceViews refuses to create

_Filed: 2026-11-01T12:33:34Z_
_Operation: DeleteBillingView | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0b7c7f06678596bb74c7aa80db04e65e20adb2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteBillingView` computes the list of views that draw from the target (`state.derived_view_arns(&input.arn)`) purely to decide whether `force` is required, then throws that list away and calls `state.forget_view(&input.arn)`, which removes only the view record and the idempotency tokens naming it. The surviving derived views keep the deleted ARN in their stored `source_views`, so the store is left holding associations that point at nothing:

* `ListSourceViewsForBillingView` on a derived view reports the deleted ARN verbatim (it returns the stored list unchanged), and `GetBillingView` counts it in `sourceViewCount`.
* The reported ARN resolves to `ResourceNotFoundException` on `GetBillingView`, so one read of this service contradicts another.
* The dangling entry still occupies one of the derived view's at-most-ten source slots, so a caller can be refused with `ServiceQuotaExceededException` for a slot held by a view that no longer exists.

This is the state the crate's own writer refuses to produce: `AssociateSourceViews` rejects a source ARN that names no stored view precisely because "the association would point at nothing". The delete path has the helper it needs (it already computed `derived`) and uses it for nothing but the conflict check.

## Evidence

The writer that forbids the state — services/service-com-amazonaws-billing-awsbilling/src/ops/associate_source_views.rs:241-247:
```
        // Every source must name a view this account holds, or the association
        // would point at nothing.
        for source in &input.source_views {
            if !state.billing_views.contains_key(source) {
                return Err(not_found(source, "sourceViews"));
            }
        }
```
The delete path that creates it — services/service-com-amazonaws-billing-awsbilling/src/ops/delete_billing_view.rs:194-210:
```
        let derived = state.derived_view_arns(&input.arn);
        if !input.force && !derived.is_empty() {
            return Err(DeleteBillingViewErrorModeled::ConflictException(ConflictException {
...
        // Removing the view also reclaims the idempotency tokens that named it,
        // so a create/delete loop frees everything it allocated.
        state.forget_view(&input.arn);
```
`forget_view` touches nothing else — services/service-com-amazonaws-billing-awsbilling/src/state.rs:662-669:
```
    /// Drop the view at `arn` and every idempotency token that named it, so a
    /// create/delete loop reclaims everything it allocated instead of leaving
    /// entries behind that no API can observe or free.
    pub fn forget_view(&mut self, arn: &str) -> Option<BillingView> {
        let removed = self.billing_views.remove(arn);
        self.idempotency_tokens.retain(|_, stored| stored != arn);
        removed
    }
```
And the read that then reports the dangling ARN — services/service-com-amazonaws-billing-awsbilling/src/ops/list_source_views_for_billing_view.rs:246-251: "source_views: page.to_vec()" with the comment "Derived on every read from the one stored copy of the view, so a list cannot disagree with what `GetBillingView` reports as `sourceViewCount`."

Test coverage: services/service-com-amazonaws-billing-awsbilling/tests/all/billing_views.rs:759-812 (`deleting_a_source_view_needs_force`) exercises exactly this scenario — it creates a source view, a derived view over it, and deletes the source with `force: true` — but asserts only that the deleted view is gone (line 806-811); it never reads the derived view's source list back, so nothing pins the stale entry.

## Steps to Reproduce

1. Boot the emulator and point an awsJson1_0 `billing` client at it.
2. CreateBillingView(name="src", sourceViews=["arn:aws:billing::<account>:billingview/primary"]) -> note the minted ARN S.
3. CreateBillingView(name="derived", sourceViews=[S]) -> note the minted ARN D.
4. DeleteBillingView(arn=S, force=true) -> 200.
5. ListSourceViewsForBillingView(arn=D) -> observe S still reported, and GetBillingView(arn=D).billingView.sourceViewCount still counts it.
6. GetBillingView(arn=S) -> 404 ResourceNotFoundException, so the association reported in step 5 points at nothing — the state AssociateSourceViews refuses to create.
