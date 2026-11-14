Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.publish-to-topic-whose-key-is-pending-deletion-fails-kms-invalid-state sns.https-subscription-with-a-zero-retry-delivery-policy-attempts-a-failing-notification-exactly-once
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.publish-to-topic-whose-key-is-pending-deletion-fails-kms-invalid-state|_refusal_once_settled#12|Timeout|time.sleep(2)
Issue-Key: sns.https-subscription-with-a-zero-retry-delivery-policy-attempts-a-failing-notification-exactly-once|_captured#5|Timeout|resp = sqs.receive_message(QueueUrl=capture_url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Commit: 6a9be084645386b6802b5798f23439463b3480cb

# sns: 2 net-new root causes — Publish ignores the topic CMK's live state (PendingDeletion/Disabled), and an https subscription is never POSTed to so its DeliveryPolicy retry budget and dead-letter are inert

> _Note: this issue was identified against the code base at commit `6a9be084645386b6802b5798f23439463b3480cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
TWO distinct net-new root causes in the SNS data plane. Both are grounded in failing checks of the `sns` service that the replay report marks neither ALREADY FILED nor OWNED by any open issue. Each must be closed before this issue is submitted.

Related OPEN issue that does NOT carry these two checks' current replay keys (named for context only; it does not cover them): 20261027T030148Z-sns-6-net-new-behavioral-root-causes-no.md. It lists both check ids among its trace ids, but its recorded keys are `...publish-to-topic-whose-key-is-pending-deletion-fails-kms-invalid-state|_refusal_once_settled#12|Timeout|time.sleep(15)` and `...https-subscription-with-a-zero-retry-delivery-policy...|_captured#3|Timeout|...`, which no longer match where these checks now block, so the report attributes neither failure to it. Also related and not covering: 20261025T025317Z-sns-authorizes-nothing-on-the-data-plane-topic.md (the topic's KMS key POLICY, not the key's lifecycle STATE).

--- Cause 1 of 2: Publish to an SSE-KMS topic never consults the live KMS key state, so a key pending deletion or left disabled still accepts publishes ---

