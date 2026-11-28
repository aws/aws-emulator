Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__emr__iam.cluster-in-nonexistent-subnet-rejected ec2__emr__iam.unknown-service-role-cluster-rejected ec2__emr__iam.service-role-without-placement-group-policy-rejected
Issue-Key: ec2__emr__iam.cluster-in-nonexistent-subnet-rejected|_poll#11|Timeout|time.sleep(interval_s)
Issue-Key: ec2__emr__iam.unknown-service-role-cluster-rejected|_poll#11|Timeout|time.sleep(interval_s)
Issue-Key: ec2__emr__iam.service-role-without-placement-group-policy-rejected|_poll#11|Timeout|time.sleep(interval_s)
Issue-Commit: 84939cc283904302bdfe02403fef158918b6c804

# ec2__emr__iam: a cluster built on an unresolvable reference is never settled TERMINATED_WITH_ERRORS / VALIDATION_ERROR — the ghost-subnet, unknown-service-role and missing-placement-group-policy checks all reach no verdict

> _Note: this issue was identified against the code base at commit `84939cc283904302bdfe02403fef158918b6c804`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
THIS ISSUE BUNDLES 1 CAUSE. It must be closed before submitting.

Cause 1 of 1: RunJobFlow accepts a cluster whose references cannot be resolved, and the cluster is then never settled into EMR's asynchronous validation failure, so no refusal is ever observable.
  AWS: RunJobFlow does NOT resolve Ec2SubnetId, ServiceRole or the service role's permissions synchronously — it returns a JobFlowId — and the cluster then ends in state TERMINATED_WITH_ERRORS with Status.StateChangeReason.Code = VALIDATION_ERROR (Smithy: ClusterStateChangeReason.Code / ClusterStateChangeReasonCode, whose values include VALIDATION_ERROR; Message carries the descriptive sentence for the unresolved reference), having launched NO EC2 capacity and, for a request naming a spread placement group, created no placement group. This holds for each of the three inputs these checks use: an Ec2SubnetId that does not exist, a ServiceRole name that does not exist, and a service role carrying AmazonElasticMapReduceRole but not AmazonElasticMapReducePlacementGroupPolicy. The same request with the reference repaired launches capacity (each check runs that positive control). Recorded behavior for the asynchronous settlement; the state and reason code are modeled.
  Emulator: the accepted cluster never reaches a terminal state — the checks poll DescribeCluster for TERMINATED/TERMINATED_WITH_ERRORS and never see one, so all three are cut off by the harness budget with 'Check timed out after 60.0s' and reach no verdict at all; no VALIDATION_ERROR reason is ever served for any of the three inputs. Hypothesis: there is no asynchronous reference/permission validation pass after RunJobFlow, so a cluster with a ghost subnet or an unusable service role behaves exactly like a valid one.
  Why: a client whose cluster request is wrong (typo'd subnet, role not yet created, role missing the placement-group permission) gets a cluster that looks healthy forever instead of AWS's fast VALIDATION_ERROR shutdown — no error surface, no cleanup trigger, and automation that waits for a terminal state hangs; conversely nothing proves the emulator refuses these inputs at all.
  Fix: the cluster state machine behind RunJobFlow/DescribeCluster in the emr service — after accepting the request it must resolve Ec2SubnetId/Ec2SubnetIds, the ServiceRole and the permissions that role needs for the resources the request asks EMR to create (placement group), and on failure settle the cluster TERMINATED_WITH_ERRORS with StateChangeReason.Code VALIDATION_ERROR and the matching message, launching no EC2 capacity and creating no placement group.
  Traces: ec2__emr__iam.cluster-in-nonexistent-subnet-rejected, ec2__emr__iam.unknown-service-role-cluster-rejected, ec2__emr__iam.service-role-without-placement-group-policy-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (ec2__emr__iam: 3/6 passed).

All three surviving failures report only:
  Error: Check timed out after 60.0s

What each check does (from the check sources): mint a fresh EMR service role + EC2 instance profile, launch a positive-control multi-primary cluster (m7g.xlarge, emr-6.15.0) and require it to reach launched EC2 capacity and then TERMINATED; then launch the same request with ONE broken reference and poll DescribeCluster until a terminal state, asserting
  (final_state, StateChangeReason.Code) == ('TERMINATED_WITH_ERRORS', 'VALIDATION_ERROR')
and that no EC2 instance (and, for the placement-group check, no placement group tagged aws:elasticmapreduce:job-flow-id=<cluster>) exists.
  - cluster-in-nonexistent-subnet-rejected: Ec2SubnetId = 'subnet-0000000000000000f'
  - unknown-service-role-cluster-rejected: ServiceRole = a role name that was never created
  - service-role-without-placement-group-policy-rejected: a service role with AmazonElasticMapReduceRole only, then repaired by attaching AmazonElasticMapReducePlacementGroupPolicy and relaunched (the arm that must launch).
Expected: the broken cluster settles TERMINATED_WITH_ERRORS / VALIDATION_ERROR within minutes. Actual: no terminal state is ever observed, so the harness kills each check at its budget.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from it.
2. Create a VPC and subnet; create an EMR service role (AmazonElasticMapReduceRole) and an EC2 instance profile (AmazonElasticMapReduceforEC2Role).
3. RunJobFlow with Instances.Ec2SubnetId='subnet-0000000000000000f' (a subnet that does not exist) — expect the JobFlowId to be returned and the cluster to settle TERMINATED_WITH_ERRORS with StateChangeReason.Code='VALIDATION_ERROR' and no EC2 instances; observe the cluster never reaching a terminal state (replay ec2__emr__iam.cluster-in-nonexistent-subnet-rejected).
4. Repeat with ServiceRole set to a role name that was never created (replay ec2__emr__iam.unknown-service-role-cluster-rejected).
5. Repeat with a multi-primary cluster requesting a spread placement group and a service role WITHOUT AmazonElasticMapReducePlacementGroupPolicy — expect the same VALIDATION_ERROR settlement and no placement group, then attach the policy and relaunch and expect capacity (replay ec2__emr__iam.service-role-without-placement-group-policy-rejected).
