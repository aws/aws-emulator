Issue-Source: trace-sweep-divergence
Issue-Trace: sns-sms-account-configuration-and-opt-out-check
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sns-sms-account-configuration-and-opt-out-check sns-sms-sandbox-number-verification-flow sns-subscription-filter-policy-and-scope-roundtrip sns-subscription-pending-and-error-paths
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns|CheckIfPhoneNumberIsOptedOut|2xx->4xx|$.CheckIfPhoneNumberIsOptedOutResponse
Issue-Key: sns|CheckIfPhoneNumberIsOptedOut|2xx->4xx|status
Issue-Key: sns|CreateSMSSandboxPhoneNumber|4xx->2xx|$.ErrorResponse
Issue-Key: sns|CreateSMSSandboxPhoneNumber|4xx->2xx|status
Issue-Key: sns|SetSubscriptionAttributes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: sns|GetSubscriptionAttributes|2xx->2xx|$.GetSubscriptionAttributesResponse.GetSubscriptionAttributesResult.Attributes.entry[*].value
Issue-Key: sns|ConfirmSubscription|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 4fd31dd1d271cb10bcbd1385ecc8231faccc1a9e

# sns: 5 wire divergences — the opt-out check and the sandbox registration answer the wrong status, and three subscription responses carry the wrong text

> _Note: this issue was identified against the code base at commit `4fd31dd1d271cb10bcbd1385ecc8231faccc1a9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CheckIfPhoneNumberIsOptedOut

## Description
Five net-new root causes found by replaying the wire fixture corpus (simparity-wire-aws-v1) against the SNS emulator: 150/156 steps pass, and the six that do not cluster into the five independent causes below. Each cause is numbered and carries its own fixture and step. ALL FIVE must be closed before this issue is submitted.

=== Cause 1 of 5: CheckIfPhoneNumberIsOptedOut refuses a phone number AWS accepts — 400 instead of 200 isOptedOut=false ===
Fixture/step: sns-sms-account-configuration-and-opt-out-check#task#7.
AWS: Smithy (com.amazonaws.sns#CheckIfPhoneNumberIsOptedOut): the input member is `phoneNumber` targeting `com.amazonaws.sns#PhoneNumber`, which is an UNCONSTRAINED @sensitive string — no @pattern and no @length. (Contrast `PhoneNumberString`, the member of the sandbox/publish operations, which the model DOES constrain: @length 0..20 and @pattern ^(\+[0-9]{8,}|[0-9]{0,9})$.) The operation's output member `isOptedOut` is a boolean defaulting to false, documented as 'false – The phone number is opted in'. Recorded behavior: this step's number is answered 200 with CheckIfPhoneNumberIsOptedOutResult.isOptedOut = 'false' — AWS reports a number it holds no opt-out record for as opted in rather than refusing the call. The modelled client refusal is InvalidParameterException (awsQueryError InvalidParameter, HTTP 400), which AWS does not take here.
Emulator: the call is refused with HTTP 400 and no CheckIfPhoneNumberIsOptedOutResponse body at all. The handler validates the number BEFORE the store is consulted (services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/check_if_phone_number_is_opted_out.rs, `state::phone_number_refusal`), and that rule is `state::is_e164` — '+' followed by 8..=15 digits, no leading zero — which the emulator's own comment admits it lifted from the CONSTRAINED `PhoneNumberString` member of a different operation. Applying that stricter rule to this operation's unconstrained member turns an accepted read into an InvalidParameter refusal.
Why: an opt-out check is a pre-flight a sender runs before every SMS send. A client that asks AWS 'may I message this number?' gets false (send it); against the emulator it gets a 400 it has no reason to expect and has to treat as a fatal error, so the whole SMS path is unusable for every number the emulator's narrower E.164 rule rejects — including numbers AWS answers for.
Fix: the CheckIfPhoneNumberIsOptedOut handler's argument validation: the opt-out read must not hold `phoneNumber` to the constraint the model places on a different, constrained member; a number with no opt-out record answers 200 with isOptedOut=false.
Traces: sns-sms-account-configuration-and-opt-out-check#task#7.

