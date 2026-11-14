Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 249455a30ae6969aede90539e86d95c65dd85024

# Amazon EBS volume export rows are built from the findings catalog, so every volume-specific column in the CSV is empty

_Filed: 2026-11-14T01:51:39Z_
_Operation: ExportEBSVolumeRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `249455a30ae6969aede90539e86d95c65dd85024`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`export_rows` has dedicated row builders for the Auto Scaling group, Amazon EC2 instance, Aurora/RDS database, license and idle surfaces, each of which computes its cells from the same stored observation the surface's recommendation READER reports from. `EbsVolume` has none: it falls into the catch-all arm, which builds rows from the generic findings-catalog row (`analysed_resource_row`), whose own doc states that "The columns specific to a resource type's own configuration are left empty here".

Consequence: an `ExportEBSVolumeRecommendations` file carries a header row with every `ExportableVolumeField` the request asked for, but the data row fills only the generic columns (AccountId, VolumeArn, Finding, CurrentPerformanceRisk, the three savings columns, the preference columns and LastRefreshTimestamp). Every column that names the volume's OWN analysis is empty, although `GetEBSVolumeRecommendations` reports all of them for the same volume at the same instant:

* `CurrentConfigurationVolumeType`, `CurrentConfigurationVolumeSize`, `CurrentConfigurationVolumeBaselineIOPS`, `CurrentConfigurationVolumeBaselineThroughput`, `CurrentConfigurationVolumeBurstIOPS`, `CurrentConfigurationVolumeBurstThroughput`, `CurrentConfigurationRootVolume`, `RootVolume` — the reader reports all of these in `currentConfiguration`;
* `UtilizationMetricsVolumeReadOpsPerSecondMaximum` and its five siblings — the reader reports them in `utilizationMetrics`;
* `CurrentMonthlyPrice`, `RecommendationOptionsConfiguration*`, `RecommendationOptionsMonthlyPrice`, `RecommendationOptionsPerformanceRisk` — the reader reports the single option in `volumeRecommendationOptions`;
* `Tags` — the reader reports the volume's tags (four other row builders fill `Tags`; the catalog row does not);
* the three `*AfterDiscounts` savings columns — the reader reports `savingsOpportunityAfterDiscounts`.

The helper that would fill them already exists and is already the reader's one rule (`ObservedVolume::to_recommendation`), and the crate states the principle for the database surface in so many words — it builds rows "from the recommendation itself rather than from that row" precisely because the catalog row carries none of the surface's own members. The EBS surface needs exactly that and does not do it.

Second symptom of the same root cause: the catalog's EBS rows are projected ONCE at store creation under `DEFAULT_LOOK_BACK_PERIOD`, while the reader resolves `effective_look_back_period`. After `PutRecommendationPreferences{resourceType: EbsVolume, lookBackPeriod: DAYS_93}` the export row still carries the DAYS_14 Finding, CurrentPerformanceRisk and savings figures while `put_preferences` writes `LookbackPeriodInDays=93` / `EffectiveRecommendationPreferencesLookBackPeriod=DAYS_93` into the SAME row — a row that contradicts itself and the reader.

## Evidence

The rule the export module states for every file — services/.../src/state/export.rs:21-34:
```
//! * the export file — one HEADER row of the `Exportable*Field` names the request asked for
//!   ... Each cell is that field's value for
//!   that recommendation, computed from the stored analysis the same way the recommendation
//!   READER computes it, so an exported row and a `Get*Recommendations` response cannot
//!   disagree. ... Where a surface's
//!   recommendation carries members the shared findings-catalog row does not — the Amazon EC2
//!   instance surface, whose options carry an instance type, and the Aurora and RDS database
//!   surface, ... — the row is built from
//!   the observation the reader reports the recommendation from rather than from that catalog
//!   row, so the one rule serves both.
```

The arm `EbsVolume` falls into — src/state/export.rs:925-947:
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

What that catch-all row admits it omits — src/state/export.rs:1359-1368:
```
/// One findings-catalog row as an export row: the columns a catalog row records about any
/// analysed resource ...
/// The columns specific to a resource type's own configuration are left empty here: they are
/// members of that type's recommendation rather than of the catalog row every findings reader
/// shares. The database surface, whose catalog row carries the configuration the analysis
/// recorded for it, reports those members from the recommendation itself
/// ([`rds_database_row`]).
```
and the builder itself (src/state/export.rs:1376-1428) puts only `AccountId`, `arn_field_of(resource_type)`, `Finding`, `FindingReasonCodes`, `CurrentPerformanceRisk`, the three savings columns, `InferredWorkloadTypes`, `put_preferences(...)` and `LastRefreshTimestamp`. A grep of the whole crate for `CurrentConfigurationVolume`/`RootVolume` returns no match — no code path ever fills a volume column — and `fn <x>_row(` exists only for the group, catalog, instance, database, license and idle surfaces (src/state/export.rs:1198, 1376, 1441, 1663, 1998, 2112): there is no volume row builder at all.

