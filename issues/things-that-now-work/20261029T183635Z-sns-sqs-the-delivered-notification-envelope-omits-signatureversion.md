Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs.sqs-envelope-raw-bytes-are-pretty-printed-in-canonical-key-order sns__sqs.message-attribute-name-outside-the-allowed-charset-rejected sns__sqs.a-message-filtered-out-at-publish-time-is-never-redelivered-when-the-policy-is-relaxed sns__sqs.sns-subscription-dlq-captures-the-notification-the-endpoint-policy-denied
Issue-Key: sns__sqs.sqs-envelope-raw-bytes-are-pretty-printed-in-canonical-key-order|run#18|AssertionError|assert order == keys, f"envelope keys for {payload!r} are {order!r}, expected {keys!r}"
Issue-Key: sns__sqs.message-attribute-name-outside-the-allowed-charset-rejected|_error#13|AssertionError|raise AssertionError(f"expected the publish to be refused, but it succeeded: {result!r}")
Issue-Key: sns__sqs.a-message-filtered-out-at-publish-time-is-never-redelivered-when-the-policy-is-relaxed|_drain#38|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: sns__sqs.sns-subscription-dlq-captures-the-notification-the-endpoint-policy-denied|_drain#32|Timeout|resp = sqs.receive_message(
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# sns__sqs: the delivered notification envelope omits SignatureVersion/Signature/SigningCertURL, Publish accepts message-attribute names outside the allowed charset, publish-time filter evaluation is unobservable, and a policy-denied notification is not dead-lettered (4 causes)

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Four net-new root causes on the SNS->SQS path. Open SNS issues exist (20261025T025317Z-sns-is-over-permissive-ten-validation-rules-aws.md, 20261025T064350Z-sns-9-behavioral-divergences-unsigned-notification-envelopes-unvalidated.md, 20261026T051123Z-sns-6-net-new-behavioral-divergences-an-empty.md) but none carries these four checks' replay keys.

Cause 1 of 4: the notification envelope delivered to an SQS subscriber is missing its three signature members.
AWS: a non-raw SNS notification delivered to an SQS queue is a JSON envelope whose keys appear in a fixed canonical order: Type, MessageId, TopicArn, Message, Timestamp, SignatureVersion, Signature, SigningCertURL, UnsubscribeURL (with Subject and MessageAttributes inserted when present). SignatureVersion, Signature and SigningCertURL are always present on a notification, and are what lets a subscriber verify authenticity. Recorded behavior only for the exact ordering; the member set is AWS-documented.
Emulator: the envelope carries only ['Type', 'MessageId', 'TopicArn', 'Message', 'Timestamp', 'UnsubscribeURL'] - the three signature members are absent entirely.
Why: any subscriber that verifies SNS message signatures (the documented and recommended practice, and mandatory for HTTPS endpoints) cannot run: there is no Signature to check and no SigningCertURL to fetch. Parsers that treat the three as required fail outright on the envelope.
Fix: the SNS notification serializer used by the Publish delivery path - emit SignatureVersion, Signature and SigningCertURL in the canonical key position for every non-raw notification.
Traces: sns__sqs.sqs-envelope-raw-bytes-are-pretty-printed-in-canonical-key-order.

Cause 2 of 4: Publish accepts message-attribute names containing characters outside the allowed charset.
AWS: a message attribute name may use only alphanumerics, hyphen, underscore and period; a name containing any other character is refused. Publish models InvalidParameterException (Smithy: errorKind client, httpError 400, awsQueryError code InvalidParameter) and InvalidParameterValueException (400, ParameterValueInvalid) exactly for this class. AWS's message is the invalid-parameter sentence identifying the offending message-attribute name - fixed text plus the attribute name taken verbatim from the request.
Emulator: the publish succeeds with HTTP 200 and a MessageId for every out-of-charset name tried.
Why: an attribute name AWS rejects is accepted and then propagated to subscribers, so a client that works here breaks at the first real publish; and conversely, code written to handle the 400 is never exercised.
Fix: the Publish handler's message-attribute validation (and the same validation on PublishBatch entries), refusing with InvalidParameterException at HTTP 400 and AWS's wording naming the offending attribute name.
Traces: sns__sqs.message-attribute-name-outside-the-allowed-charset-rejected.