=== Cause 2 of 5: CreateSMSSandboxPhoneNumber accepts a registration AWS refuses — 200 instead of 400 UserError 'No origination entities available to send' ===
Fixture/step: sns-sms-sandbox-number-verification-flow#task#2.
AWS: Smithy (com.amazonaws.sns#CreateSMSSandboxPhoneNumber) declares UserErrorException (awsQueryError UserError, httpResponseCode 400) beside InvalidParameter/OptedOut/Throttled. Recorded behavior: registering a destination number for sandbox verification in an account that holds NO origination entity able to send the verification SMS is refused 400 with ErrorResponse.Error.Type = 'Sender', Error.Code = 'UserError', Error.Message = 'No origination entities available to send'. That message is WHOLLY FIXED text — it carries no part taken from the request or from state (the destination number is @sensitive and is not echoed).
Emulator: the registration is accepted with HTTP 200 and an empty CreateSMSSandboxPhoneNumberResult, so no ErrorResponse is produced at all. The account's origination inventory exists in state (`SnsStore::origination_numbers`, documented there as populated out of band and empty unless seeded) but the handler never consults it: nothing stands between the request and a stored pending sandbox number.
Why: the sandbox verification flow is accepted and then cannot complete — the verification message AWS refuses to even attempt is reported as sent, so a client waiting for the OTP waits forever with a 200 in hand, and a test asserting that an account with no origination number cannot register a sandbox destination passes against AWS and fails here.
Fix: the CreateSMSSandboxPhoneNumber handler's pre-flight: before a pending sandbox number is stored, require the account to hold an origination entity able to send the verification SMS, and refuse with UserErrorException (400 / UserError) carrying exactly 'No origination entities available to send'.
Traces: sns-sms-sandbox-number-verification-flow#task#2.

=== Cause 3 of 5: the SetSubscriptionAttributes FilterPolicy parse-failure message carries an extra 'Attributes Reason: ' clause that AWS does not ===
Fixture/step: sns-subscription-filter-policy-and-scope-roundtrip#task#8.
AWS: Smithy: SetSubscriptionAttributes declares InvalidParameterException (awsQueryError InvalidParameter, HTTP 400); AttributeName documents FilterPolicy as a settable subscription attribute whose value is a JSON policy document. Recorded behavior: a FilterPolicy value that is not JSON is refused 400 / InvalidParameter with the message 'Invalid parameter: FilterPolicy: failed to parse JSON. ' followed by the parser's own diagnostic. Fixed text: 'Invalid parameter: ', then the ATTRIBUTE NAME as the caller spelled it, then ': failed to parse JSON. '; the variable part is the parser diagnostic derived from the offending value (in this recording "Unrecognized token 'not': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1]"). Note what the fixed part does NOT contain: there is no '<member> Reason: ' frame — SNS names the attribute directly for this condition.
Emulator: status 400 and code InvalidParameter are right, but the message is prefixed with the member frame — 'Invalid parameter: Attributes Reason: FilterPolicy: failed to parse JSON. ...' — so every attribute-level parse refusal carries an 'Attributes Reason: ' clause AWS never emits.
Why: the message is the only machine-readable discriminator between the attribute-value failures SNS groups under one error code, so a client, matcher or log pipeline keyed on AWS's text mis-classifies a legitimate validation refusal; and an operator reading it is told the fault is in the 'Attributes' parameter rather than in the named FilterPolicy document.
Fix: the SetSubscriptionAttributes handler's attribute validation, where the InvalidParameter message for a value that fails to parse is composed: emit 'Invalid parameter: <AttributeName>: failed to parse JSON. <parser diagnostic>' with no '<member> Reason: ' frame.
Related open issue that does NOT cover this and does NOT carry this replay key: 20261026T051123Z-sns-6-net-new-behavioral-divergences-an-empty.md Cause 1 is about an EMPTY FilterPolicy value having to CLEAR the attribute instead of being parsed; this cause is the wording of the parse refusal itself, which stays wrong once that is fixed.
Traces: sns-subscription-filter-policy-and-scope-roundtrip#task#8.

=== Cause 4 of 5: the GetSubscriptionAttributes attribute map of an http/https subscription does not line up with AWS's — the EffectiveDeliveryPolicy entry carries the protocol string instead of the policy document, and the emulator's document has no requestPolicy section ===
Fixture/step: sns-subscription-pending-and-error-paths#task#5.
AWS: Smithy: GetSubscriptionAttributes.Attributes is a SubscriptionAttributesMap whose doc lists 'EffectiveDeliveryPolicy – The JSON serialization of the effective delivery policy that takes into account the topic delivery policy and account system defaults'. Recorded behavior: for an https subscription with no delivery policy of its own, that entry's value is the account system-default document in full — healthyRetryPolicy {minDelayTarget, maxDelayTarget, numRetries, numMaxDelayRetries, numNoDelayRetries, numMinDelayRetries, backoffFunction}, sicklyRetryPolicy: null, throttlePolicy: null, requestPolicy: {headerContentType: 'text/plain; charset=UTF-8'} and guaranteed: false. Fixed parts: the member names and the null/false placeholders and the requestPolicy section; variable parts: only the retry numbers, which come from the topic's and the subscription's own DeliveryPolicy laid over the account defaults.
Emulator: at the position where the recorded map carries that document the emulator's map carries the protocol string 'https' instead, so the attribute entries do not line up with AWS's for this subscription. Two facts behind it, both in the emulator: the Attributes map is assembled into a BTreeMap and reported in that order (get_subscription_attributes.rs), and the document shape it would serialize, `state::SubscriptionDeliveryPolicy`, declares only healthy_retry_policy / sickly_retry_policy / throttle_policy / guaranteed — there is NO requestPolicy member anywhere in the service, so even where the document is reported it cannot equal AWS's.
Why: a client that reads EffectiveDeliveryPolicy to learn the retry/request behavior its endpoint is actually delivered under reads a protocol name, or a document missing the requestPolicy that fixes the Content-Type of the delivered notification; any consumer that walks the attribute entries positionally (the query-protocol entry list) is off by one from AWS, and drift detection against a just-written subscription reports a permanent difference.
Fix: the GetSubscriptionAttributes handler's attribute assembly together with `state::effective_subscription_delivery_policy` / `state::SubscriptionDeliveryPolicy`: report the EffectiveDeliveryPolicy entry for an http/https subscription with AWS's full document, requestPolicy section included, and in AWS's entry layout.
Related open issue that does NOT carry this replay key: 20261026T051123Z-sns-6-net-new-behavioral-divergences-an-empty.md Cause 2 covers the stored (subscription-written) DeliveryPolicy normalization; this cause is the COMPUTED EffectiveDeliveryPolicy entry of a subscription nobody gave a policy, read over the wire.
Traces: sns-subscription-pending-and-error-paths#task#5.

=== Cause 5 of 5: the ConfirmSubscription bad-token refusal message is not AWS's text ===
Fixture/step: sns-subscription-pending-and-error-paths#task#10.
AWS: Smithy: ConfirmSubscription takes @required Token and declares InvalidParameterException (awsQueryError InvalidParameter, HTTP 400) and NotFoundException (404). Recorded behavior: confirming a subscription with a token that is not a confirmation token for the topic is refused 400 / InvalidParameter with the message 'Invalid token' — WHOLLY FIXED text, with no part taken from the request (the token is not echoed) and no '<member> Reason: ' frame.
Emulator: status 400 and code InvalidParameter are right, but the message is 'Invalid parameter: Token' — a different template that reads as a generic member-constraint complaint.
Why: 'Invalid token' is how SNS distinguishes a token that does not confirm anything from a malformed request member, and it is the only part of the response that carries that distinction; clients and tests that branch on AWS's text (and operators reading confirmation-endpoint logs) cannot tell the two apart.
Fix: the ConfirmSubscription handler, where the InvalidParameter refusal for a token that matches no pending confirmation is constructed: emit exactly 'Invalid token'.
Traces: sns-subscription-pending-and-error-paths#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator: 24 fixtures replayed, 156 steps, 150 passed, 6 mismatched, 0 errored, 0 blocked (score 96.2%, emulator_up=true, bootstrap_signal=false). The six mismatches cluster into the five divergences below.

Cause 1 — sns-sms-account-configuration-and-opt-out-check#task#7, CheckIfPhoneNumberIsOptedOut:
  status: expected 200, got 400
  $.CheckIfPhoneNumberIsOptedOutResponse: missing (expected {'CheckIfPhoneNumberIsOptedOutResult': {'isOptedOut': 'false'}, 'ResponseMetadata': {...}}, got None)
  (divergence_key service=sns|CheckIfPhoneNumberIsOptedOut|status=200->400|keys=$.CheckIfPhoneNumberIsOptedOutResponse,status)

Cause 2 — sns-sms-sandbox-number-verification-flow#task#2, CreateSMSSandboxPhoneNumber:
  status: expected 400, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'UserError', 'Message': 'No origination entities available to send'}, 'RequestId': ...}, got None)
  (divergence_key service=sns|CreateSMSSandboxPhoneNumber|status=400->200|keys=$.ErrorResponse,status)

