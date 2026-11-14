Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.op-set-subscription-attributes-filter-policy-roundtrip sns.subscription-delivery-policy-reads-back-normalized-on-an-sqs-subscription sns.raw-message-delivery-flag-is-case-insensitive-and-reads-back-lowercase sns.publish-message-structure-json-semantics sns.message-structure-json-stringifies-scalar-branches-and-ignores-object-branches sns.firehose-raw-subscription-writes-the-bare-message-bodies
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns|SetSubscriptionAttributes|InvalidParameter
Issue-Key: sns.subscription-delivery-policy-reads-back-normalized-on-an-sqs-subscription|run#13|AssertionError|assert stored == _STORED, f"the stored DeliveryPolicy is {stored!r}, expected {_STORED!r}"
Issue-Key: sns.raw-message-delivery-flag-is-case-insensitive-and-reads-back-lowercase|run#14|AssertionError|assert (code, message) == (
Issue-Key: sns.publish-message-structure-json-semantics|run#13|AssertionError|assert message2 == 'Invalid parameter: Message Structure - JSON message body failed to parse', (
Issue-Key: sns.message-structure-json-stringifies-scalar-branches-and-ignores-object-branches|_receive_keyed#15|Timeout|resp = sqs.receive_message(
Issue-Key: sns.firehose-raw-subscription-writes-the-bare-message-bodies|_records#9|Timeout|time.sleep(5)
Issue-Commit: 388872f1987f74fe9682288e62ee8e18a6dfb8b9

# SNS: 6 net-new behavioral divergences — an empty-value FilterPolicy clear is refused, the stored subscription DeliveryPolicy is normalized wrongly, two refusal messages diverge from AWS's text, and json-branch / firehose-raw deliveries never arrive (6 causes)

