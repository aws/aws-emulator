Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 249455a30ae6969aede90539e86d95c65dd85024

# Lambda function export rows are built from the findings catalog, so every function-specific column is empty and an Unavailable function is never exported

_Filed: 2026-11-14T01:51:39Z_
_Operation: ExportLambdaFunctionRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `249455a30ae6969aede90539e86d95c65dd85024`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`export_rows` dispatches `LambdaFunction` into the catch-all arm, which builds rows from the generic findings-catalog row (`analysed_resource_row`) instead of from the observation `GetLambdaFunctionRecommendations` reports the recommendation from (`ObservedFunction::to_recommendation`). The catalog row's own doc states that "The columns specific to a resource type's own configuration are left empty here".

Two consequences for the same account at the same instant:

1. The CSV's header carries every `ExportableLambdaFunctionField` the request asked for, but the data row fills only the generic columns (AccountId, FunctionArn, Finding, FindingReasonCodes, CurrentPerformanceRisk, the three savings columns, the preference columns, LastRefreshTimestamp). `FunctionVersion`, `NumberOfInvocations`, `UtilizationMetricsDurationMaximum/Average`, `UtilizationMetricsMemoryMaximum/Average`, `CurrentConfigurationMemorySize`, `CurrentConfigurationTimeout`, `CurrentCostTotal`, `CurrentCostAverage`, `RecommendationOptionsConfigurationMemorySize`, `RecommendationOptionsCostLow`, `RecommendationOptionsCostHigh`, the three `RecommendationOptionsProjectedUtilizationMetricsDuration*` columns, `Tags` and the three `*AfterDiscounts` columns are all empty, although the reader reports each of them for the same function.
2. A function whose finding is `Unavailable` has NO catalog row at all (`ObservedFunction::to_analysed_resource` returns `None` for it), so it can never appear in an export file — not even when the export request carries the very filter the reader honours for it (`filters=[{name: Finding, values: [Unavailable]}]`, which the reader answers with that function). The export then reports zero rows for a filter the reader selects a recommendation with, which is exactly the agreement the export module promises.

## Evidence

The rule the export module states — services/.../src/state/export.rs:25-34:
```
//!   Each cell is that field's value for
//!   that recommendation, computed from the stored analysis the same way the recommendation
//!   READER computes it, so an exported row and a `Get*Recommendations` response cannot
//!   disagree. ... Where a surface's
//!   recommendation carries members the shared findings-catalog row does not — ... the row is built from
//!   the observation the reader reports the recommendation from rather than from that catalog
//!   row, so the one rule serves both.
```

The arm `LambdaFunction` falls into — src/state/export.rs:925-947 (no `RESOURCE_TYPE_LAMBDA_FUNCTION` arm; `resource_type => self.analysed_resource_export_rows(...)`), and what that row omits — src/state/export.rs:1364-1368:
```
/// The columns specific to a resource type's own configuration are left empty here: they are
/// members of that type's recommendation rather than of the catalog row every findings reader
/// shares.
```
A grep of the crate for `CurrentMemorySize`/`FunctionVersion` in the export module returns no match, and `fn <x>_row(` exists only for the group, catalog, instance, database, license and idle surfaces (src/state/export.rs:1198, 1376, 1441, 1663, 1998, 2112) — there is no function row builder.

Where the reader gets those members instead — src/ops/get_lambda_function_recommendations.rs:290-294:
```
        let matching: Vec<LambdaFunctionRecommendation> = selected
            .into_iter()
            .map(|function| {
                function.to_recommendation(&region, &look_back_period, last_refresh_timestamp)
            })
```
(`ObservedFunction::to_recommendation`, src/state/functions.rs:515).

The missing Unavailable row — src/state/functions.rs:600-608:
```
    pub fn to_analysed_resource(&self, look_back_period: &str) -> Option<AnalysedResource> {
        let look_back_days = self.analysed_days(look_back_period);
        let finding = self.finding(look_back_days);
        if !findings_reported_for(SOURCE_TYPE_LAMBDA_FUNCTION)
            .iter()
            .any(|(stored, _)| *stored == finding)
        {
            return None;
        }
```
versus the reader, which DOES report it when the filter asks — src/ops/get_lambda_function_recommendations.rs:301-308:
```
            // "Functions with a finding of unavailable are not returned unless you specify
            // the filter parameter with a value of Unavailable in your
            // GetLambdaFunctionRecommendations request."
            .filter(|recommendation| {
                include_unavailable
                    || recommendation.finding.as_deref()
                        != Some(crate::state::LAMBDA_FINDING_UNAVAILABLE)
            })
```

The same reasoning the crate applies to the database surface, which builds from the recommendation — src/state/export.rs:1123-1127: "...none of which is a column of the findings-catalog row every other findings reader shares, so this surface builds its rows from the recommendation itself rather than from that row."

Test coverage: there is no `lambda_function_recommendations_export_file.rs`; the only content cross-check compares row COUNTS — tests/all/recommendation_export_objects.rs:828-834 — and its Lambda rows (lines 769-782) use only `Finding=NotOptimized` and `FindingReasonCode=MemoryOverprovisioned`, never `Finding=Unavailable`.

## Steps to Reproduce

1. Start the emulator with Compute Optimizer and Amazon S3 on one engine.
2. Call UpdateEnrollmentStatus with status=Active and let the analysis come due (ANALYSIS_DELAY).
3. Call GetLambdaFunctionRecommendations: each recommendation carries functionVersion, numberOfInvocations, currentMemorySize, utilizationMetrics and a memorySizeRecommendationOption with its projected durations and costs.
4. CreateBucket for the destination, then call ExportLambdaFunctionRecommendations with fieldsToExport=["FunctionArn","FunctionVersion","NumberOfInvocations","CurrentConfigurationMemorySize","UtilizationMetricsDurationMaximum","RecommendationOptionsConfigurationMemorySize","RecommendationOptionsCostLow","Tags","Finding"].
5. Poll DescribeRecommendationExportJobs until Complete and GetObject the answered key: every cell except FunctionArn and Finding is empty.
6. Call GetLambdaFunctionRecommendations with filters=[{"name":"Finding","values":["Unavailable"]}] — a recommendation comes back — then export with the same filter: the CSV holds the header row only.
