Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__dynamodb__iam.appsync-data-source-role-without-appsync-trust-rejected
Issue-Key: appsync__dynamodb__iam.appsync-data-source-role-without-appsync-trust-rejected|run#22|AssertionError|assert body.get("data") == {"untrusted": None}, f"the refused field must read null, got {body!r}"
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# appsync__dynamodb__iam: an AMAZON_DYNAMODB data source whose service role does not trust appsync.amazonaws.com still resolves

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataSource

## Description
Cause 1 of 1: AppSync does not require the data source's service role to trust appsync.amazonaws.com.
AWS: an AMAZON_DYNAMODB data source assumes its `serviceRoleArn`. If that role's trust policy does not name `appsync.amazonaws.com` as a principal, the AssumeRole fails at resolve time and the field resolves to null with an error in the 200 GraphQL errors envelope — no DynamoDB call is made. (Recorded behavior for the exact envelope; the requirement under test is only that the refused field reads null.)
Emulator: the resolver runs anyway and the field returns data: `{'data': {'untrusted': {'holder': 'kim', 'level': 3}}}` — the role's trust policy is never evaluated, so an un-assumable role grants full access to the table.
Why: an authorization boundary is not enforced. A deployment that forgot the AppSync trust statement works in the emulator and fails in AWS, and conversely a role deliberately scoped away from AppSync is still usable, so the emulator cannot be used to validate least-privilege wiring.
Fix: the AppSync data-source resolve path — it must assume `serviceRoleArn` through the IAM trust-policy evaluation before issuing the DynamoDB call, and surface the AssumeRole failure as a field error.
Traces: appsync__dynamodb__iam.appsync-data-source-role-without-appsync-trust-rejected.

## Evidence
Expected: the field backed by the untrusted role reads null (with an error in the errors envelope). Actual: `the refused field must read null, got {'data': {'untrusted': {'holder': 'kim', 'level': 3}}}`.

## Steps to Reproduce
1. Create an IAM role with dynamodb:GetItem on a table but a trust policy that does not name appsync.amazonaws.com.
2. Create an AMAZON_DYNAMODB data source on a GraphQL API using that role, with a GetItem resolver.
3. Query the field — observe it returns the stored item instead of null.
