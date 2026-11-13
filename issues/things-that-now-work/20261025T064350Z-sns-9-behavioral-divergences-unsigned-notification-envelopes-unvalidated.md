Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.sqs-subscription-delivers-the-published-message-unconditionally sns.notification-signature-verifies-against-the-signing-certificate sns.signature-version-two-switches-the-delivered-digest-to-sha256 sns.create-sms-sandbox-invalid-number-rejected sns.create-sms-sandbox-unsupported-country-code-rejected sns.verify-sms-sandbox-wrong-otp-rejected sns.get-sms-sandbox-account-status-throttle-error-shape sns.authorization-error-explicit-deny-returns-correct-code sns.get-data-protection-policy-malformed-arn-rejected sns.put-data-protection-policy-malformed-json-rejected sns.list-topics-and-subscriptions-forged-next-token-rejected sns.publish-batch-aggregate-size-at-the-exact-262144-byte-boundary sns.publish-batch-validation-is-request-level sns.raw-delivery-normalises-number-attributes-the-envelope-carries-verbatim sns.standard-topic-message-group-id-reaches-the-queue-as-its-message-group-id sns.filter-policy-exists-false-and-anything-but-decide-delivery sns.string-array-message-attribute-matches-a-member-and-binary-is-ignored sns.clearing-a-filter-policy-with-an-empty-value-restores-unfiltered-delivery sns.fifo-concurrent-publishers-across-many-groups-keep-each-group-in-order sns.fifo-message-group-dedup-scope-holds-per-group-under-concurrent-publishers sns.publish-batch-one-call-two-message-groups-each-keeps-its-own-order
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.sqs-subscription-delivers-the-published-message-unconditionally|run#14|AssertionError|assert set(envelope) >= required, (
Issue-Key: sns.notification-signature-verifies-against-the-signing-certificate|run#10|KeyError|assert envelope["SignatureVersion"] == declared, (
Issue-Key: sns.signature-version-two-switches-the-delivered-digest-to-sha256|_assert_signed_with#7|KeyError|got_version = envelope["SignatureVersion"]
Issue-Key: sns.create-sms-sandbox-invalid-number-rejected|run#2|AssertionError|raise AssertionError(
Issue-Key: sns.create-sms-sandbox-unsupported-country-code-rejected|_rejected_error#2|AssertionError|raise AssertionError(
Issue-Key: sns.verify-sms-sandbox-wrong-otp-rejected|run#10|AssertionError|assert code == "VerificationException", f"expected VerificationException, got {code}"
Issue-Key: sns.get-sms-sandbox-account-status-throttle-error-shape|run#7|AssertionError|assert all(code == "Throttling" for code in codes), codes
Issue-Key: sns.authorization-error-explicit-deny-returns-correct-code|run#17|AssertionError|assert denied[0] == "AuthorizationError", (
Issue-Key: sns.get-data-protection-policy-malformed-arn-rejected|run#5|AssertionError|assert before[0] == "empty" or before == ("error", "NotFound"), (
Issue-Key: sns.put-data-protection-policy-malformed-json-rejected|run#8|AssertionError|assert well_formed_code == "InvalidAction", (
Issue-Key: sns.list-topics-and-subscriptions-forged-next-token-rejected|run#8|AssertionError|assert code == "InvalidParameter", f"ListSubscriptions with a forged NextToken gave {code!r}"
Issue-Key: sns.publish-batch-aggregate-size-at-the-exact-262144-byte-boundary|run#20|AssertionError|assert message == "The length of all the messages put together is more than the limit.", (
Issue-Key: sns.publish-batch-validation-is-request-level|run#15|AssertionError|raise AssertionError(
Issue-Key: sns.raw-delivery-normalises-number-attributes-the-envelope-carries-verbatim|run#17|AssertionError|assert raw_attrs == want_raw, (
Issue-Key: sns.standard-topic-message-group-id-reaches-the-queue-as-its-message-group-id|run#14|AssertionError|assert got == expected, (
Issue-Key: sns.filter-policy-exists-false-and-anything-but-decide-delivery|_await_bodies#20|Timeout|resp = sqs.receive_message(
Issue-Key: sns.string-array-message-attribute-matches-a-member-and-binary-is-ignored|_await_envelopes#24|Timeout|resp = sqs.receive_message(
Issue-Key: sns.clearing-a-filter-policy-with-an-empty-value-restores-unfiltered-delivery|_drain_until#13|Timeout|for msg in sqs.receive_message(
Issue-Key: sns.fifo-concurrent-publishers-across-many-groups-keep-each-group-in-order|_drain#16|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2,
Issue-Key: sns.fifo-message-group-dedup-scope-holds-per-group-under-concurrent-publishers|_drain#17|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2,
Issue-Key: sns.publish-batch-one-call-two-message-groups-each-keeps-its-own-order|_drain_until_count#22|Timeout|resp = sqs.receive_message(
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# SNS: 9 behavioral divergences — unsigned notification envelopes, unvalidated SMS-sandbox numbers, NotImplemented in place of modelled refusals, PublishBatch validation/message gaps, raw-delivery Number canonicalisation, dropped MessageGroupId, and filter-policy/FIFO deliveries that never arrive (9 causes)

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Nine net-new root causes found by replaying the behavioral suite against the SNS emulator. Each is numbered below with its own failing check ids. All nine must be closed before this issue is submitted.

