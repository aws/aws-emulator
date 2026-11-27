Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__iam.rule-and-task-definition-lookup-from-another-region-rejected
Issue-Key: ecs__events__iam.rule-and-task-definition-lookup-from-another-region-rejected|run#28|AssertionError|assert far_cl["clusters"] == [] and [f.get("reason") for f in far_cl["failures"]] == ["MISSING"], (
Issue-Commit: 952a95f6f44f35dec8503ef518e3b550df5d6797

# ecs__events__iam: DescribeClusters is not region-scoped — a cluster created in another region is reported ACTIVE instead of failures[].reason MISSING

> _Note: this issue was identified against the code base at commit `952a95f6f44f35dec8503ef518e3b550df5d6797`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusters

## Description
One root cause. Related open issue 20261113T094431Z-ecs-events-iam-s3-ecs-state-is-not.md reports the same family of defect for the ecs__events__iam__s3 heading (ListClusters/ListTaskDefinitions crossing regions); it does not cover this check, which is filed under ecs__events__iam and fails specifically on DescribeClusters.

=== Cause 1 of 1: DescribeClusters resolves a cluster that exists only in another region and reports it as ACTIVE ===
AWS: ECS is a regional service; a cluster created against one region's endpoint exists only there. Smithy (com.amazonaws.ecs): DescribeClusters output has two members, `clusters` (Clusters) and `failures` (Failures); the Failure shape carries `arn`, `reason` and `detail` and its doc points at ECS's "API failure reasons". For a cluster name or ARN the endpoint's own region does not hold, AWS answers HTTP 200 with `clusters: []` and one `failures` entry whose `reason` is the fixed text "MISSING" and whose `arn` is the cluster ARN built from the REQUEST's region and the caller's account for the requested cluster name — the ARN and name are values taken from the request/state, "MISSING" is the fixed text. Beyond that, every ARN an ECS endpoint returns carries the region of the endpoint that served it. In the same check the EventBridge side is already correct (DescribeRule in the far region refuses ResourceNotFoundException and ListRules is empty) and ECS ListTaskDefinitions in the far region is empty, so only the cluster lookup diverges.
Emulator: a DescribeClusters call made against a second region (the check uses us-east-2 while the cluster was created in us-west-2) answers HTTP 200 with the foreign cluster fully populated — `clusters: [{clusterArn: arn:aws:ecs:us-west-2:<account>:cluster/<name>, clusterName: <name>, status: 'ACTIVE', ...}]` — and `failures: []`. The returned clusterArn still names the CREATING region, not the region of the endpoint that served the response, so the cluster store is shared across regional endpoints rather than keyed by region.
Why: a client that enumerates or probes ECS per region sees the same cluster in every region. Region-scoping logic breaks both ways: a caller that uses the MISSING failure to decide "this cluster is not here, create it" never creates it; a multi-region deployment or a per-region cleanup pass counts and can delete another region's cluster; and an operator is misled by a response whose ARN region disagrees with the endpoint it was addressed to.
Fix: the ecs service state and the DescribeClusters handler — cluster records must be keyed by the request's region, and the handler must place any cluster not present in that region into `failures` with reason "MISSING" instead of returning it in `clusters`.
Traces: ecs__events__iam.rule-and-task-definition-lookup-from-another-region-rejected

## Evidence
Failing check ecs__events__iam.rule-and-task-definition-lookup-from-another-region-rejected (mismatch):

AssertionError: DescribeClusters in us-east-2 must report the cluster MISSING, got {'clusters': [{'clusterArn': 'arn:aws:ecs:us-west-2:000000000000:cluster/sim2real-ebecs-8fb62cd8516f', 'clusterName': 'sim2real-ebecs-8fb62cd8516f', 'status': 'ACTIVE', 'registeredContainerInstancesCount': 0, 'runningTasksCount': 0, 'pendingTasksCount': 0, 'activeServicesCount': 0, 'statistics': [], 'tags': [], 'settings': [], 'capacityProviders': [], 'defaultCapacityProviderStrategy': []}], 'failures': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Expected (AWS): HTTP 200 with clusters == [] and failures == [{'arn': '<cluster ARN in the REQUEST's region>', 'reason': 'MISSING'}]. Only 'MISSING' is fixed text; the cluster name, account id and region pair above are this run's values and are not part of the requirement.

Positive controls in the same check passed: home-region DescribeRule/ListRules/ListTaskDefinitions/DescribeClusters all answered, and the far-region DescribeRule refusal (ResourceNotFoundException, "Rule <name> does not exist on EventBus default."), far-region ListRules == [] and far-region ListTaskDefinitions == [] all matched AWS — so the divergence is isolated to ECS cluster state partitioning, not to the harness's region configuration.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. In region A (e.g. us-west-2) call ecs:CreateCluster with a unique cluster name.
3. Build an ECS client for region B (e.g. us-east-2) and call ecs:DescribeClusters with clusters=[<that cluster name>].
4. Observe clusters contains the region-A cluster with status ACTIVE and an arn:aws:ecs:us-west-2:... ARN, and failures is empty, where AWS returns clusters == [] and failures == [{arn: <region-B ARN>, reason: 'MISSING'}].
5. Equivalently: replay check ecs__events__iam.rule-and-task-definition-lookup-from-another-region-rejected.