AWS: Smithy (com.amazonaws.sns#Publish) declares `KMSInvalidStateException` with awsQueryError code `KMSInvalidState`, httpError 400 ("The request was rejected because the state of the specified resource isn't valid for this request") and `KMSDisabledException` with code `KMSDisabled`, httpError 400. SNS resolves the topic's `KmsMasterKeyId` state on each publish, so: after kms:ScheduleKeyDeletion the key is PendingDeletion and Publish is refused HTTP 400 `KMSInvalidState`, relaying KMS's message verbatim — fixed text " is pending deletion. (Service: AWSKMS; Status Code: 400; Error Code: KMSInvalidStateException; Request ID: " … "; Proxy: null)" with the key ARN (taken from the topic's own KmsMasterKeyId) prefixed and the KMS request id as the ONLY per-call value; after kms:CancelKeyDeletion the key is Disabled and the byte-identical Publish is refused HTTP 400 `KMSDisabled` with the same template around the fixed text " is disabled." and `Error Code: DisabledException`; kms:EnableKey restores Publish. Everything but <key ARN> and <KMS request id> is fixed text of the message; neither a specific ARN nor a specific request id from any one run is part of the requirement.

Emulator: Publish keeps answering 200 with a MessageId in all three key states. The key is read (if at all) when the topic is created, never per publish, so no KMS state decision is made and neither refusal code can ever be produced. The check publishes on a bounded poll that exits as soon as any refusal appears, sees only successes, and is killed at the harness cap — "Error: Check timed out after 60.0s". Its first leg (Publish while the key is Enabled) succeeds, so the topic is usable and the timeout is caused only by the refusal never arriving. Calibration note: a key-state change can take minutes to reach real SNS's publish path (the check budgets 420 s), which the 60 s cap cannot cover on its own — confirm the fix with a direct ScheduleKeyDeletion/CancelKeyDeletion + Publish probe as well as the replay.

Why: revoking or scheduling deletion of a topic's CMK is the documented way to stop an encrypted topic accepting traffic. A client is told the message was accepted and stored encrypted under a key that could not have encrypted it, and the two distinct codes callers branch on — `KMSInvalidState` (pending deletion, needs CancelKeyDeletion) versus `KMSDisabled` (recoverable with EnableKey) — are both unreachable, so no automated remediation can distinguish them.

Fix: the Publish handler's KMS key resolution (shared with PublishBatch) in the sns service's publish operation — read the key's state per request instead of at CreateTopic, and map PendingDeletion/Disabled onto the already-declared KMSInvalidStateException / KMSDisabledException channels, relaying KMS's message text.

Traces: sns.publish-to-topic-whose-key-is-pending-deletion-fails-kms-invalid-state (mismatch, 60 s timeout inside the refusal poll). Handler component: services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/publish.rs.

--- Cause 2 of 2: an https subscription receives no POST at all, so a DeliveryPolicy retry budget is never applied and a failing notification is never dead-lettered ---

AWS: Smithy SubscribeInput$Protocol documents `https` as "delivery of JSON-encoded message via HTTPS POST", and SetSubscriptionAttributes accepts `DeliveryPolicy` / exposes `EffectiveDeliveryPolicy` and `RedrivePolicy` ("sends undeliverable messages to the specified Amazon SQS dead-letter queue"). Recorded behavior: Subscribe(Protocol="https") makes SNS POST a `SubscriptionConfirmation` body (Type, MessageId, TopicArn, Token, SubscribeURL, SigningCertURL) to the endpoint with headers `x-amz-sns-message-type: SubscriptionConfirmation`, `x-amz-sns-topic-arn`, `x-amz-sns-message-id`; after ConfirmSubscription each Publish is POSTed as `Type: "Notification"` with `x-amz-sns-message-id` equal to the MessageId Publish returned. An endpoint that answers HTTP 500 is attempted according to the subscription's healthyRetryPolicy: with `numRetries: 0` the notification is POSTed EXACTLY ONCE and then written to the queue named by the subscription's RedrivePolicy (one dead letter carrying that MessageId); a twin subscription with `numRetries: 1` draws at least two POSTs for the same single publish. The attempt count is a function of the configured policy, not of any one recording's numbers.

Emulator: nothing is ever POSTed to an http/https endpoint. The check cannot even obtain the confirmation Token — it repeats Subscribe and drains its capture queue waiting for a `SubscriptionConfirmation` that never arrives, and is killed at the harness cap ("Error: Check timed out after 60.0s"). Consequently the configured retry budget is never exercised, no notification is attempted once or twice, and no undeliverable notification is written to the RedrivePolicy dead-letter queue — while Publish answers 200 with a MessageId each time. Note on attribution: the check's receiver is a Lambda function URL (Lambda invocation is separately filed as 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md), but the SNS-side absence is independent of it — SNS emits no HTTPS request at all, and the retry/dead-letter accounting is SNS's own.

Why: a webhook consumer, the most common non-AWS SNS subscriber, is told its subscription exists and every Publish is accepted, yet receives nothing — no confirmation to echo back, no notification, and no dead letter when its endpoint fails. A client that sizes its retry budget through DeliveryPolicy (numRetries=0 to avoid duplicate side effects) cannot observe or rely on that budget, and an operator loses every failed notification silently instead of finding it in the subscription's DLQ.

Fix: the SNS Publish delivery dispatch component (the protocol target resolution used by the Publish handler's fan-out) plus the Subscribe handler's confirmation emission — add an http/https delivery path that POSTs the SubscriptionConfirmation and Notification envelopes with the x-amz-sns-* headers, counts attempts against the subscription's EffectiveDeliveryPolicy healthyRetryPolicy, and routes an exhausted delivery to the subscription's RedrivePolicy queue.

Traces: sns.https-subscription-with-a-zero-retry-delivery-policy-attempts-a-failing-notification-exactly-once (mismatch, 60 s timeout while waiting for the SubscriptionConfirmation POST). Handler component: services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/publish.rs (protocol target resolution) and the Subscribe handler.

## Evidence
Both failing checks are listed under the suite's `### sns` heading and are marked neither ALREADY FILED nor OWNED.

Cause 1 — `sns.publish-to-topic-whose-key-is-pending-deletion-fails-kms-invalid-state`: "Error: Check timed out after 60.0s".
Expected: after kms:ScheduleKeyDeletion (DescribeKey reports KeyState="PendingDeletion"), Publish → HTTP 400, Error.Code `KMSInvalidState`, Message "<key ARN> is pending deletion. (Service: AWSKMS; Status Code: 400; Error Code: KMSInvalidStateException; Request ID: <KMS request id>; Proxy: null)"; after kms:CancelKeyDeletion (KeyState="Disabled"), the identical Publish → HTTP 400, Error.Code `KMSDisabled`, Message "<key ARN> is disabled. (Service: AWSKMS; Status Code: 400; Error Code: DisabledException; Request ID: <KMS request id>; Proxy: null)"; after kms:EnableKey, Publish succeeds. Fixed text is everything except <key ARN> (from the topic's KmsMasterKeyId) and <KMS request id> (per-call; the check itself only requires it to parse as a UUID).
Actual: Publish returns HTTP 200 with a MessageId in the PendingDeletion and Disabled states; the check's refusal poll (which returns as soon as any refusal code appears) saw only successes and was killed at the 60 s cap. The pre-ScheduleKeyDeletion control Publish succeeded, so the topic was usable before the key state was changed.

Cause 2 — `sns.https-subscription-with-a-zero-retry-delivery-policy-attempts-a-failing-notification-exactly-once`: "Error: Check timed out after 60.0s".
Expected: Subscribe(Protocol="https") POSTs a SubscriptionConfirmation to the endpoint (headers x-amz-sns-message-type/x-amz-sns-topic-arn/x-amz-sns-message-id; body with Token + SubscribeURL) so ConfirmSubscription can be called with that Token; with healthyRetryPolicy numRetries=0 and a RedrivePolicy, one Publish to an endpoint answering HTTP 500 yields exactly ONE POST carrying that publish's x-amz-sns-message-id and exactly one dead letter with that MessageId in the DLQ; the twin subscription with numRetries=1 yields >= 2 POSTs for one publish.
Actual: no POST of any kind reaches the endpoint — the check never gets past waiting for the SubscriptionConfirmation, so attempts counted = 0, dead letters = 0, while Publish answers 200. Killed at the 60 s cap.

## Steps to Reproduce
Cause 1 (sns.publish-to-topic-whose-key-is-pending-deletion-fails-kms-invalid-state):
1. kms:CreateKey; kms:DescribeKey to capture the key ARN.
2. sns:CreateTopic(Attributes={"KmsMasterKeyId": "<key id>"}); sns:Publish → succeeds (control).
3. kms:ScheduleKeyDeletion(KeyId=<key>, PendingWindowInDays=7); DescribeKey reports PendingDeletion.
4. sns:Publish again — EXPECT 400 `KMSInvalidState` with "<key ARN> is pending deletion. (Service: AWSKMS; Status Code: 400; Error Code: KMSInvalidStateException; Request ID: <uuid>; Proxy: null)"; OBSERVE 200 + MessageId.
5. kms:CancelKeyDeletion (key now Disabled); sns:Publish — EXPECT 400 `KMSDisabled` with "<key ARN> is disabled. (… Error Code: DisabledException …)"; OBSERVE 200 + MessageId.
6. kms:EnableKey; sns:Publish succeeds (recovery leg).

Cause 2 (sns.https-subscription-with-a-zero-retry-delivery-policy-attempts-a-failing-notification-exactly-once):
1. sns:CreateTopic twice (one topic per retry policy).
2. Stand up an https endpoint that records every request and answers HTTP 500 to Notification and 200 to SubscriptionConfirmation.
3. sns:Subscribe(TopicArn=<topic>, Protocol="https", Endpoint=<that URL>, ReturnSubscriptionArn=true) — OBSERVE that no SubscriptionConfirmation POST ever reaches the endpoint, so no Token exists to pass to sns:ConfirmSubscription.
4. On each subscription set DeliveryPolicy {"healthyRetryPolicy": {"numRetries": 0|1, "minDelayTarget": 1, "maxDelayTarget": 1, "numNoDelayRetries": 0, "numMinDelayRetries": 0, "numMaxDelayRetries": 0, "backoffFunction": "linear"}} and a RedrivePolicy naming an SQS DLQ whose policy allows sns.amazonaws.com sqs:SendMessage.
5. sns:Publish once per subscription — EXPECT exactly 1 POST (numRetries=0) and >= 2 POSTs (numRetries=1) carrying the publish's x-amz-sns-message-id, plus exactly one dead letter per publish in the matching DLQ; OBSERVE zero POSTs and empty DLQs.
