Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 22d0a813c75ce3e8d355f134bff43e3d58146669

# EcsService summary counts a seeded catalog row instead of the Amazon ECS services GetECSServiceRecommendations reports

_Filed: 2026-11-14T01:16:37Z_
_Operation: GetRecommendationSummaries | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `22d0a813c75ce3e8d355f134bff43e3d58146669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The summary reader groups the findings catalog by resource type and then substitutes the RDS group with `analysed_databases`, precisely so that the summary and `GetRDSDatabaseRecommendations` cannot disagree about how many databases there are or about any one's finding. The `EcsService` group gets no such substitution, even though Amazon ECS recommendations are likewise NOT reported from the findings catalog: `GetECSServiceRecommendations` computes them from the Amazon ECS service inventory plus the CloudWatch utilization published for each service.

So the `EcsService` summary counts exactly one `Overprovisioned` service — the hard-coded catalog row for `service/payments/checkout` — in an account whose ECS store holds no service at all, and it keeps counting that one row (and nothing else) after a client creates real ECS services that `GetECSServiceRecommendations` does report recommendations for. The two readers of "the account's Amazon ECS services" therefore disagree in both directions, which is the exact disagreement the sibling RDS branch exists to prevent.

## Evidence

The substitution the sibling group gets, and the reason given for it — services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/src/ops/get_recommendation_summaries.rs:277-302:
```
        // The databases the account holds an analysis for, by the crate's one rule for it
        // ([`crate::state::analysed_databases`]): a summary of the account's Amazon Aurora and
        // RDS databases counts exactly the databases `GetRDSDatabaseRecommendations` reports
        // at this endpoint, so the two cannot disagree about how many there are or about any
        // one's finding, risk or cost.
        let databases = crate::state::analysed_databases(...);
        ...
        let mut analysed = guard.analysed_resources_by_type(&account_id);
        // The database rows are the analysed databases above ...
        analysed.insert(
            crate::state::SOURCE_TYPE_RDS_DB_INSTANCE,
            databases.iter().collect(),
        );
```
Nothing equivalent is done for `SOURCE_TYPE_ECS_SERVICE`; the group used at src/ops/get_recommendation_summaries.rs:320-323 (`analysed.get(resource_type)`) is the catalog's.

The only `EcsService` row that catalog holds — src/state.rs:3634-3645:
```
    vec![
        row(
            SOURCE_TYPE_ECS_SERVICE,
            "ecs",
            "service/payments/checkout",
            FINDING_OVERPROVISIONED,
```

Where the ECS reader gets its recommendations — src/ops/get_ecs_service_recommendations.rs:382-414:
```
        let mut candidates: Vec<Candidate> = {
            let inventory = inventory.lock();
            ...
                inventory
                    .services
                    .values()
                    .filter(|record| record.is_active())
```

Test coverage: tests/all/classified_resource_type_summaries.rs:56-60 pins only the FINDING VOCABULARY reported for `EcsService` (`["Underprovisioned", "Overprovisioned", "Optimized"]`), and tests/all/recommendation_summaries.rs:73/110 likewise asserts the resource-type surface — no test ties the `EcsService` counts to what GetECSServiceRecommendations reports (the RDS counts are tied to their reader).

## Steps to Reproduce

1. Start the emulator with Compute Optimizer, Amazon ECS and CloudWatch on one engine; call UpdateEnrollmentStatus with status=Active and wait for `ANALYSIS_DELAY`.
2. Call GetECSServiceRecommendations with no arguments: it reports an empty `ecsServiceRecommendations` list (the ECS store holds no service).
3. Call GetRecommendationSummaries and read the summary whose `recommendationResourceType` is `EcsService`: its `Overprovisioned` count is 1, for a service no ECS call created and no ECS reader reports.
4. Create an ECS cluster, a Fargate task definition and a service, and publish AWS/ECS CPUUtilization and MemoryUtilization for it; call GetECSServiceRecommendations again: one recommendation is now reported.
5. Call GetRecommendationSummaries again: the `EcsService` counts are unchanged — still the one phantom row, and nothing for the service the reader reports.
