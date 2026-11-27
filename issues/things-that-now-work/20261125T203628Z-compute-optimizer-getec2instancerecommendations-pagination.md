Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: ac876aa83f4d95da3df81ad77985626059f5d170

# The page-token fingerprint digests the RAW optional `filters` member, so two spellings of one selection mint non-interchangeable tokens

_Filed: 2026-11-25T20:36:28Z_
_Operation: GetEC2InstanceRecommendations | Class: pagination_

> _Note: this issue was identified against the code base at commit `ac876aa83f4d95da3df81ad77985626059f5d170`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Four boundary-token readers bind `input.filters` into the selection fingerprint EXACTLY as it arrived off the wire — an `Option<Vec<Filter>>` — while normalising every other member of the same tuple. `crate::state::fingerprint` digests the Debug rendering, and `None` renders as `"None"` while `Some([])` renders as `"Some([])"`, so a request that omits `filters` and a request that sends `"filters": []` — which select the IDENTICAL result set, because the handler reads the member through `input.filters.as_deref().unwrap_or_default()` / `input.filters.iter().flatten()` — produce two different fingerprints. A `nextToken` minted by one spelling is therefore refused with 400 `InvalidParameterValueException` when the walk is resumed with the other, instead of resuming the same sequence.

The divergence is internal to this crate: the offset-token readers normalise the member FIRST and digest the normalised value, so for them both spellings resume each other's walk. Within `GetEC2InstanceRecommendations`'s own fingerprint expression three of the four selecting members (`account_id`, `requested_arns`, `requested_architectures`) are already normalised and only `filters` is raw.

Same defect, same expression, in these sites (one normalisation rule fixes all of them):
- `get_ec2_instance_recommendations.rs:232-238` — `&input.filters`
- `get_auto_scaling_group_recommendations.rs:291-297` — `&input.filters`
- `get_ebs_volume_recommendations.rs:215-220` — `&input.filters`
- `get_ecs_service_recommendations.rs:305-310` — `&input.service_arns` AND `&input.filters`, both raw
- `get_recommendation_preferences.rs:209-210` — `&input.scope`, although `PreferenceKey::new` documents that an empty `Scope` object addresses the same record as no scope at all

## Evidence

The rule the digest claims to keep — `state.rs:2264-2267`:
```
/// A stable digest of `value`'s debug rendering, used as the selection fingerprint bound
/// into a page token. The derived `Debug` of a generated input shape renders its members in
/// declaration order, so the same selection always digests to the same string and a
/// different one almost never does.
pub fn fingerprint<T: std::fmt::Debug>(value: &T) -> String {
    ::hex::encode(Sha256::digest(format!("{value:?}").as_bytes()))
```

The raw member, `src/ops/get_ec2_instance_recommendations.rs:232-238`:
```
        let selection_fingerprint = crate::state::fingerprint(&(
            Self::NAME,
            &account_id,
            &requested_arns,
            &input.filters,
            &requested_architectures,
        ));
```
…while the same handler reads the member normalised, `get_ec2_instance_recommendations.rs:212-213` and `:320-325`:
```
        if let Some(reason) =
            crate::state::resource_filter_rejection(input.filters.as_deref().unwrap_or_default())
```
```
                input
                    .filters
                    .iter()
                    .flatten()
                    .all(|filter| filter_matches(filter, instance, preferences))
```

The sibling readers of this crate that DO normalise before digesting — `src/ops/get_license_recommendations.rs:239` then `:247-253`:
```
        let filters = input.filters.clone().unwrap_or_default();
```
```
        let selection_fingerprint = crate::state::fingerprint(&(
            Self::NAME,
            &resource_arns,
            &account_id,
            &filters,
            &region,
        ));
```
and `src/ops/get_idle_recommendations.rs:242` + `:257-264`, `src/ops/get_rds_database_recommendations.rs:259` + `:274-281`, `src/ops/describe_recommendation_export_jobs.rs:208` + `:222-223`, `src/ops/get_enrollment_statuses_for_organization.rs:191` + `:199` — all digest the defaulted list.

The companion case on `GetRecommendationPreferences`, where the crate states the equivalence explicitly — `state.rs:628-629`:
```
    /// The key `resource_type` + `scope` addresses. An empty `Scope` object (neither
    /// member set) addresses the same record as no scope at all.
```
yet `src/ops/get_recommendation_preferences.rs:209-210` digests the raw member:
```
        let selection_fingerprint =
            crate::state::fingerprint(&(Self::NAME, &input.resource_type, &input.scope));
```

Test coverage: `tests/all/shared_paging_rules.rs` crosses every paginated reader with page sizes and asserts token/selection scoping (module doc lines 18-23: "a `nextToken` names a boundary of the walk that minted it and of no other, so a reader resumes its own token and refuses one another reader minted, or one minted for a different selection of its own"), but no row sends a request carrying an explicitly empty `filters` list, so neither spelling of the same selection is exercised.

## Steps to Reproduce

1. Opt the account in (`UpdateEnrollmentStatus` with `status=Active`) and let the analysis become due, so `GetEC2InstanceRecommendations` reports more than one instance.
2. POST `X-Amz-Target: ComputeOptimizerService.GetEC2InstanceRecommendations` with body `{"filters": [], "maxResults": 1}` — note the returned `nextToken`.
3. POST the same operation with `{"maxResults": 1, "nextToken": "<that token>"}` (i.e. `filters` omitted, which selects the identical set).
4. Observe 400 `InvalidParameterValueException` "The value supplied for parameter nextToken is not valid." instead of the second page.
5. Repeat the same two calls against `GetLicenseRecommendations` (which normalises `filters` before digesting) and observe that the walk resumes normally — the two readers disagree about whether the two spellings are one selection.
