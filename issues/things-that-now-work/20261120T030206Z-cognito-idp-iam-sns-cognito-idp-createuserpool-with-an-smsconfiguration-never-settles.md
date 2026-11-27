Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__iam__sns.create-user-pool-sms-role-without-sns-publish-rejected
Issue-Key: cognito-idp__iam__sns.create-user-pool-sms-role-without-sns-publish-rejected|_create_pool_tolerating_trust_propagation#6|Timeout|return cidp.create_user_pool(
Issue-Commit: 4a31818efe332ca88a0519d4a493d7749df24842

# cognito-idp CreateUserPool with an SmsConfiguration never settles: the SMS-role probe keeps refusing a validly-trusted role, so the sns:Publish refusal leg is never reached

> _Note: this issue was identified against the code base at commit `4a31818efe332ca88a0519d4a493d7749df24842`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPool

## Description
Cause 1 of 1: binding an SNS caller role to a user pool at CreateUserPool reaches neither of AWS's two outcomes — the call never succeeds for a correctly trusted role, and the distinct refusal for a role without sns:Publish is therefore never produced.

AWS: Smithy (com-amazonaws-cognitoidentityprovider): CreateUserPool.SmsConfiguration = {SnsCallerArn (required), ExternalId, SnsRegion}; the operation models InvalidSmsRoleTrustRelationshipException (client, 400) and InvalidSmsRoleAccessPolicyException (client, 400). AWS probes the named role before accepting the pool. (a) A role whose trust policy allows sts:AssumeRole for Principal Service cognito-idp.amazonaws.com with the request's sts:ExternalId (and aws:SourceAccount/aws:SourceArn conditions satisfied) and which grants sns:Publish is accepted: CreateUserPool returns 200 and DescribeUserPool echoes SmsConfiguration.SnsCallerArn as the ARN that was bound. (b) A role with that same trust policy but no sns:Publish grant is refused 400 InvalidSmsRoleAccessPolicyException whose Message is the fixed sentence "Role does not have permission to publish with SNS" (no variable part), and no pool is left behind — ListUserPools never shows the refused PoolName. (c) InvalidSmsRoleTrustRelationshipException is reserved for a role Cognito cannot assume at all, and in practice only appears transiently while a just-created role propagates in IAM.

Emulator: the check makes no progress and is killed at the harness's 60 s per-check budget ("Error: Check timed out after 60.0s"). The only loop in the check retries CreateUserPool while — and only while — the error code is InvalidSmsRoleTrustRelationshipException, so the emulator is answering that refusal repeatedly for a role whose trust policy does name cognito-idp.amazonaws.com with the matching ExternalId and SourceAccount. Because the good-path create never succeeds, the access-policy leg (b) is never exercised and InvalidSmsRoleAccessPolicyException with its fixed message is never observed. Hypothesis: the SMS-role probe does not resolve the role in IAM and evaluate its trust policy, and refuses with the trust-relationship error unconditionally (or the probe does not terminate).

Why: no user pool can be created with SMS enabled, so every SMS-MFA / phone-verification path that depends on an SNS caller role is unreachable for a client; and a client that mis-scopes the role's permissions is told the trust relationship is wrong rather than that the role cannot publish, so it edits the trust policy instead of adding sns:Publish.

Fix: SmsConfiguration role validation in the cognito-idp crate's CreateUserPool handler (and the identical path in UpdateUserPool/SetUserPoolMfaConfig): resolve SnsCallerArn in IAM, evaluate the trust relationship against cognito-idp.amazonaws.com with the supplied ExternalId, and separately evaluate the sns:Publish grant, mapping the two failures onto InvalidSmsRoleTrustRelationshipException and InvalidSmsRoleAccessPolicyException respectively, with no pool persisted on refusal.

Traces: cognito-idp__iam__sns.create-user-pool-sms-role-without-sns-publish-rejected. Note: the open issue 20261117T112621Z-cognito-idp-iam-sns-createuserpool-with-an-sms.md describes this same CreateUserPool SMS-role behavior; the replay report did not report this check as owned by it (its recorded replay key no longer matches this run's failure, which now carries no traceback frame), so it is re-filed here against the current failure and should be closed together with that issue if they share one fix.

## Evidence
Failing check: [mismatch] cognito-idp__iam__sns.create-user-pool-sms-role-without-sns-publish-rejected
Error: Check timed out after 60.0s

The check's only retry path is bounded to one error code — it calls CreateUserPool in a loop and re-raises anything whose Error.Code is not 'InvalidSmsRoleTrustRelationshipException' — so the 60 s hang locates the emulator's response: CreateUserPool kept answering InvalidSmsRoleTrustRelationshipException for a role created moments earlier with the trust policy
  Effect Allow / Principal {Service: cognito-idp.amazonaws.com} / Action sts:AssumeRole
  Condition StringEquals {sts:ExternalId: <externalId>, aws:SourceAccount: <account>}, ArnLike {aws:SourceArn: arn:aws:cognito-idp:<region>:<account>:userpool/*}
and with an inline Allow of sns:Publish on "*".

Expected (AWS), per the assertions the check never reached:
  - good role: CreateUserPool 200, then DescribeUserPool echoes SmsConfiguration.SnsCallerArn == the bound role ARN
  - role without the sns:Publish grant: 400, Error.Code = InvalidSmsRoleAccessPolicyException, Error.Message = "Role does not have permission to publish with SNS" (entirely fixed text — no request or state values appear in it)
  - after that refusal, ListUserPools must not contain the refused PoolName

Actual (emulator): CreateUserPool never returns either outcome for an SmsConfiguration; it repeats InvalidSmsRoleTrustRelationshipException until the check's budget expires.

## Steps to Reproduce
1. iam:CreateRole with a trust policy allowing sts:AssumeRole for Principal Service cognito-idp.amazonaws.com, conditioned on StringEquals sts:ExternalId=<E> and aws:SourceAccount=<account> and ArnLike aws:SourceArn=arn:aws:cognito-idp:<region>:<account>:userpool/*; then iam:PutRolePolicy granting Allow sns:Publish on "*".
2. cognito-idp:CreateUserPool(PoolName=<good>, SmsConfiguration={SnsCallerArn: <role arn>, ExternalId: <E>}) — expect 200; the emulator instead answers 400 InvalidSmsRoleTrustRelationshipException and keeps doing so on retry.
3. cognito-idp:DescribeUserPool(UserPoolId) — expect SmsConfiguration.SnsCallerArn to echo the role ARN bound at step 2.
4. Mint a second identical role with a different ExternalId but NO sns:Publish policy, and call CreateUserPool with it — expect 400 InvalidSmsRoleAccessPolicyException with Message "Role does not have permission to publish with SNS".
5. cognito-idp:ListUserPools — the refused PoolName from step 4 must not appear.
Replay: cognito-idp__iam__sns.create-user-pool-sms-role-without-sns-publish-rejected