Cause 3 — sns-subscription-filter-policy-and-scope-roundtrip#task#8, SetSubscriptionAttributes (status 400 and Error.Code matched; only Error.Message diverged):
  $.ErrorResponse.Error.Message
  expected: "Invalid parameter: FilterPolicy: failed to parse JSON. Unrecognized token 'not': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1]"
  actual:   "Invalid parameter: Attributes Reason: FilterPolicy: failed to parse JSON. Unrecognized token 'not': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1]"
  (the only difference is the inserted 'Attributes Reason: ' clause)

Cause 4 — sns-subscription-pending-and-error-paths#task#5, GetSubscriptionAttributes (status 200 in both):
  $.GetSubscriptionAttributesResponse.GetSubscriptionAttributesResult.Attributes.entry[5].value
  expected: '{"healthyRetryPolicy":{"minDelayTarget":20,"maxDelayTarget":20,"numRetries":3,"numMaxDelayRetries":0,"numNoDelayRetries":0,"numMinDelayRetries":0,"backoffFunction":"linear"},"sicklyRetryPolicy":null,"throttlePolicy":null,"requestPolicy":{"headerContentType":"text/plain; charset=UTF-8"},"guaranteed":false}'
  actual:   'https'
  (the retry numbers above are this recording's values for a subscription with no policy of its own; the member set, the nulls, the requestPolicy section and guaranteed:false are the fixed shape. Corroborating code fact: state::SubscriptionDeliveryPolicy declares healthy_retry_policy/sickly_retry_policy/throttle_policy/guaranteed and no requestPolicy member.)

Cause 5 — sns-subscription-pending-and-error-paths#task#10, ConfirmSubscription (status 400 and Error.Code matched; only Error.Message diverged):
  $.ErrorResponse.Error.Message
  expected: 'Invalid token'
  actual:   'Invalid parameter: Token'

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from that checkout.
2. replay_wire_corpus() over trace set simparity-wire-aws-v1 and re-replay exactly the fixture#step pairs named in this issue (Issue-Trace/Issue-Step plus Issue-Trace-Ids).
3. Per cause, by hand:
   - Cause 1 (sns-sms-account-configuration-and-opt-out-check#task#7): call CheckIfPhoneNumberIsOptedOut with the fixture's phoneNumber (a number no opt-out was ever recorded for) and read the status — AWS answers 200 with CheckIfPhoneNumberIsOptedOutResult.isOptedOut='false'; the emulator answers 400 with no result body.
   - Cause 2 (sns-sms-sandbox-number-verification-flow#task#2): in an account holding no origination entity, call CreateSMSSandboxPhoneNumber for a destination number and read the status, Error.Type, Error.Code and Error.Message — AWS refuses 400 Sender/UserError/'No origination entities available to send'; the emulator answers 200 with an empty result.
   - Cause 3 (sns-subscription-filter-policy-and-scope-roundtrip#task#8): CreateTopic, Subscribe, then SetSubscriptionAttributes(AttributeName='FilterPolicy', AttributeValue=<a value that is not JSON, e.g. starting with the bare token 'not'>) and read Error.Message — compare against 'Invalid parameter: FilterPolicy: failed to parse JSON. <parser diagnostic>' with no 'Attributes Reason: ' clause.
   - Cause 4 (sns-subscription-pending-and-error-paths#task#5): Subscribe an https endpoint to a topic, then GetSubscriptionAttributes on the subscription ARN and walk the Attributes entry list — the entry AWS fills with the effective delivery-policy document carries 'https' instead, and no entry anywhere carries a requestPolicy section.
   - Cause 5 (sns-subscription-pending-and-error-paths#task#10): ConfirmSubscription(TopicArn=<the topic>, Token=<a token that confirms nothing>) and read Error.Message — AWS states exactly 'Invalid token'.