Cause 3 of 4: a message filtered out at publish time must never be redelivered when the policy is later relaxed - and this cannot be established.
AWS: a subscription's FilterPolicy is evaluated once, at publish time. A message the policy rejected is dropped then and there; SNS keeps no backlog, so relaxing or clearing the policy afterwards delivers nothing retroactively, while publishes issued after the change are delivered normally. Recorded behavior only for the counts and timing.
Emulator: the check runs to the harness cap and is terminated at 60 s - the post-relaxation control delivery that the check needs never arrives, so it cannot distinguish "correctly not redelivered" from "nothing is ever delivered".
Why: a client cannot confirm the publish-time evaluation semantics, which decide whether changing a filter policy is safe on a live topic. If the emulator were in fact replaying a backlog, a consumer would see duplicate/stale events; the check that would catch this is unable to run.
Fix: the Publish delivery path's filter-policy evaluation (evaluate once at publish, keep no per-subscription backlog) and the delivery path to SQS subscribers that the control arm depends on.
Traces: sns__sqs.a-message-filtered-out-at-publish-time-is-never-redelivered-when-the-policy-is-relaxed.

Cause 4 of 4: a notification the destination queue's policy denied is not captured by the subscription's redrive DLQ.
AWS: when a subscription has a RedrivePolicy naming a dead-letter queue, a notification that cannot be delivered to the endpoint - including one the destination SQS queue's resource policy denies (SNS has no sqs:SendMessage grant) - is moved to that DLQ after the delivery attempts are exhausted, rather than being discarded. Recorded behavior only for the retry timing; the redrive contract is AWS-documented.
Emulator: the check runs to the harness cap and is terminated at 60 s - neither the denied delivery nor any dead letter is observable, so the notification is silently lost.
Why: subscription redrive is the only place a client can recover notifications that a mis-permissioned endpoint rejected. With no dead letter, those messages are gone and the failure is invisible - a silent data-loss mode in exactly the configuration redrive exists to protect.
Fix: the SNS subscription delivery path's failure handling - on a delivery refusal (including an endpoint-policy denial) exhaust the delivery policy's attempts and then enqueue the notification to the RedrivePolicy dead-letter queue.
Traces: sns__sqs.sns-subscription-dlq-captures-the-notification-the-endpoint-policy-denied.

## Evidence
Cause 1 - "AssertionError: envelope keys for 'sim2real-layout-bare-...' are ['Type', 'MessageId', 'TopicArn', 'Message', 'Timestamp', 'UnsubscribeURL'], expected ['Type', 'MessageId', 'TopicArn', 'Message', 'Timestamp', 'SignatureVersion', 'Signature', 'SigningCertURL', 'UnsubscribeURL']". Cause 2 - expected a 400 InvalidParameter refusal; actual "AssertionError: expected the publish to be refused, but it succeeded: {'MessageId': '21374e3d-...', 'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}". Cause 3 - expected: nothing redelivered after the policy is relaxed, with a fresh publish delivered as control; actual "Error: Check timed out after 60.0s". Cause 4 - expected: the denied notification appears in the subscription's dead-letter queue; actual "Error: Check timed out after 60.0s".

## Steps to Reproduce
Cause 1: CreateTopic, CreateQueue, Subscribe with RawMessageDelivery=false, Publish, ReceiveMessage and inspect the envelope keys - SignatureVersion/Signature/SigningCertURL are absent.
Cause 2: Publish with MessageAttributes containing a name with a character outside [A-Za-z0-9_.-] - 200 with a MessageId instead of 400 InvalidParameter.
Cause 3: Subscribe with a FilterPolicy that rejects a message, Publish that message, then SetSubscriptionAttributes to relax/clear the policy and publish a fresh matching message - the fresh control delivery never arrives.
Cause 4: Subscribe an SQS queue whose resource policy denies SNS sqs:SendMessage, SetSubscriptionAttributes(RedrivePolicy) naming a DLQ, Publish - no dead letter is ever written.
