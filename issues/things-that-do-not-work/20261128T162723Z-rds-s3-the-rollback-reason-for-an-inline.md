Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.cfn-deploy-rds-cluster-inline-s3-import-role-negative
Issue-Key: rds__s3.cfn-deploy-rds-cluster-inline-s3-import-role-negative|run#10|AssertionError|assert "s3Import" in detail and "not valid for the Aurora MySQL" in detail, (
Issue-Commit: f2a96a9f1fdbdd444ddfeaed949096426fea7442

# rds__s3: the rollback reason for an inline s3Import AssociatedRoles entry on an aurora-mysql DBCluster inserts 'the engine' before the engine label, so it does not read 'not valid for the Aurora MySQL'

> _Note: this issue was identified against the code base at commit `f2a96a9f1fdbdd444ddfeaed949096426fea7442`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddRoleToDBCluster

## Description
One net-new root cause in the rds__s3 surface, found by replaying the behavioral suite (simparity-behavioral-aws-v1) against the emulator built from this checkout. The check was not reported blocked by an unimplemented stub and is not marked OWNED by any open issue.

---
Cause 1 of 1: the feature-name refusal RDS returns for an unsupported FeatureName — surfaced as the CloudFormation rollback reason for an AWS::RDS::DBCluster created with an inline s3Import AssociatedRoles entry on aurora-mysql — is worded with the engine label after the word "engine" instead of directly after "for the".
AWS: recorded behavior only (Smithy models AddRoleToDBCluster's FeatureName as a plain String with no enum, and InvalidParameterValue is a service-common sender error, so neither the supported feature set nor the sentence is modeled). An aurora-mysql cluster does not support the s3Import feature, so creating the cluster with that AssociatedRoles entry fails and the stack rolls back with a reason that (a) names the rejected feature, s3Import, and (b) says it is "not valid for the Aurora MySQL" — i.e. the engine label RDS uses for the resolved cluster follows "not valid for the" immediately. The fixed text is the "The feature name <FeatureName> is not valid for the <engine label>" shape; the variable parts are the FeatureName taken from the template and the engine label taken from the cluster's resolved engine ("Aurora MySQL" for aurora-mysql). The failure itself is correct: AWS also fails the stack, leaves no cluster, and the same template without the entry creates an available cluster.
Emulator: the stack does roll back (ROLLBACK_COMPLETE) and the resource reason does name s3Import, but it is worded "The feature name s3Import is not valid for the engine Aurora MySQL." — the word "engine" is inserted between "for the" and the engine label, so the required phrase "not valid for the Aurora MySQL" never appears and the check cannot grade the refusal. NOTE for the fixer: open issue 20261125T230334Z-rds-s3-2-net-new-causes-addroletodbcluster-answers.md (which the replay did NOT bind to this check — its Cause 1 is about AddRoleToDBCluster answering the IAM-role-visibility sentence instead of a feature-name sentence at all) quotes the AWS wording as "... not valid for the engine Aurora MySQL.", the variant this check rejects. The two must be reconciled against the recording before the sentence is changed; the deterministic evidence here is that this check requires the engine label immediately after "for the".
Why: a client reading the stack-event reason (or the API refusal behind it) matches on this sentence to tell a terminal "this engine has no such feature, change the template" from the retryable role-visibility refusal that shares the InvalidParameterValue code. A reason whose wording differs from AWS's fails that match, so automation that classifies the rollback reason treats a terminal template error as unclassified or retryable, and a negative test of the inline-AssociatedRoles path cannot pass.
Fix: the FeatureName-validation message in the AddRoleToDBCluster handler and the AWS::RDS::DBCluster AssociatedRoles path that reaches it (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/ — the add-role-to-DB-cluster operation, whose refusal the CloudFormation resource handler copies into the stack-event reason): emit the sentence with the engine label where AWS puts it, keeping FeatureName from the request and the engine label from the resolved cluster.
Traces: rds__s3.cfn-deploy-rds-cluster-inline-s3-import-role-negative.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1) replayed against the emulator built from this checkout: 613/752 passed (81.5%); service rds__s3 124/136 (91%), 12 failed, of which this is the only one with no owning issue and no blocking stub (the sibling rds__s3.cfn-deploy-rds-cluster-missing-iam-trust-negative timed out on its own 'hold for at least max(120s, 3x the measured propagation lag)' wait, which exceeds the runner's 60 s budget by construction, so it is not filed here).

[mismatch] rds__s3.cfn-deploy-rds-cluster-inline-s3-import-role-negative:
  File "check_rds__s3_cfn_deploy_rds_cluster_inline_s3_import_role_negative.py", line 62, in run
    assert "s3Import" in detail and "not valid for the Aurora MySQL" in detail
  AssertionError: expected the inline-AssociatedRoles create to fail with the 's3Import ... not valid for the Aurora MySQL' error, got: expected stack sim2real-xsvc-stk-eb8f2b6a3642 CREATE_COMPLETE, got ROLLBACK_COMPLETE: ['sim2real-xsvc-stk-eb8f2b6a3642: The following resource(s) failed to create: [AuroraCluster]. Rollback requested by user.', 'AuroraCluster: Resource handler returned message: "The feature name s3Import is not valid for the engine Aurora MySQL. (Service: Rds, Status Code: 400, Request ID: 0bd935b3-...) (SDK Attempt Count: 1)" (RequestToken: 5b5bd161-..., HandlerErrorCode: InvalidRequest)']
The stack name, request id and request token are per-run values and are not part of the requirement; the status class (400), the feature name in the reason and the "not valid for the <engine label>" wording are.

## Steps to Reproduce
1. Deploy a CloudFormation stack containing an AWS::RDS::DBCluster with Engine: aurora-mysql and an inline AssociatedRoles entry whose FeatureName is s3Import, plus the IAM role it references (trusting rds.amazonaws.com).
2. AWS: the stack fails and rolls back, and the AuroraCluster resource reason names s3Import and reads '... is not valid for the Aurora MySQL ...'; no cluster remains. Emulator: the stack rolls back with the reason 'The feature name s3Import is not valid for the engine Aurora MySQL.' — the required phrase 'not valid for the Aurora MySQL' is absent because 'engine' is inserted before the label.
3. Deploy the same template without the AssociatedRoles entry and confirm the cluster reaches available (the check's positive control).
4. Re-replay to confirm a fix: rds__s3.cfn-deploy-rds-cluster-inline-s3-import-role-negative.