=== Cause 1 of 9: the delivered notification envelope carries no signature fields, and the topic SignatureVersion attribute is inert ===
AWS: recorded behavior only (the out-of-band envelope is not in the Smithy model; Publish's modelled output is just MessageId/SequenceNumber). The recordings show every SNS `Type=Notification` envelope delivered to an SQS subscriber carrying, besides Type/MessageId/TopicArn/Message/Timestamp, the three signature members `SignatureVersion`, `Signature` and `SigningCertURL`; AWS documents SignatureVersion "1" (SHA-1) as the topic default and "2" (SHA-256) as selectable through the topic attribute `SignatureVersion`, with the canonical signed string unchanged between the two.
Emulator: the delivered envelope carries only ['Message','MessageId','Timestamp','TopicArn','Type'] — the signature trio is absent entirely, so reading `envelope["SignatureVersion"]` raises KeyError, and setting the topic attribute SignatureVersion="2" changes nothing in the delivered payload.
Why: a client that verifies notification authenticity (the documented SNS pattern: fetch SigningCertURL, rebuild the canonical string, verify Signature with the digest selected by SignatureVersion) cannot run at all against this emulator — every notification looks unsigned and unverifiable, and an application that rejects unsigned envelopes drops all traffic.
Fix: the SNS notification-envelope serializer used by the subscription delivery path, plus the topic `SignatureVersion` attribute handling that selects the digest.
Traces: sns.sqs-subscription-delivers-the-published-message-unconditionally, sns.notification-signature-verifies-against-the-signing-certificate, sns.signature-version-two-switches-the-delivered-digest-to-sha256.

=== Cause 2 of 9: CreateSMSSandboxPhoneNumber accepts phone numbers AWS refuses ===
AWS: Smithy: CreateSMSSandboxPhoneNumber.PhoneNumber is required, targets PhoneNumberString with @pattern `^(\+[0-9]{8,}|[0-9]{0,9})$` and @length max 20; the operation's modelled client errors include InvalidParameterException (awsQueryError InvalidParameter, HTTP 400) and UserErrorException (UserError, HTTP 400). A non-E.164 number violates that pattern, and a well-formed number with an unsupported country code is refused with a 400 client error (recorded behavior: refused).
Emulator: both the non-E.164 number and the unsupported-country-code number '+0123456789' are accepted — HTTP 200 with an empty CreateSMSSandboxPhoneNumberResult and no error at all.
Why: a client's number-validation path is never exercised; bad numbers are silently registered, so code that relies on the 400 to surface a typo to a user sees success and then an unexplainably dead sandbox entry.
Fix: CreateSMSSandboxPhoneNumber handler's input validation (pattern/length enforcement and the country-code gate).
Related open issues that do NOT cover this: 20261025T025317Z-sns-createsmssandboxphonenumber-stub.md describes the operation as an unimplemented stub, but it is now answering 200 — the missing part is validation; 20261025T025317Z-sns-is-over-permissive-ten-validation-rules-aws.md already carries ten other causes and not these replay keys.
Traces: sns.create-sms-sandbox-invalid-number-rejected, sns.create-sms-sandbox-unsupported-country-code-rejected.

=== Cause 3 of 9: six refusal paths answer error code 'NotImplemented' where AWS answers a modelled error ===
AWS: Smithy: VerifySMSSandboxPhoneNumber declares VerificationException (HTTP 400) for a wrong one-time password; GetSMSSandboxAccountStatus declares ThrottledException (awsQueryError Throttled, HTTP 429) and AuthorizationErrorException (AuthorizationError, HTTP 403); GetDataProtectionPolicy declares NotFoundException (NotFound, 404) and InvalidParameterException (InvalidParameter, 400) and returns an empty DataProtectionPolicy when no policy is installed; ListSubscriptions declares InvalidParameterException (InvalidParameter, 400) for a NextToken it did not mint. Recorded behavior: PutDataProtectionPolicy is refused with code InvalidAction on accounts where SNS message data protection has been withdrawn; a burst above the hard 10-TPS GetSMSSandboxAccountStatus quota is refused Throttling; an IAM policy carrying an explicit Deny for sns:GetSMSSandboxAccountStatus is refused AuthorizationErrorException.
Emulator: every one of those six calls answers error code 'NotImplemented' with message 'operation <Op> is scaffolded but not implemented in this emulator'. Observed codes: 'NotImplemented' for VerifySMSSandboxPhoneNumber (expected VerificationException), 'NotImplemented' x150 for the GetSMSSandboxAccountStatus burst (expected Throttling), 'NotImplemented' for the explicit-Deny GetSMSSandboxAccountStatus (expected AuthorizationErrorException), ('error','NotImplemented') for GetDataProtectionPolicy on a well-formed ARN with no policy (expected empty content or NotFound), 'NotImplemented' for PutDataProtectionPolicy (expected InvalidAction), 'NotImplemented' for ListSubscriptions with a forged NextToken (expected InvalidParameter).
Why: NotImplemented is not a modelled SNS error, so no SDK error-handling branch matches it — retry-on-Throttling, deny-handling, not-found-handling and pagination-token-recovery code all fall through to a generic failure.
Fix: implement these operation handlers and their refusal paths (verify_sms_sandbox_phone_number, get_sms_sandbox_account_status, get_data_protection_policy, put_data_protection_policy, list_subscriptions) so each returns its modelled status and code.
Related open issues that name the stubs but do not carry these replay keys or these required refusals: 20261025T025317Z-sns-getsmssandboxaccountstatus-stub.md, 20261025T025317Z-sns-getdataprotectionpolicy-stub.md, 20261025T025317Z-sns-putdataprotectionpolicy-stub.md, 20261025T025317Z-sns-listsubscriptions-stub.md.
Traces: sns.verify-sms-sandbox-wrong-otp-rejected, sns.get-sms-sandbox-account-status-throttle-error-shape, sns.authorization-error-explicit-deny-returns-correct-code, sns.get-data-protection-policy-malformed-arn-rejected, sns.put-data-protection-policy-malformed-json-rejected, sns.list-topics-and-subscriptions-forged-next-token-rejected.

=== Cause 4 of 9: the PublishBatch aggregate-size refusal message is computed from the request instead of AWS's fixed text ===
AWS: Smithy: PublishBatch declares BatchRequestTooLongException (awsQueryError BatchRequestTooLong, HTTP 400) for a batch whose bodies total more than 262144 bytes. AWS returns that code with a FIXED message that is not derived from the request: 'The length of all the messages put together is more than the limit.' — the rule is known (a constant string for this condition), so the text is a requirement here, not an example.
Emulator: the status and code are right (400 / BatchRequestTooLong) but the message is built from the request: 'The length of all the batch messages put together is more than the limit: 262145 bytes exceeds the topic's limit of 262144 bytes.'
Why: clients and tests that branch on the documented constant (and AWS log-analysis tooling keyed on it) do not match, so a legitimate size refusal is mis-classified.
Fix: the PublishBatch handler's aggregate-size validation, where the BatchRequestTooLong error is constructed.
Related open issue: 20261025T025317Z-sns-refusal-messages-do-not-match-the-fixed.md carries ten other message-text causes and not this replay key.
Traces: sns.publish-batch-aggregate-size-at-the-exact-262144-byte-boundary.

=== Cause 5 of 9: PublishBatch performs no validation of entry message attributes ===
AWS: Smithy: PublishBatch declares InvalidParameterException (InvalidParameter, 400) and InvalidParameterValueException (ParameterValueInvalid, 400); a malformed Number message attribute in any entry makes AWS refuse the WHOLE request with a 400 client error — it is request-level, so no entry is published and no per-entry Failed element is returned.
Emulator: the PublishBatch call containing a malformed Number attribute SUCCEEDS (HTTP 200) and the entry is reported Successful.
Why: a client that relies on SNS to reject malformed typed attributes publishes corrupt attributes to every subscriber, and filter policies keyed on those attributes then behave unpredictably downstream.
Fix: PublishBatch handler's request-level validation of each entry's MessageAttributes (type/value consistency for Number), refusing before anything is published.
Related open issue: 20261025T025317Z-sns-fifo-deduplication-does-not-deduplicate-publishbatch-validates.md says PublishBatch 'validates per entry instead of per request'; here NO validation of Number attribute values happens at all, and that issue does not carry this replay key.
Traces: sns.publish-batch-validation-is-request-level.

=== Cause 6 of 9: raw delivery does not canonicalise Number message attributes ===
AWS: recorded behavior only. The recording shows a Number message attribute published as '+5', '.5', '1.' or '1.500' reaching a raw-delivery SQS subscriber as an SQS Number attribute in CANONICAL form — '5', '0.5', '1', '1.5' — while the non-raw JSON envelope of the same publish keeps the verbatim spelling.
Emulator: the raw subscriber receives the verbatim spellings: {'plus': ('Number','+5'), 'lead': ('Number','.5'), 'trail': ('Number','1.'), 'zeros': ('Number','1.500'), 'plain': ('Number','42')} where {'plus':'5','lead':'0.5','trail':'1','zeros':'1.5','plain':'42'} was required.
Why: a consumer that parses or string-compares the SQS Number attribute sees a different value than on AWS; '1.500' vs '1.5' breaks equality checks and dedup keys built from attribute values.
Fix: the raw-delivery attribute translation in the SNS subscription delivery path (SNS message attribute -> SQS message attribute conversion).
Traces: sns.raw-delivery-normalises-number-attributes-the-envelope-carries-verbatim.

=== Cause 7 of 9: a MessageGroupId published to a standard topic is not forwarded to the subscribed queue ===
AWS: Smithy: Publish.MessageGroupId exists on the standard-topic input too, as does PublishBatchRequestEntry's. Recorded behavior: SNS hands that value to each subscribed standard SQS queue as the delivered message's MessageGroupId system attribute — through Publish and per PublishBatch entry, for raw and enveloped subscriptions alike — while a publish with no group id arrives with no MessageGroupId, and a standard topic mints no SequenceNumber.
Emulator: every delivered message carries MessageGroupId=None, including the two publishes that supplied 'tenant-a' (single publish) and 'tenant-b' (batch entry): got {...'single-grouped': None, 'single-plain': None, 'batch-grouped': None, 'batch-plain': None}, expected {...'single-grouped': 'tenant-a', 'single-plain': None, 'batch-grouped': 'tenant-b', 'batch-plain': None}.
Why: this is how SNS feeds SQS fair queues — with the group id dropped, every tenant collapses into one group and per-tenant fairness/isolation silently disappears for a client that configured it.
Fix: the SNS->SQS delivery path's mapping of publish system attributes (MessageGroupId) onto the SendMessage it issues.
Traces: sns.standard-topic-message-group-id-reaches-the-queue-as-its-message-group-id.

=== Cause 8 of 9: filter-policy evaluation withholds messages that must be delivered, so the checks never terminate ===
AWS: recorded behavior only. The recordings show: a filter policy `{"<key>": [{"exists": false}]}` matches a message with no such attribute and an `anything-but` policy matches everything outside its value list, so the matching publishes are delivered; a `String.Array` attribute matches a policy key when ANY array member equals a policy value (Binary attributes are ignored); and clearing a subscription's FilterPolicy by setting an empty value restores unfiltered delivery.
Emulator: in all three cases the publish that must be delivered never arrives at the subscribed queue, so the check's delivery poll never completes and the check is killed at the 60.0 s harness cap. Plain (unfiltered) SNS->SQS delivery does work in this same run — other checks observe bodies arriving — so the message is being withheld by filter evaluation rather than by a dead delivery path. Hypothesis: `exists:false` / `anything-but` / String.Array member matching evaluate to no-match, and the empty-value FilterPolicy write does not clear the stored policy.
Why: a subscriber with any of these very common policies receives nothing on this emulator, and the client's own timeout is the only symptom — there is no error to diagnose.
Fix: the subscription filter-policy evaluator (exists/anything-but operators and String.Array member matching) and the SetSubscriptionAttributes FilterPolicy clear path.
Traces: sns.filter-policy-exists-false-and-anything-but-decide-delivery, sns.string-array-message-attribute-matches-a-member-and-binary-is-ignored, sns.clearing-a-filter-policy-with-an-empty-value-restores-unfiltered-delivery.

=== Cause 9 of 9: FIFO-topic delivery across multiple message groups never arrives, so the checks never terminate ===
AWS: recorded behavior only. The recordings show a FIFO topic delivering each message group's messages to its FIFO subscriber in publish order, holding deduplication scope per message group under concurrent publishers, and keeping each group's order when one PublishBatch call carries entries for two different groups.
Emulator: in all three cases nothing is delivered within the check's poll window and the check is killed at the 60.0 s harness cap — no per-group ordering, no per-group dedup scope and no two-group batch delivery can be observed at all.
Why: a FIFO fan-out client gets no messages and no error; ordering and exactly-once-per-group guarantees it was built on are unobservable, so it hangs rather than failing fast.
Fix: FIFO topic delivery in the subscription delivery path — per-message-group ordering and per-group deduplication scope, including the PublishBatch multi-group case.
Traces: sns.fifo-concurrent-publishers-across-many-groups-keep-each-group-in-order, sns.fifo-message-group-dedup-scope-holds-per-group-under-concurrent-publishers, sns.publish-batch-one-call-two-message-groups-each-keeps-its-own-order.

## Evidence
Cause 1 — expected vs actual:
  sns.sqs-subscription-delivers-the-published-message-unconditionally: AssertionError: the envelope is missing required fields ['Signature', 'SignatureVersion', 'SigningCertURL']; it carried ['Message', 'MessageId', 'Timestamp', 'TopicArn', 'Type']
  sns.notification-signature-verifies-against-the-signing-certificate: KeyError: 'SignatureVersion' (envelope["SignatureVersion"])
  sns.signature-version-two-switches-the-delivered-digest-to-sha256: KeyError: 'SignatureVersion' in _assert_signed_with(..., expect_version="1")

Cause 2 — expected 400 InvalidParameter/UserError, actual HTTP 200:
  sns.create-sms-sandbox-invalid-number-rejected: AssertionError: expected a rejection for a non-E.164 sandbox phone number, but the call SUCCEEDED: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  sns.create-sms-sandbox-unsupported-country-code-rejected: AssertionError: expected a well-formed but unsupported-country-code number ('+0123456789') to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Cause 3 — expected modelled error code, actual 'NotImplemented':
  sns.verify-sms-sandbox-wrong-otp-rejected: AssertionError: expected VerificationException, got NotImplemented
  sns.get-sms-sandbox-account-status-throttle-error-shape: AssertionError: ['NotImplemented', 'NotImplemented', ... ] (150-call burst; every code expected 'Throttling')
  sns.authorization-error-explicit-deny-returns-correct-code: AssertionError: a policy carrying both an explicit Allow and an explicit Deny for sns:GetSMSSandboxAccountStatus must refuse the real signed call with exactly AuthorizationErrorException, never a generic 403 or InternalErrorException; got 'NotImplemented'
  sns.get-data-protection-policy-malformed-arn-rejected: AssertionError: expected a well-formed ARN with no installed policy to report empty content or NotFound, got ('error', 'NotImplemented')
  sns.put-data-protection-policy-malformed-json-rejected: AssertionError: expected InvalidAction (feature withdrawn), got 'NotImplemented'
  sns.list-topics-and-subscriptions-forged-next-token-rejected: AssertionError: ListSubscriptions with a forged NextToken gave 'NotImplemented' (expected InvalidParameter)

Cause 4 — code matched (BatchRequestTooLong, 400), message diverged:
  expected: 'The length of all the messages put together is more than the limit.'
  actual:   'The length of all the batch messages put together is more than the limit: 262145 bytes exceeds the topic's limit of 262144 bytes.'
  (the same check's preceding assertion on code == 'BatchRequestTooLong' passed, and the exact-262144-byte batch was accepted and delivered)

Cause 5 — expected 400 refusal of the whole request, actual success:
  sns.publish-batch-validation-is-request-level: AssertionError: a PublishBatch with a malformed Number attribute was expected to be rejected, but it SUCCEEDED

Cause 6 — raw-delivery Number attributes:
  expected {'plus': ('Number','5'), 'lead': ('Number','0.5'), 'trail': ('Number','1'), 'zeros': ('Number','1.5'), 'plain': ('Number','42')}
  actual   {'zeros': ('Number','1.500'), 'lead': ('Number','.5'), 'plain': ('Number','42'), 'plus': ('Number','+5'), 'trail': ('Number','1.')}

Cause 7 — MessageGroupId forwarded to the queue:
  expected {'...-single-grouped': 'tenant-a', '...-single-plain': None, '...-batch-grouped': 'tenant-b', '...-batch-plain': None}
  actual   {'...-single-grouped': None, '...-single-plain': None, '...-batch-grouped': None, '...-batch-plain': None}

Cause 8 — 'Check timed out after 60.0s' on all three filter-policy delivery checks; no message ever arrives at the subscribed queue, while unfiltered SNS->SQS delivery is observed to work elsewhere in the same run.

Cause 9 — 'Check timed out after 60.0s' on all three FIFO delivery/ordering checks; no message ever arrives at the FIFO subscriber.

## Steps to Reproduce
1. git_clone the repo and run the behavioral suite (run_behavioral_suite) against the built emulator.
2. Re-replay exactly the check ids in trace_ids. Per cause:
   - Cause 1: create a topic, subscribe a real SQS queue (non-raw), Publish, receive the message and parse the JSON body — inspect the envelope keys for SignatureVersion/Signature/SigningCertURL; then SetTopicAttributes(SignatureVersion="2") and publish again.
   - Cause 2: CreateSMSSandboxPhoneNumber with a non-E.164 number, and again with '+0123456789'; observe HTTP 200 instead of a 400 InvalidParameter/UserError.
   - Cause 3: call VerifySMSSandboxPhoneNumber with a wrong OTP; burst 150 concurrent GetSMSSandboxAccountStatus calls; call GetSMSSandboxAccountStatus under an IAM policy with an explicit Deny; GetDataProtectionPolicy on a well-formed topic ARN with no policy; PutDataProtectionPolicy with any payload; ListSubscriptions with NextToken='c2ltMnJlYWwtbm90LWEtcmVhbC10b2tlbg=='. Each answers 'NotImplemented'.
   - Cause 4: PublishBatch with two entries whose bodies total 262145 bytes; read Error.Code and Error.Message.
   - Cause 5: PublishBatch with one entry carrying a malformed Number MessageAttribute; observe 200.
   - Cause 6: Publish Number attributes '+5', '.5', '1.', '1.500' to a topic with a raw-delivery SQS subscriber; read the SQS message attributes.
   - Cause 7: Publish (and PublishBatch) with MessageGroupId to a STANDARD topic with a subscribed standard queue; ReceiveMessage with AttributeNames including MessageGroupId.
   - Cause 8: subscribe with FilterPolicy {"k":[{"exists":false}]}, with an anything-but policy, and publish a String.Array attribute; then clear the policy with an empty value and publish again — poll the queue.
   - Cause 9: publish to a FIFO topic from concurrent publishers across several message groups, and one PublishBatch carrying two groups — poll the FIFO subscriber.