The same reasoning stated for the database surface, which DOES build from the recommendation — src/state/export.rs:1123-1127:
```
    /// A database's recommendation carries the engine, the DB instance class, the storage
    /// configuration, the utilization observed on it and the instance and storage options the
    /// analysis would move it to — none of which is a column of the findings-catalog row every
    /// other findings reader shares, so this surface builds its rows from the recommendation
    /// itself rather than from that row.
```

Where the reader gets those members instead — src/ops/get_ebs_volume_recommendations.rs:280-284:
```
        let matching: Vec<VolumeRecommendation> = selected
            .into_iter()
            .map(|volume| {
                volume.to_recommendation(&region, &look_back_period, last_refresh_timestamp)
            })
```
and `ObservedVolume::to_recommendation` (src/state/volumes.rs:353-410) fills `current_configuration`, `utilization_metrics`, `volume_recommendation_options`, `savings_opportunity_after_discounts` and `tags` — none of which reaches the export.

The look-back drift: the catalog rows are frozen at the default period — src/state.rs:3673-3681:
```
    // The Amazon EBS volume rows are the SAME observation
    // `GetEBSVolumeRecommendations` reports, projected the same way
    // ([`ObservedVolume::to_analysed_resource`]) under the look-back period in force when
    // no preference sets one, ...
    .chain(
        observed_volumes()
            .iter()
            .map(|volume| volume.to_analysed_resource(DEFAULT_LOOK_BACK_PERIOD)),
    )
```
while the reader resolves the preference — src/ops/get_ebs_volume_recommendations.rs:246-247:
```
        let look_back_period = guard
            .effective_look_back_period(crate::state::RESOURCE_TYPE_EBS_VOLUME, &account_id);
```
and the crate states that this changes the answer — src/state/volumes.rs:104-106: "io1, 200 GiB with 8,000 provisioned IOPS: far more than the workload asks for, and its heaviest day falls OUTSIDE the default look-back period, so a longer look-back preference changes the option this volume gets" (pinned by tests/all/ebs_volume_recommendations.rs:363-396 `a_longer_look_back_preference_analyses_more_of_the_observed_period`).

Test coverage: there is no `ebs_volume_recommendations_export_file.rs` (unlike `ec2_instance_recommendations_export_file.rs`, `auto_scaling_group_recommendations_export_file.rs` and `rds_database_recommendations_export_file.rs`), and the only cross-check of the EBS file's content compares row COUNTS only — tests/all/recommendation_export_objects.rs:828-834: `let data = csv_rows(&exported).len().saturating_sub(1); assert_eq!(data > 0, selected > 0, ...)`. tests/all/ebs_volume_recommendations_export.rs:503-521 asks for `CurrentConfigurationVolumeType`, `RecommendationOptionsMonthlyPrice`, `RootVolume` and `Tags` but asserts only that a job id comes back.

## Steps to Reproduce

1. Start the emulator with Compute Optimizer and Amazon S3 on one engine.
2. Call UpdateEnrollmentStatus with status=Active and let the analysis come due (ANALYSIS_DELAY).
3. Call GetEBSVolumeRecommendations: each volumeRecommendation carries currentConfiguration (volumeType/volumeSize/IOPS/throughput/rootVolume), utilizationMetrics, one volumeRecommendationOption with its configuration and savings, and tags.
4. CreateBucket for the destination, then call ExportEBSVolumeRecommendations with s3DestinationConfig={bucket} and fieldsToExport=["VolumeArn","CurrentConfigurationVolumeType","CurrentConfigurationVolumeSize","UtilizationMetricsVolumeReadOpsPerSecondMaximum","RecommendationOptionsConfigurationVolumeType","RecommendationOptionsMonthlyPrice","RootVolume","Tags","Finding"].
5. Poll DescribeRecommendationExportJobs until the job is Complete, then GetObject the answered s3Destination.key.
6. Every cell except VolumeArn and Finding is empty, although the reader reported a value for each of them.
7. Additionally: call PutRecommendationPreferences{resourceType:"EbsVolume", lookBackPeriod:"DAYS_93"}, re-read GetEBSVolumeRecommendations (the io1 volume's option and savings change), and export again: the row still carries the DAYS_14 Finding/CurrentPerformanceRisk/savings while its LookbackPeriodInDays cell reads 93.
