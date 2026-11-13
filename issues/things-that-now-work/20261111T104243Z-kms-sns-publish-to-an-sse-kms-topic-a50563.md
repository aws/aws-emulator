Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__sns.publish-fails-while-kms-master-key-is-disabled
Issue-Key: kms__sns.publish-fails-while-kms-master-key-is-disabled|run#19|AssertionError|assert code == ERR_KMS_DISABLED, (
Issue-Commit: 56af4a38801370870cefc18c3ef22e7084182017

# kms__sns: Publish to an SSE-KMS topic is accepted while the topic's CMK is DISABLED instead of being refused 400 KMSDisabled

> _Note: this issue was identified against the code base at commit `56af4a38801370870cefc18c3ef22e7084182017`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
ONE net-new root cause, grounded in the only failing check of this suite's `kms__sns` service that the replay report marks neither ALREADY FILED nor OWNED.

Related OPEN issue, named for context only and NOT covering this failure (it does not carry this check's current replay key — its Issue-Key is the old 60 s-timeout signature, while the check now fails with an explicit AssertionError after 211 bounded attempts): 20261102T133928Z-kms-sns-publish-to-an-sse-kms-topic-b9e489.md. Also related but different surfaces: 20261025T025317Z-sns-authorizes-nothing-on-the-data-plane-topic.md (key POLICY, not key STATE).

Cause 1 of 1: SNS Publish never consults the live state of the customer CMK named by the topic's KmsMasterKeyId.

AWS: Smithy (com.amazonaws.sns#Publish) models KMSDisabledException with awsQueryError code `KMSDisabled` and httpError 400 — "The request was rejected because the specified customer master key (CMK) isn't enabled" — exactly for this input. A topic created with Attributes.KmsMasterKeyId = a customer CMK (an alias is accepted and resolved) envelopes every message under that key, so the key is resolved on the publish path. Recorded behavior: while the key is Enabled, Publish succeeds with a MessageId; after kms:DisableKey (DescribeKey KeyState="Disabled"), Publish is refused HTTP 400 with Error.Code exactly `KMSDisabled`, every further Publish is refused the same way while the key stays disabled, and kms:EnableKey restores Publish. The requirement is the status (400) and the code (`KMSDisabled`); any key identity inside the message is a value read from the topic's own KmsMasterKeyId and from KMS state, never a literal from one run.

Emulator: every Publish after DisableKey still answers 200 with a MessageId. The check polled for 420 s over 211 attempts and every single attempt SUCCEEDED (the check reports the sentinel '<published>', which it defines as "every attempt SUCCEEDED, i.e. the disabled key was never consulted at Publish time"). The positive control Publish before DisableKey also succeeded, so the topic is usable and the only divergence is the missing refusal. Hypothesis: KmsMasterKeyId is stored as an opaque topic attribute at CreateTopic and no KMS data-key request is issued per publish.

Why: disabling a topic's CMK is the standard break-glass control for stopping an encrypted topic from accepting traffic. A client told its Publish succeeded believes the message was enveloped under a key that could not have encrypted it; a revocation runbook that proves the stop by a failing Publish silently passes; and the `KMSDisabled` code callers branch on (transient — retry after re-enabling) is unreachable.

Fix: the SNS Publish handler's encryption/key-resolution path (and the identical path behind PublishBatch) — resolve the topic's KmsMasterKeyId, aliases included, request the data key from KMS per publish, and map a disabled key onto the already-modelled KMSDisabledException channel.

Traces: kms__sns.publish-fails-while-kms-master-key-is-disabled (mismatch). Handler component: services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/publish.rs.

## Evidence
Failing check (suite service heading `### kms__sns`): kms__sns.publish-fails-while-kms-master-key-is-disabled

AssertionError: "Publish to a topic whose CMK is disabled must fail with 'KMSDisabled' within 420s and 211 attempts; got '<published>' ('<published>' means every attempt SUCCEEDED, i.e. the disabled key was never consulted at Publish time)".

Expected: HTTP 400, Error.Code `KMSDisabled` (Smithy: Publish declares KMSDisabledException, awsQueryError code `KMSDisabled`, httpError 400) on every Publish while the CMK is disabled; success again after EnableKey.
Actual: HTTP 200 with a MessageId on all 211 attempts over 420 s.
Control: the Publish issued before DisableKey returned a MessageId, so the CMK-encrypted topic was serving before the single thing under test was broken.

## Steps to Reproduce
1. kms:CreateKey; kms:CreateAlias pointing at that key.
2. sns:CreateTopic(Name=..., Attributes={"KmsMasterKeyId": "<that alias>"}).
3. sns:Publish(TopicArn=<topic>, Message="control-before-disable") — succeeds with a MessageId (positive control).
4. kms:DisableKey(KeyId=<key>); kms:DescribeKey shows KeyState="Disabled".
5. sns:Publish(TopicArn=<topic>, Message="probe") — EXPECT HTTP 400 with Error.Code `KMSDisabled`; OBSERVE HTTP 200 with a MessageId (repeat: still accepted).
6. kms:EnableKey(KeyId=<key>); sns:Publish succeeds again (recovery leg).
Replay: kms__sns.publish-fails-while-kms-master-key-is-disabled.