> _Note: this issue was identified against the code base at commit `388872f1987f74fe9682288e62ee8e18a6dfb8b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Six net-new root causes found by replaying the behavioral suite against the SNS emulator. Each is numbered below with its own failing check ids. ALL SIX must be closed before this issue is submitted.

=== Cause 1 of 6: SetSubscriptionAttributes refuses an empty AttributeValue for FilterPolicy instead of clearing the policy ===
AWS: Smithy (com.amazonaws.sns#SetSubscriptionAttributes): SubscriptionArn and AttributeName are @required, AttributeValue is OPTIONAL; the AttributeName documentation lists FilterPolicy as a settable subscription attribute; the modelled client refusal is InvalidParameterException (awsQueryError InvalidParameter, HTTP 400). Recorded behavior: writing FilterPolicy with an EMPTY AttributeValue is accepted (HTTP 200) and REMOVES the attribute — the following GetSubscriptionAttributes no longer contains a FilterPolicy key at all, and the subscription returns to unfiltered delivery. An empty value is the documented way to clear a subscription attribute; it is not a policy document and must not be fed to the JSON parser.
Emulator: the clear is refused — HTTP 400, code InvalidParameter, message 'Invalid parameter: Attributes Reason: FilterPolicy: failed to parse JSON. Unexpected end-of-input: expected a value\nat [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 0]'. So the FilterPolicy set earlier in the same check can never be removed, and the stale policy keeps filtering.
Why: a client cannot un-filter a subscription. Every teardown/reset path (tests, IaC that removes a FilterPolicy property, operators widening a subscription) gets a 400 from a call AWS answers 200, and the subscription keeps silently dropping messages the client believes it is now receiving.
Fix: the SetSubscriptionAttributes handler's attribute validation (services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/set_subscription_attributes.rs): an empty/absent AttributeValue must be treated as a REMOVE of the named attribute, before any JSON parse of the value is attempted.
Related open issue that does NOT cover this: 20261025T064350Z-sns-9-behavioral-divergences-unsigned-notification-envelopes-unvalidated.md, Cause 8, hypothesises that 'the empty-value FilterPolicy write does not clear the stored policy' from a DELIVERY timeout (sns.clearing-a-filter-policy-with-an-empty-value-restores-unfiltered-delivery). It does not carry this check's replay key, and this check shows the harder fact: the clear is REFUSED at the API with a 400, it is not merely ineffective.
Traces: sns.op-set-subscription-attributes-filter-policy-roundtrip.

=== Cause 2 of 6: a subscription DeliveryPolicy is stored in a different normalized form than AWS's — unsupplied retry fields are filled with values, and the requestPolicy section is dropped ===
AWS: recorded behavior only (the delivery-policy document is not modelled; the attribute is an opaque JSON string, see the SetSubscriptionAttributes AttributeName doc entry for DeliveryPolicy). The recording shows that after writing {"healthyRetryPolicy": {"numRetries": N, "minDelayTarget": A, "maxDelayTarget": B}} on an sqs subscription, SNS stores and reads back the FULLY EXPANDED normalized document: healthyRetryPolicy carries every retry field, with the ones the request did not supply set to JSON null (numMaxDelayRetries, numNoDelayRetries, numMinDelayRetries, backoffFunction all null), plus the sibling sections sicklyRetryPolicy: null, throttlePolicy: null, requestPolicy: null and guaranteed: false. Fixed parts of the document: the member names and the null/false placeholders; variable parts: only the three numbers the request supplied.
Emulator: unsupplied retry fields are INVENTED rather than nulled, and one section is missing entirely — it stores numMaxDelayRetries: 0, numNoDelayRetries: 0, numMinDelayRetries: 0, backoffFunction: 'linear' and has no requestPolicy member at all.
Why: a client that reads the attribute back to learn what is actually configured sees four values it never wrote (and cannot distinguish 'unset' from 'set to 0/linear'), and a client that diffs the stored document against AWS's normalized form — IaC drift detection, config assertions — reports a permanent difference on a subscription it just wrote.
Fix: the DeliveryPolicy normalization applied when SetSubscriptionAttributes stores the attribute and when GetSubscriptionAttributes serializes it (set_subscription_attributes / get_subscription_attributes handlers and the subscription-attribute state): absent retry members normalize to null, never to a default value, and requestPolicy must be present as null.
Related fixed issue: 20261025T025317Z-sns-refuses-four-calls-aws-accepts-deliverypolicy-on.md made this call accepted (it is no longer refused — the write returns 200); what remains and is net-new is the stored/read-back SHAPE of the document.
Traces: sns.subscription-delivery-policy-reads-back-normalized-on-an-sqs-subscription.

=== Cause 3 of 6: the RawMessageDelivery invalid-value refusal message is not AWS's text and drops the offending value ===
AWS: Smithy: SetSubscriptionAttributes declares InvalidParameterException (awsQueryError InvalidParameter, HTTP 400); the AttributeName doc states RawMessageDelivery is a boolean-valued attribute ('When set to true, enables raw message delivery'). Recorded behavior: a non-boolean value is refused 400 / InvalidParameter with the message 'Invalid parameter: RawMessageDelivery: Invalid value [<value>]. Must be true or false.' — fixed text: 'Invalid parameter: RawMessageDelivery: Invalid value [' ... ']. Must be true or false.'; the single variable part is the rejected value taken verbatim from the request (in this replay, 'yes').
Emulator: status and code are right (400 / InvalidParameter) but the message is a different template that also discards the rejected value: 'Invalid parameter: Attributes Reason: RawMessageDelivery - the value must be true or false'.
Why: the error message is the only place the offending value appears, so an operator or log consumer cannot tell WHICH value was refused; and clients/tests that match AWS's documented text (or tooling keyed on it) mis-classify a legitimate validation refusal.
Fix: the SetSubscriptionAttributes handler's RawMessageDelivery validation, where the InvalidParameter error is constructed — emit AWS's fixed text with the request's value interpolated in the [brackets].
Related: the preceding assertions in the same check pass, so the case-insensitive acceptance of 'True'/'FALSE' and the lowercase read-back now work; the fixed issue 20261025T025317Z-sns-refusal-messages-do-not-match-the-fixed.md covered ten OTHER message families and not this one, and the open 20261025T103512Z-sns-four-net-new-behavioral-divergences-publish-accepts.md carries two different message texts.
Traces: sns.raw-message-delivery-flag-is-case-insensitive-and-reads-back-lowercase.

=== Cause 4 of 6: the MessageStructure=json unparseable-body refusal message is not AWS's text ===
AWS: Smithy: Publish declares InvalidParameterException (awsQueryError InvalidParameter, HTTP 400); Publish.MessageStructure documents that when it is 'json' the Message 'must be a syntactically valid JSON object' and 'contain at least a top-level JSON key of "default"'. Recorded behavior: a Message that does not parse as JSON is refused 400 / InvalidParameter with the WHOLLY FIXED text 'Invalid parameter: Message Structure - JSON message body failed to parse' (no request-derived part at all), the sibling condition being 'Invalid parameter: Message Structure - No default entry in JSON message body'.
Emulator: code is right (InvalidParameter) but the message is 'Invalid parameter: Message - message must be a syntactically valid JSON object' — a different, non-AWS template that also attributes the fault to the Message parameter rather than to Message Structure. Note the sibling no-default-entry message DOES match in the same check (that assertion passes first), so only the parse-failure branch diverges.
Why: the two json-structure failures are distinguished only by this text; a client that branches on it (or an operator reading it) cannot tell 'body is not JSON' from 'JSON has no default entry', and matchers written against AWS fail.
Fix: the Publish handler's MessageStructure=json validation (services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/publish.rs), where the parse-failure InvalidParameter error is constructed.
Traces: sns.publish-message-structure-json-semantics.

=== Cause 5 of 6: json-structured publishes with non-string branches are accepted but nothing is ever delivered, so the check hangs ===
AWS: recorded behavior only (branch selection is observable only in the delivered Message). The recording shows that with MessageStructure=json the selected branch need not be a string: a number, true or null is delivered as its JSON text ('7', 'true', 'null'), an empty-string branch is delivered as an empty Message, an object or null protocol branch is IGNORED so the 'default' is delivered instead, and a publish with any MessageStructure other than 'json' delivers the body verbatim and unparsed. An object/array 'default' is refused as 'Invalid parameter: Message Structure - No default entry in JSON message body'.
Emulator: each of the six json-structured publishes and the MessageStructure='string' publish is ACCEPTED (a MessageId is returned), but no notification ever reaches the subscribed SQS queue — the check is killed at the 60.0 s harness cap while polling for the per-branch bodies. There is no error to diagnose: the publish succeeds and the queue stays empty. Hypothesis: the json branch-resolution path in the delivery leg drops the message when the selected branch is not a plain string, so none of the deliveries (including the 'string'-structure one issued afterwards) complete.
Why: a client fanning out per-protocol bodies gets 200 on every publish and no message at any subscriber, and its own timeout is the only symptom.
Fix: the MessageStructure=json branch resolution in the Publish-to-subscription delivery path (the per-protocol message selection that turns the parsed branch into the delivered Message text), in the sns publish/delivery component.
Related open issue that does NOT carry this replay key: 20261025T025317Z-sns-performs-no-out-of-band-delivery-a.md (4 causes).
Traces: sns.message-structure-json-stringifies-scalar-branches-and-ignores-object-branches.

=== Cause 6 of 6: a firehose subscription with RawMessageDelivery accepts the attribute but puts no record into the delivery stream, so the check hangs ===
AWS: recorded behavior only. The recording shows a `firehose` subscription accepting RawMessageDelivery=true (unlike a lambda subscription) and reading it back as 'true', and SNS then putting each published message body into the delivery stream VERBATIM — the stream's bucket holds the published JSON documents themselves, back to back, with no Type/MessageId/TopicArn envelope and no added delimiter (envelope records, by contrast, are newline-terminated).
Emulator: the attribute read-back is correct (RawMessageDelivery reads 'true' — that assertion passes), but no record ever reaches the stream's bucket: the check is killed at the 60.0 s harness cap inside its warm-up loop, which publishes and polls the bucket until one record appears. So nothing about the raw record format can even be observed; SNS puts no records into a Firehose subscriber at all.
Why: a Firehose fan-out subscriber receives nothing while every Publish returns 200 — an archival/analytics pipeline built on SNS->Firehose->S3 silently stores no data and reports no error.
Fix: the firehose protocol leg of the SNS subscription delivery path (assume the subscription's SubscriptionRoleArn and PutRecord/PutRecordBatch into the delivery stream), plus the raw-vs-envelope record framing for that protocol.
Related open issue that does NOT carry this replay key: 20261025T025317Z-sns-performs-no-out-of-band-delivery-a.md (4 causes) covers the ENVELOPE firehose checks. Related fixed issue 20261025T025317Z-sns-refuses-four-calls-aws-accepts-deliverypolicy-on.md made the firehose RawMessageDelivery attribute acceptable; the delivery itself still never happens.
Traces: sns.firehose-raw-subscription-writes-the-bare-message-bodies.

## Evidence
Cause 1 — expected HTTP 200 and the attribute removed, actual a 400 refusal:
  sns.op-set-subscription-attributes-filter-policy-roundtrip:
    botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameter) when calling the SetSubscriptionAttributes operation: Invalid parameter: Attributes Reason: FilterPolicy: failed to parse JSON. Unexpected end-of-input: expected a value
    at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 0]
  (raised by: set_subscription_attributes(SubscriptionArn=sub, AttributeName="FilterPolicy", AttributeValue=""); the earlier write of {"tier":["gold","silver"]} and the FilterPolicyScope write both succeeded)

Cause 2 — stored DeliveryPolicy after writing {"healthyRetryPolicy": {"numRetries": 5, "minDelayTarget": 1, "maxDelayTarget": 10}}:
  expected: {'healthyRetryPolicy': {'minDelayTarget': 1, 'maxDelayTarget': 10, 'numRetries': 5, 'numMaxDelayRetries': None, 'numNoDelayRetries': None, 'numMinDelayRetries': None, 'backoffFunction': None}, 'sicklyRetryPolicy': None, 'throttlePolicy': None, 'requestPolicy': None, 'guaranteed': False}
  actual:   {'healthyRetryPolicy': {'minDelayTarget': 1, 'maxDelayTarget': 10, 'numRetries': 5, 'numMaxDelayRetries': 0, 'numNoDelayRetries': 0, 'numMinDelayRetries': 0, 'backoffFunction': 'linear'}, 'sicklyRetryPolicy': None, 'throttlePolicy': None, 'guaranteed': False}
  (AssertionError: the stored DeliveryPolicy is ... expected ...)

Cause 3 — code matched (InvalidParameter), message diverged, for SetSubscriptionAttributes(RawMessageDelivery='yes'):
  expected: 'Invalid parameter: RawMessageDelivery: Invalid value [yes]. Must be true or false.'
  actual:   'Invalid parameter: Attributes Reason: RawMessageDelivery - the value must be true or false'
  (AssertionError: RawMessageDelivery 'yes': got 'InvalidParameter' 'Invalid parameter: Attributes Reason: RawMessageDelivery - the value must be true or false')

Cause 4 — code matched (InvalidParameter), message diverged, for Publish(Message='not-json-at-all', MessageStructure='json'):
  expected: 'Invalid parameter: Message Structure - JSON message body failed to parse'
  actual:   'Invalid parameter: Message - message must be a syntactically valid JSON object'
  (AssertionError: expected the exact message 'Invalid parameter: Message Structure - JSON message body failed to parse', got 'Invalid parameter: Message - message must be a syntactically valid JSON object'; the preceding assertion on the no-default-entry message passed)

Cause 5 — 'Check timed out after 60.0s' on sns.message-structure-json-stringifies-scalar-branches-and-ignores-object-branches: all six json-structured publishes plus the MessageStructure='string' publish returned a MessageId, and the subsequent poll of the subscribed queue for the seven expected bodies never completed.

Cause 6 — 'Check timed out after 60.0s' on sns.firehose-raw-subscription-writes-the-bare-message-bodies: GetSubscriptionAttributes reported RawMessageDelivery='true' on the firehose subscription, then the warm-up loop (publish, then poll the stream's bucket for the published record) never saw a single record and the check was killed at the cap.

## Steps to Reproduce
1. git_clone the repo and run the behavioral suite (run_behavioral_suite) against the built emulator.
2. Re-replay exactly the check ids in trace_ids. Per cause:
   - Cause 1: CreateTopic; Subscribe an SQS queue; SetSubscriptionAttributes(FilterPolicy='{"tier":["gold","silver"]}'); then SetSubscriptionAttributes(SubscriptionArn=..., AttributeName='FilterPolicy', AttributeValue='') — observe the 400 InvalidParameter 'failed to parse JSON' instead of a 200 whose GetSubscriptionAttributes no longer lists FilterPolicy.
   - Cause 2: Subscribe an sqs endpoint, SetSubscriptionAttributes(DeliveryPolicy='{"healthyRetryPolicy":{"numRetries":5,"minDelayTarget":1,"maxDelayTarget":10}}'), then GetSubscriptionAttributes and json.loads the DeliveryPolicy — compare member-by-member with AWS's normalized document (nulls, requestPolicy present).
   - Cause 3: SetSubscriptionAttributes(AttributeName='RawMessageDelivery', AttributeValue='yes') on an sqs subscription; read Error.Code and Error.Message.
   - Cause 4: Publish(TopicArn=..., Message='not-json-at-all', MessageStructure='json'); read Error.Message.
   - Cause 5: Subscribe a real SQS queue and confirm it, then Publish with MessageStructure='json' and bodies {"default":D,"sqs":{"nested":1}}, {"default":D,"sqs":null}, {"default":"x","sqs":<number>}, {"default":true}, {"default":null}, {"default":"y","sqs":""}, plus one Publish with MessageStructure='string'; poll the queue for the selected branch of each.
   - Cause 6: create a DirectPut Firehose delivery stream writing to a bucket, create a subscription role that may PutRecord into it, Subscribe(Protocol='firehose', Endpoint=<stream arn>, Attributes={'SubscriptionRoleArn':..., 'RawMessageDelivery':'true'}), Publish, and poll the bucket for the record.
