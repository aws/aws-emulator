Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ddedefe2ba7626fc434a3b439556791987e3d078

# A resourceTags entry that omits the optional value is stored as the empty string, so the tag reads back carrying a value the caller never sent

_Filed: 2026-11-21T03:22:16Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ddedefe2ba7626fc434a3b439556791987e3d078`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ResourceTag$value` is NOT `@required` in the model (it targets `ResourceTagValue`, length 0..256), so a request may carry `{"key": "team"}` with no value at all — and because `""` is itself a legal value of that member (length minimum 0), "no value" and "the empty value" are two different inputs on the wire.

The one place this crate collects a requested tag set, `state::requested_tag_set`, erases that difference: it inserts `tag.value.clone().unwrap_or_default()` into a `BTreeMap<String, String>`, which has no way to hold "no value". `ListTagsForResource` then rebuilds every stored tag as `value: Some(value.clone())`, so a `TagResource` (or `CreateBillingView`, which collects through the same helper) request that omitted `value` reads back as `value: ""` — a value the caller never submitted — and a client cannot distinguish the tag it created from one created with an explicit empty value.

The crate preserves exactly this distinction for the other optional request string it stores on the same record: `BillingView::description` is kept as an `Option` by `CreateBillingView` and reported by `GetBillingView` as `view.description.clone()`, so an omitted description comes back as an absent member rather than `""`. `GetCredits` states the same convention in words for its own optional members ("An empty stored list is reported as an absent member rather than an empty one ... an absent member is how 'nothing recorded' is reported"). The tagging plane is the one surface that substitutes a value instead, and the output member it fills (`ResourceTag$value` in `ListTagsForResourceResponse`) is optional too, so nothing forces it to invent one.

## Evidence

Model — `smithy_query(query="shape", shape="ResourceTag")`:
```
"ResourceTag": {"type": "structure", "members": {
  "key": {"target": "ResourceTagKey", "required": true, "length": {"min": 1, "max": 128}, ...},
  "value": {"target": "ResourceTagValue", "length": {"min": 0, "max": 256}, "doc": "<p> The value that's associated with the tag. </p>"}}}
```
(`value` carries no `@required`; its own length minimum is 0, so an explicit `""` is a distinct legal value.)

The collapse — services/service-com-amazonaws-billing-awsbilling/src/state.rs:1597-1608:
```
pub fn requested_tag_set(tags: &[ResourceTag]) -> Result<BTreeMap<String, String>, String> {
    let mut requested: BTreeMap<String, String> = BTreeMap::new();
    for tag in tags {
        if requested
            .insert(tag.key.clone(), tag.value.clone().unwrap_or_default())
            .is_some()
```

The writer that calls it — services/service-com-amazonaws-billing-awsbilling/src/ops/tag_resource.rs:228-235:
```
        let requested = crate::state::requested_tag_set(&input.resource_tags)
            .map_err(|why| invalid_field("resourceTags", &why))?;
...
        let mut merged = view.tags.clone();
        merged.extend(requested);
```
(and services/service-com-amazonaws-billing-awsbilling/src/ops/create_billing_view.rs:263-264, the same helper for `CreateBillingView$resourceTags`.)

The read that reports the invented value — services/service-com-amazonaws-billing-awsbilling/src/ops/list_tags_for_resource.rs:212-221:
```
        Ok(ListTagsForResourceOutput {
            resource_tags: Some(
                view.tags
                    .iter()
                    .map(|(key, value)| ResourceTag {
                        key: key.clone(),
                        value: Some(value.clone()),
                    })
```

The sibling optional request string this crate does NOT collapse — services/service-com-amazonaws-billing-awsbilling/src/ops/create_billing_view.rs:337-340 (`name: input.name, description: input.description,`) and services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_view.rs:210-213:
```
            billing_view: BillingViewElement {
                arn: Some(view.arn.clone()),
                name: Some(view.name.clone()),
                description: view.description.clone(),
```
...and the convention stated in words — services/service-com-amazonaws-billing-awsbilling/src/ops/get_credits.rs:276-279:
```
/// An empty stored list is reported as an absent member rather than an empty one:
/// the model leaves `applicableProductNames`, `shareableAccounts` and
/// `purchaseTypeApplications` optional, and "no restriction recorded" is what an
/// empty one in the store means.
```

Test coverage: every tagging test sends an explicit `value` — tests/all/arn_account_boundary.rs:262-277 (`[{ "key": "team", "value": "finance" }]` read back as `[{ "key": "team", "value": "finance" }]`), tests/all/source_views_tags_and_policy.rs:611-690, tests/all/untag_resource.rs:262-281, tests/all/tag_set_duplicate_keys.rs:31. No test sends a `resourceTags` entry without `value`, so nothing pins the current substitution.

## Steps to Reproduce

1. Boot the emulator with the harness in tests/all/billing_views.rs::emulator_with_engine and create a CUSTOM billing view (CreateBillingView with name + sourceViews=[the account's PRIMARY view]).
2. Call TagResource(resourceArn=<that ARN>, resourceTags=[{"key": "team"}]) — the model leaves `ResourceTag$value` optional, so the entry is valid and reaches the handler.
3. Call ListTagsForResource(resourceArn=<that ARN>) and observe `resourceTags: [{"key": "team", "value": ""}]`: the empty string the store substituted at state.rs:1601, reported as a value by list_tags_for_resource.rs:216-219.
4. Call TagResource(resourceArn=<that ARN>, resourceTags=[{"key": "team", "value": ""}]) and read back again: the two different requests are indistinguishable, although `ResourceTagValue` admits `""` as a value of its own and the output member is optional.
5. Contrast CreateBillingView without `description` followed by GetBillingView: the absent optional string stays absent (create_billing_view.rs:339, get_billing_view.rs:213) instead of coming back as `""`.
