Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.rds-associate-role-wrong-feature-name-negative
Issue-Key: rds__s3.rds-associate-role-wrong-feature-name-negative|run#13|AssertionError|assert error.get("Message") == expected_message, (
Issue-Commit: e9aa0118b8d3778084a8f5f6890b0994ca6ea9d7

# rds__s3: AddRoleToDBCluster's unrecognized-FeatureName refusal re-words AWS's sentence — "... is not valid for the Aurora PostgreSQL engine." instead of "... is not valid for the engine Aurora (PostgreSQL)."

> _Note: this issue was identified against the code base at commit `e9aa0118b8d3778084a8f5f6890b0994ca6ea9d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddRoleToDBCluster

## Description
One net-new root cause in the rds__s3 surface.

Cause 1 of 1: AddRoleToDBCluster now classifies an unrecognized FeatureName correctly (InvalidParameterValue, no association created) but serves a re-worded message: the engine display name and the clause order are both different from AWS's sentence.

AWS: AddRoleToDBCluster with a FeatureName the resolved cluster's engine does not support is refused with HTTP 400, error code InvalidParameterValue, and the message "The feature name <FeatureName> is not valid for the engine <engine>." The FIXED text is, in this order: "The feature name " … " is not valid for the engine " … "." — the clause naming the engine comes LAST, introduced by "for the engine". The VARIABLE parts are (a) <FeatureName>, copied verbatim from the request, and (b) <engine>, the engine display name RDS uses for the cluster resolved from DBClusterIdentifier — "Aurora (PostgreSQL)" for an aurora-postgresql cluster, "Aurora MySQL" for an aurora-mysql one. Smithy (com.amazonaws.rds#AmazonRDSv19, awsQuery): AddRoleToDBCluster input is DBClusterIdentifier (required, String), RoleArn (required, IAMRoleArn, @length 20..2048, @pattern ^arn:aws[a-z-]*:iam::[0-9]*:role/.*$) and FeatureName, a plain String with NO enum; its modeled errors are DBClusterNotFoundFault(404), DBClusterRoleAlreadyExistsFault(400), DBClusterRoleQuotaExceededFault(400), InvalidDBClusterStateFault(400). The model therefore says nothing about which feature names an engine supports and nothing about this message; InvalidParameterValue is a service-common error, so the code and the sentence are recorded behavior only.
Emulator: the status and code are right (400 InvalidParameterValue) and DescribeDBClusters correctly still reports AssociatedRoles empty, but the message is "The feature name NotARealFeatureXYZ is not valid for the Aurora PostgreSQL engine." Two divergences in one sentence: the engine clause is moved before the word "engine" ("for the <engine> engine." instead of "for the engine <engine>."), and the engine display name is rendered "Aurora PostgreSQL" instead of the parenthesised "Aurora (PostgreSQL)" RDS uses for aurora-postgresql. The FeatureName itself is echoed correctly.
Why: under awsQuery every RDS refusal of this class shares one code, InvalidParameterValue, so clients discriminate on the message: operators grep for "is not valid for the engine", and tooling that must distinguish the terminal feature-name rejection from the retryable role-visibility sentence ("IAM role ARN value is invalid or does not include the required permissions for: AWS_ROLE_INTEGRATION") matches on fixed substrings. A client that matches AWS's text classifies this refusal as unknown and either retries a terminal error forever or surfaces the wrong cause; a user or log pipeline that keys on the engine token sees "Aurora PostgreSQL", a string RDS never emits for this engine. CloudFormation surfaces the same sentence verbatim as an AWS::RDS::DBCluster failure reason, so the divergence propagates into stack events too.
Fix: the FeatureName validation in the AddRoleToDBCluster handler and the engine display-name mapping it formats the message from — emit AWS's clause order ("The feature name {feature} is not valid for the engine {engine}.") and the engine's RDS display name for the cluster resolved from the request ("Aurora (PostgreSQL)" for aurora-postgresql, "Aurora MySQL" for aurora-mysql).
Traces: rds__s3.rds-associate-role-wrong-feature-name-negative.

This is the residue of a partially-landed fix, and the replay report records no issue as owning this check: the still-open 20261125T230334Z-rds-s3-2-net-new-causes-addroletodbcluster-answers.md described the earlier state, in which the call answered the ROLE sentence permanently and this check burned its budget in the role-visibility retry loop. That half is gone — the emulator now answers the feature-name sentence and the check proceeds to its message assertion — but the sentence itself is worded differently, which that issue's key (a Timeout on the error-code assertion) no longer matches. The sibling check named there, rds__s3.rds-load-from-s3-missing-role-negative (aurora-mysql / "Aurora MySQL"), is reported as owned and is not re-filed here; fixing the wording should be checked against both engine display names.

## Evidence
Behavioral suite replay against the emulator built from this checkout: 2377/2681 passed (88.7%). This is one of only two failing checks the report returns with no owning issue and no blocking stub.

[mismatch] rds__s3.rds-associate-role-wrong-feature-name-negative
  File "check_rds__s3_rds_associate_role_wrong_feature_name_negative.py", line 61, in run
    assert error.get("Message") == expected_message, (
  AssertionError: expected message 'The feature name NotARealFeatureXYZ is not valid for the engine Aurora (PostgreSQL).', got 'The feature name NotARealFeatureXYZ is not valid for the Aurora PostgreSQL engine.'

Expected vs actual, with the parts separated:
  status/code: 400 InvalidParameterValue — AGREES (the preceding `assert error.get("Code") == "InvalidParameterValue"` passed).
  message fixed text: AWS "The feature name " / " is not valid for the engine " / "."; emulator "The feature name " / " is not valid for the " / " engine." — the engine clause and the word "engine" are transposed.
  message variable parts: FeatureName = "NotARealFeatureXYZ", taken from the request — AGREES. Engine display name for the aurora-postgresql cluster resolved from DBClusterIdentifier = "Aurora (PostgreSQL)" — emulator emits "Aurora PostgreSQL".

The check reaches this assertion only after deliberately polling out AWS's role-propagation window (it loops while the message equals "IAM role ARN value is invalid or does not include the required permissions for: AWS_ROLE_INTEGRATION"), so the emulator is no longer stuck on the role sentence — the loop exits and the graded answer is the feature-name validation itself. Nothing in this evidence depends on per-run values: the cluster identifier, role ARN and bucket are all ctx.unique_name()-generated and do not appear in the asserted message.

## Steps to Reproduce
1. Create an Aurora PostgreSQL cluster: rds CreateDBCluster(DBClusterIdentifier=<id>, Engine="aurora-postgresql", DBSubnetGroupName=<group spanning >=2 AZs>, ...).
2. Create an IAM role trusting rds.amazonaws.com with s3:GetObject/s3:ListBucket on a bucket, then wait out RDS's role-visibility window by calling AddRoleToDBCluster repeatedly while the answer's message is "IAM role ARN value is invalid or does not include the required permissions for: AWS_ROLE_INTEGRATION".
3. Call rds AddRoleToDBCluster(DBClusterIdentifier=<id>, RoleArn=<role>, FeatureName="NotARealFeatureXYZ").
   AWS: 400 InvalidParameterValue, message "The feature name NotARealFeatureXYZ is not valid for the engine Aurora (PostgreSQL)."
   Emulator: 400 InvalidParameterValue, message "The feature name NotARealFeatureXYZ is not valid for the Aurora PostgreSQL engine."
4. Confirm DescribeDBClusters(DBClusterIdentifier=<id>)["DBClusters"][0]["AssociatedRoles"] == [] (the emulator already agrees), and that the same role with FeatureName="s3Import" is accepted and reaches ACTIVE.
5. Repeat step 3 on an aurora-mysql cluster with FeatureName="s3Import" to check the other engine display name: AWS says "The feature name s3Import is not valid for the engine Aurora MySQL."
6. Re-replay to confirm: rds__s3.rds-associate-role-wrong-feature-name-negative.
