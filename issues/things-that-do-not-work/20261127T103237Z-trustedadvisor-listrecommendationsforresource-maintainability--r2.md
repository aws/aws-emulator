Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 7f2f9e8fbacb5ccb6dc94ed688d25f78f8760a08

# ListRecommendationsForResource hand-rolls three filter tests the crate's two shared filter predicates already state

_Filed: 2026-11-27T10:32:37Z_
_Operation: ListRecommendationsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `7f2f9e8fbacb5ccb6dc94ed688d25f78f8760a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states each list-filter rule ONCE and reaches it from every handler that needs it: `crate::state::ResourceFilters::keeps` is the one predicate both resource lists select a stored `ResourceEntry` with, and `crate::state::RecommendationFilters::keeps` is the one predicate both recommendation lists select a stored recommendation with. `ListRecommendationsForResource` cuts its rows on three filters — `status` over the flagged resource's own status, `pillar` over the recommendation's `pillars`, and `checkArn` over the recommendation's check — and every one of those three tests is already written in one of those two shared predicates. The handler nevertheless writes all three out again inline, so the crate now holds two copies of each of them and the copies can drift: a change to `ResourceFilters::keeps`'s status test, or to `RecommendationFilters::keeps`'s pillar-membership / check-equality tests, would not reach this handler, and the only thing keeping `GET /v1/recommendations-for-resource/{arn}?status=ok` and `GET /v1/recommendations/{id}/resources?status=ok` selecting alike is that the two copies happen to be written identically today. Its sibling `ListRecommendationResources` builds `crate::state::ResourceFilters` from the request and calls the shared `keeps` instead. The fix is to select through the existing shared predicates (the filters this operation does not model stay `None`, which keeps every resource/recommendation), leaving every response byte and every existing test unchanged.

## Evidence

The inline copy — `services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_recommendations_for_resource.rs:342-352`:
```
                let kept = input
                    .status
                    .as_ref()
                    .is_none_or(|status| &resource.status == status)
                    && input.pillar.as_ref().is_none_or(|pillar| {
                        recommendation.pillars.iter().any(|stored| stored == pillar)
                    })
                    && input
                        .check_arn
                        .as_ref()
                        .is_none_or(|check| &check_arn == check);
```

The already-existing copy of the status test — `src/state.rs:1159-1167`:
```
    pub fn keeps(&self, resource: &ResourceEntry) -> bool {
        self.status.is_none_or(|status| resource.status == status)
            && self
                .exclusion_status
                .is_none_or(|exclusion| resource.exclusion_status == exclusion)
```

The already-existing copies of the pillar-membership and check-equality tests — `src/state.rs:1294-1303`:
```
            && self
                .pillar
                .is_none_or(|pillar| row.pillars.iter().any(|stored| stored == pillar))
...
            && self
                .check_identifier
                .is_none_or(|check| row.check_arn == Some(check))
```

The rule the crate states about exactly this — `src/state.rs:1251-1254`:
```
/// The eight are modelled identically on the two operations and select over
/// members both entry types carry, so the predicate is stated once — by
/// [`Self::keeps`] — and the cursor scope the filters contribute once, by
/// [`Self::scope`], as [`ResourceFilters`] already states the resource lists'.
```

And the sibling handler that does reach the shared predicate — `src/ops/list_recommendation_resources.rs:205-215`:
```
        // The three filters this read cuts on, and the ONE predicate they
        // select a stored resource with — the same one the organization-level
        // view of these resources applies
        // ([`crate::state::ResourceFilters`]), so the two cannot select
        // differently.
        let filters = crate::state::ResourceFilters {
            status: input.status.as_deref(),
            exclusion_status: input.exclusion_status.as_deref(),
            region_code: input.region_code.as_deref(),
        };
```

Test coverage that pins the responses a refactor must not change: `tests/all/recommendations_for_resource.rs:6-22` ("cut by the request's `status` / `pillar` / `checkArn` filters"), `tests/all/shared_recommendation_filters.rs:9-19`, and `tests/all/shared_resource_view.rs`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/ops/list_recommendations_for_resource.rs and read lines 342-352: the `status`, `pillar` and `checkArn` tests are written inline in the handler.
2. Open src/state.rs:1159-1167 and read `ResourceFilters::keeps`: the identical `status`-equality test over a `ResourceEntry` is already there.
3. Open src/state.rs:1289-1306 and read `RecommendationFilters::keeps`: the identical pillar-membership test over `row.pillars` and the identical check-equality test over `row.check_arn` are already there.
4. Compare with src/ops/list_recommendation_resources.rs:205-215 and src/ops/list_recommendations.rs:221-230, where the sibling handlers build the shared filter structs and call `keeps` instead of restating the tests.
5. Observe that editing either shared `keeps` (for example to normalise a Region or a status comparison) changes the two resource lists and the two recommendation lists but leaves ListRecommendationsForResource on its own copy.
