Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 22d0a813c75ce3e8d355f134bff43e3d58146669

# Amazon ECS export file is built from the findings catalog, not from the analysis GetECSServiceRecommendations reports

_Filed: 2026-11-14T01:16:37Z_
_Operation: ExportECSServiceRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `22d0a813c75ce3e8d355f134bff43e3d58146669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other `Export*Recommendations` surface of this crate builds its CSV rows from the same stored analysis its reader reports from, and the export module states that as the rule. `EcsService` is the exception.

`export_rows` has explicit arms for `AutoScalingGroup`, `Idle`, `License`, `Ec2Instance` and `RdsDBInstance`; `EcsService` falls into the catch-all arm, which builds rows from the FINDINGS CATALOG (`analysed_resources`). But `GetECSServiceRecommendations` never reads the findings catalog: it sweeps the Amazon ECS service inventory (`ecs::state`) and the CloudWatch utilization published for each service. The only `EcsService` row the findings catalog holds is a hard-coded seed row for `service/payments/checkout` — an ECS service that exists in no ECS store.

Consequence, for the same account at the same instant:
* the export file carries exactly one data row, for `arn:aws:ecs:<region>:<account>:service/payments/checkout`, a service `GetECSServiceRecommendations` reports no recommendation for (and that no ECS API call ever created);
* every Amazon ECS service that the reader DOES report a recommendation for is absent from the file;
* the one row that is written carries only the generic catalog columns, so the ECS-specific columns (the current task CPU/memory, the recommended task size, the ECS utilization metrics) are empty — `analysed_resource_row`'s own doc says "The columns specific to a resource type's own configuration are left empty here".

The request's `filters` are then applied to that phantom row, so a filter that selects what the reader selects still selects nothing a client can reconcile with the reader.

## Evidence

The rule the crate states for every export file — services/service-com-amazonaws-computeoptimizer-computeoptimizerservice/src/state/export.rs:21-28:
```
//! * the export file — one HEADER row of the `Exportable*Field` names the request asked for
//!   ("The recommendations data to include in the export file"), then one row per
//!   recommendation the account's analysis holds for the job's resource type, ...
//!   Each cell is that field's value for
//!   that recommendation, computed from the stored analysis the same way the recommendation
//!   READER computes it, so an exported row and a `Get*Recommendations` response cannot
//!   disagree.
```

The arm `EcsService` falls into — src/state/export.rs:925-947:
```
        let mut rows = match plan.resource_type.as_str() {
            RESOURCE_TYPE_AUTO_SCALING_GROUP => self.auto_scaling_group_export_rows(...),
            RESOURCE_TYPE_IDLE => self.idle_export_rows(&plan.region, last_refresh),
            RESOURCE_TYPE_LICENSE => self.license_export_rows(&plan.region, last_refresh),
            RESOURCE_TYPE_EC2_INSTANCE => self.instance_export_rows(...),
            RESOURCE_TYPE_RDS_DB_INSTANCE => self.rds_database_export_rows(...),
            resource_type => {
                self.analysed_resource_export_rows(resource_type, &plan.region, last_refresh)
            }
        };
```
and what that catch-all is for — src/state/export.rs:1086-1096:
```
    /// The export rows of the findings catalog, for a resource type whose recommendation is
    /// reported from it: one row per analysed resource, ...
    fn analysed_resource_export_rows(
        ...
        let by_type = self.analysed_resources_by_type(account_id);
```

The only `EcsService` row that catalog holds — src/state.rs:3634-3645:
```
    vec![
        row(
            SOURCE_TYPE_ECS_SERVICE,
            "ecs",
            "service/payments/checkout",
            FINDING_OVERPROVISIONED,
```

Where the reader gets its recommendations instead — src/ops/get_ecs_service_recommendations.rs:382-414:
```
        let mut candidates: Vec<Candidate> = {
            let inventory = inventory.lock();
            ...
                inventory
                    .services
                    .values()
                    .filter(|record| record.is_active())
```
(and src/ops/get_ecs_service_recommendations.rs:470 `match analyse(candidate, &observations, now)`, computed from the CloudWatch store) — `analysed_resources` is never consulted.

The crate's own test suite records the gap — tests/all/recommendation_export_objects.rs:631-634:
```
/// The reader is the oracle, so each row asserts the export and the reader AGREE rather than
/// restating a value this analysis happens to hold; the table spans the filter names of every
/// surface whose reader reports from the same stored analysis the export file is built from,
```
That table (lines 638-797) covers ExportAutoScalingGroupRecommendations, ExportEBSVolumeRecommendations, ExportLicenseRecommendations, ExportRDSDatabaseRecommendations, ExportLambdaFunctionRecommendations and ExportIdleRecommendations — ExportECSServiceRecommendations is the one export operation left out, and tests/all/ecs_service_recommendations_export.rs asserts only the job record (ids, destination keys, lifecycle, refusals), never the file's rows.

## Steps to Reproduce

1. Start the emulator with Compute Optimizer, Amazon ECS, CloudWatch and Amazon S3 on one engine.
2. Call UpdateEnrollmentStatus with status=Active and wait for `ANALYSIS_DELAY` so the analysis is due.
3. Create an ECS cluster, register a Fargate task definition with task-level cpu/memory, CreateService with launchType=FARGATE, and publish AWS/ECS CPUUtilization and MemoryUtilization for it.
4. Call GetECSServiceRecommendations: it reports one recommendation, for `arn:aws:ecs:<region>:<account>:service/<cluster>/<service>`.
5. Create the destination bucket, then call ExportECSServiceRecommendations with fieldsToExport=[AccountId, ServiceArn, Finding] and poll DescribeRecommendationExportJobs until the job is Complete.
6. GetObject the answered `s3Destination.key`: the CSV holds one data row whose ServiceArn is `arn:aws:ecs:<region>:<account>:service/payments/checkout` — a service GetECSServiceRecommendations reports nothing for — and no row for the service created in step 3.
