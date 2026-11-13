Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.topic-policy-principal-arn-pattern-admits-only-matching-roles sns.topic-policy-subscribe-only-grant-refuses-the-roles-publish sns.topic-policy-endpoint-condition-limits-which-queue-a-role-may-subscribe sns.topic-policy-protocol-condition-blocks-a-real-signed-subscribe sns.deploy-cfn-topic-policy-authorizes-only-the-named-role-and-follows-updates sns.encrypted-topic-whose-key-policy-withholds-usage-refuses-publish
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.topic-policy-principal-arn-pattern-admits-only-matching-roles|_error#21|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.topic-policy-subscribe-only-grant-refuses-the-roles-publish|_error#19|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.topic-policy-endpoint-condition-limits-which-queue-a-role-may-subscribe|_error#16|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.topic-policy-protocol-condition-blocks-a-real-signed-subscribe|_error#15|AssertionError|raise AssertionError(
Issue-Key: sns.deploy-cfn-topic-policy-authorizes-only-the-named-role-and-follows-updates|_error#13|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: sns.encrypted-topic-whose-key-policy-withholds-usage-refuses-publish|_error#15|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 5f222c49021a8706c85b6c9c85441ae0b0065824

# SNS authorizes nothing on the data plane: topic-policy statements and the topic's KMS key policy are never evaluated (2 causes)

> _Note: this issue was identified against the code base at commit `5f222c49021a8706c85b6c9c85441ae0b0065824`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 2: a topic access policy is stored but never consulted.
AWS: Publish and Subscribe model AuthorizationErrorException (HTTP 403, awsQueryError code AuthorizationError). A signed call by a principal the topic policy does not allow — or explicitly denies, or that fails a policy Condition (aws:SourceArn / sns:Endpoint / sns:Protocol / a Principal ARN pattern) — is refused with 403 AuthorizationError and has no effect.
Emulator: the call succeeds with HTTP 200 (Publish returns a MessageId; Subscribe returns a SubscriptionArn). src/ops/publish.rs performs no principal or policy evaluation at all: it validates parameters, looks the topic up and records the message.
Why: this is an over-permissive emulator. Least-privilege policies cannot be tested — a deployment whose topic policy is wrong passes here and is refused in production, and a negative security test can never fail.
Fix: an authorization stage in the SNS request walk, consulted by the Publish, PublishBatch, Subscribe and SetSubscriptionAttributes handlers (topic Policy statements incl. Condition keys).
Traces: sns.topic-policy-principal-arn-pattern-admits-only-matching-roles, sns.topic-policy-subscribe-only-grant-refuses-the-roles-publish, sns.topic-policy-endpoint-condition-limits-which-queue-a-role-may-subscribe, sns.topic-policy-protocol-condition-blocks-a-real-signed-subscribe, sns.deploy-cfn-topic-policy-authorizes-only-the-named-role-and-follows-updates.
See also open issue 20261025T000319Z-iam-sns-sns-performs-no-iam-authorization-publish.md, which carries the identity-policy side of the same gap for the iam__sns checks; these five sns checks are not covered by it.

Cause 2 of 2: publishing to an SSE-KMS topic never consults the key policy or key state.
AWS: Publish models KMSAccessDeniedException, KMSDisabledException, KMSInvalidStateException, KMSNotFoundException (all HTTP 400). Publishing to a topic whose KmsMasterKeyId key policy does not grant SNS kms:GenerateDataKey/kms:Decrypt is refused with KMSAccessDenied; a key pending deletion gives KMSInvalidState.
Emulator: Publish answers 200 with a MessageId regardless of the key policy or key state.
Why: a client believes an encrypted topic accepted its message when AWS would refuse it, and the emulator cannot exercise any KMS failure path.
Fix: a KMS check in the Publish/PublishBatch handler before the message is recorded.
Traces: sns.encrypted-topic-whose-key-policy-withholds-usage-refuses-publish (and sns.publish-to-topic-whose-key-is-pending-deletion-fails-kms-invalid-state, which additionally hangs on the missing delivery path).

## Evidence
Expected (AWS): HTTP 403 AuthorizationErrorException for the unauthorized Publish/Subscribe; HTTP 400 KMSAccessDeniedException for the publish to an SSE topic whose key policy withholds usage.
Actual (emulator):
- sns.topic-policy-subscribe-only-grant-refuses-the-roles-publish: AssertionError: expected the call to be refused, but it succeeded: {'MessageId': 'b6ed9f74-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
- sns.topic-policy-principal-arn-pattern-admits-only-matching-roles: same, 200 + MessageId
- sns.topic-policy-endpoint-condition-limits-which-queue-a-role-may-subscribe: 200 + SubscriptionArn
- sns.topic-policy-protocol-condition-blocks-a-real-signed-subscribe: expected this call to be refused, but it succeeded
- sns.deploy-cfn-topic-policy-authorizes-only-the-named-role-and-follows-updates: expected the call to be refused, but it succeeded: {'MessageId': '88e67932-...'}
- sns.encrypted-topic-whose-key-policy-withholds-usage-refuses-publish: expected the call to be refused, but it succeeded: {'MessageId': 'be69a66c-...'}

## Steps to Reproduce
1. CreateTopic; SetTopicAttributes(Policy=<statement allowing only sns:Subscribe to role R, or an explicit Deny of sns:Publish for R>).
2. AssumeRole R, and with those credentials call Publish(TopicArn, Message="x") -> 200 + MessageId (AWS: 403 AuthorizationError).
3. Repeat with a policy Condition on sns:Protocol or sns:Endpoint and call Subscribe with a non-matching protocol/endpoint -> 200 + SubscriptionArn.
4. CreateKey with a key policy that does not grant sns.amazonaws.com; SetTopicAttributes(KmsMasterKeyId=<that key>); Publish -> 200 (AWS: 400 KMSAccessDenied).
