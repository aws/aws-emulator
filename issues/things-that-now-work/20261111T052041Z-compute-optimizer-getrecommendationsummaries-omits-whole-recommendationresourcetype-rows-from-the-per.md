Issue-Source: trace-sweep-divergence
Issue-Trace: compute-optimizer-recommendation-summaries-and-export-job-listing
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: compute-optimizer-recommendation-summaries-and-export-job-listing
Issue-Shape-Id: com.amazonaws.computeoptimizer#ComputeOptimizerService
Issue-Key: compute-optimizer|GetRecommendationSummaries|2xx->2xx|$.recommendationSummaries[*]
Issue-Commit: f3853c2ce3831f59f19a9d4fb097e3f9d5244398

# GetRecommendationSummaries omits whole recommendationResourceType rows from the per-account summary set

> _Note: this issue was identified against the code base at commit `f3853c2ce3831f59f19a9d4fb097e3f9d5244398`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecommendationSummaries

## Description
Cause 1 of 1: GetRecommendationSummaries returns an incomplete recommendationSummaries list - several recommendationResourceType rows are never emitted, and one emitted row is missing a finding bucket.

AWS: GetRecommendationSummaries returns, for each requested account, one RecommendationSummary per resource type that Compute Optimizer summarises, covering the RecommendationSourceType enum (Smithy: com.amazonaws.computeoptimizer#RecommendationSourceType values Ec2Instance, AutoScalingGroup, EbsVolume, LambdaFunction, EcsService, License, RdsDBInstance, RdsDBInstanceStorage, AuroraDBClusterStorage, NatGateway, DynamoDBTable, ElastiCacheCluster, MemoryDBCluster, DocumentDBCluster, WorkSpaces, SageMakerEndpoint). A row is emitted even when the account has no analysed resource of that type: its finding buckets are present with value 0 (Smithy: Summary.value / IdleSummary.value target SummaryValue with @default 0). Rows for resource types whose findings are classified by utilisation carry `summaries` (Smithy: Summary.name -> Finding), while idle-classified resource types carry `idleSummaries` with the three IdleFinding buckets Idle, Unattached and Unused (Smithy: com.amazonaws.computeoptimizer#IdleFinding values Idle, Unattached, Unused). Each row also carries the owning `accountId`. The License row's `summaries` carries three buckets - Optimized, NotOptimized and InsufficientMetrics - which is recorded behavior only: the modelled Finding enum lists Underprovisioned, Overprovisioned, Optimized, NotOptimized and does not name InsufficientMetrics, but the recorded AWS response emits that bucket for License. Operation is paginated over recommendationSummaries (Smithy: paginated items=recommendationSummaries, inputToken/outputToken=nextToken, pageSize=maxResults), so a short list is not a pagination artefact when no nextToken is returned.

Emulator: the call answers HTTP 200 with the modelled shape, but recommendationSummaries is short and the rows after the gap are shifted. At the position where the recording has the `License` row (accountId, recommendationResourceType "License", summaries Optimized/NotOptimized/InsufficientMetrics all 0.0) the emulator instead has its `AuroraDBClusterStorage` row - and that row carries only two summary buckets (Optimized, NotOptimized) where the recording has three. The six trailing idle-classified rows are absent altogether: where the recording has rows with recommendationResourceType ElastiCacheCluster, DynamoDBTable, MemoryDBCluster, SageMakerEndpoint, WorkSpaces and DocumentDBCluster, each with idleSummaries Idle/Unattached/Unused at 0.0, the emulator returns nothing (got None) - i.e. the list simply ends early. So the emulator emits summaries only for the resource types it has analysers for, and only for the finding buckets it happens to populate, instead of the full zero-filled resource-type matrix AWS returns.

Why: a client that reads this response as the account-wide optimisation picture silently loses whole resource classes. Console-style and reporting clients index the response by recommendationResourceType and will report License, DynamoDBTable, ElastiCacheCluster, MemoryDBCluster, DocumentDBCluster, WorkSpaces and SageMakerEndpoint as "not returned" rather than "zero findings"; a client that walks the list positionally reads the wrong resource type's counts entirely, because the missing rows shift every later row. A client that sums the finding buckets to compute an optimised/not-optimised ratio also gets a wrong denominator for the row that is missing its InsufficientMetrics bucket, and cannot distinguish "no idle resources" from "idle findings unsupported" because the idle rows are simply absent rather than zero.

Fix: the GetRecommendationSummaries handler and the summary aggregation it reads from in the compute-optimizer crate. The handler must enumerate the full RecommendationSourceType set for each requested account rather than only the types backed by state, emit a zero-valued row for a type with no analysed resources, choose `summaries` vs `idleSummaries` per resource type (idle-classified types get all three IdleFinding buckets, utilisation-classified types get their full bucket set including the License row's InsufficientMetrics bucket), and keep the row order stable so paginated clients see a deterministic list.

Traces: compute-optimizer-recommendation-summaries-and-export-job-listing#task#3 (mismatch, expected 200 / actual 200, body divergences at $.recommendationSummaries[8] and $.recommendationSummaries[10] through [15]).

## Evidence
Fixture compute-optimizer-recommendation-summaries-and-export-job-listing, step task#3, operation GetRecommendationSummaries, verdict mismatch, expected_status 200, actual_status 200.

$.recommendationSummaries[8]
  expected: {"accountId": "<account>", "recommendationResourceType": "License", "summaries": [{"name": "Optimized", "value": 0.0}, {"name": "NotOptimized", "value": 0.0}, {"name": "InsufficientMetrics", "value": 0.0}]}
  actual:   {"accountId": "<account>", "recommendationResourceType": "AuroraDBClusterStorage", "summaries": [{"name": "Optimized", "value": 0.0}, {"name": "NotOptimized", "value": 0.0}]}

$.recommendationSummaries[10] .. [15]
  expected: one row per resource type ElastiCacheCluster, DynamoDBTable, MemoryDBCluster, SageMakerEndpoint, WorkSpaces, DocumentDBCluster, each {"accountId": "<account>", "recommendationResourceType": "<type>", "idleSummaries": [{"name": "Idle", "value": 0.0}, {"name": "Unattached", "value": 0.0}, {"name": "Unused", "value": 0.0}]}
  actual:   None (the list ends before these indices)

The accountId value above is a per-environment value from the replay account and is not part of the requirement; the requirement is that every RecommendationSourceType row is present, carries the requesting account's id, and carries its full zero-valued finding buckets. No nextToken was expected at this step, so the short list is not pagination.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture `compute-optimizer-recommendation-summaries-and-export-job-listing`.
2. At step `task#3` the client calls compute-optimizer GetRecommendationSummaries for the caller's account (no accountIds filter narrowing it, no maxResults cut-off).
3. Inspect the 200 response body's recommendationSummaries array: collect the set of recommendationResourceType values present.
4. Observe the emulator's set is missing License, ElastiCacheCluster, DynamoDBTable, MemoryDBCluster, SageMakerEndpoint, WorkSpaces and DocumentDBCluster, and that the AuroraDBClusterStorage row carries only Optimized and NotOptimized buckets; AWS returns a row for every RecommendationSourceType with its finding buckets present at 0.
