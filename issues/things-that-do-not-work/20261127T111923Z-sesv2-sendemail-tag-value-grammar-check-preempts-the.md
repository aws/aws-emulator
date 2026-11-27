Issue-Source: trace-sweep-divergence
Issue-Trace: sesv2-send-email-tag-value-grammar-and-length-bounds
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sesv2-send-email-tag-value-grammar-and-length-bounds
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|SendEmail|4xx->4xx|$headers.x-amzn-errortype
Issue-Related: 20261124T235503Z-sesv2-send-path-s-unverified-identity-refusal-wrong.md
Issue-Commit: fe76b837b9c6a96309bb6d3401999300c9d3921a

# sesv2 SendEmail tag-value grammar check preempts the unverified-identity refusal

> _Note: this issue was identified against the code base at commit `fe76b837b9c6a96309bb6d3401999300c9d3921a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
One net-new cause on service token sesv2, in the outbound single-send path (SendEmail POST /v2/email/outbound-emails): an emulator-authored EmailTags *value* grammar validation fires ahead of the send path's identity-verification gate, so a send AWS refuses as an unverified-identity send is instead refused as a malformed request.

Cause 1 of 1: SendEmail with an EmailTags value outside the documented character set and an unverified sender/recipient is refused with a tag-value syntax error instead of the unverified-identity refusal (fixture sesv2-send-email-tag-value-grammar-and-length-bounds, step task#2, operation SendEmail).

AWS: Smithy (sesv2, services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json): SendEmail is POST /v2/email/outbound-emails, success code 200, input member EmailTags targets MessageTagList of MessageTag{Name: MessageTagName (required), Value: MessageTagValue (required)}; MessageTagValue carries NO length/pattern trait - its criteria are prose only ("It can only contain ASCII letters (a-z, A-Z), numbers (0-9), underscores (_), or dashes (-)" and "It can contain no more than 256 characters"). Modeled refusals are MessageRejected, BadRequestException, AccountSuspendedException, LimitExceededException, MailFromDomainNotVerifiedException and SendingPausedException at httpError 400, NotFoundException at 404 and TooManyRequestsException at 429. Recorded behavior: for a request whose EmailTags value contains characters outside that prose set (the recorded value mixes '.' and '@' with letters, digits, '_' and '-') AND whose From/To identities are not verified, AWS answers HTTP 400 with the unverified-identity refusal "Email address is not verified. The following identities failed the check in region <REGION>: <identity>[, <identity>...]" - the leading sentence and "The following identities failed the check in region " are fixed text, <REGION> is the request's region rendered upper-case, and the identity list is drawn from the request and the account's identity state. The recorded step pins the status and that message and asserts x-amzn-errortype by shape only. So the requirement this step establishes is the PRECEDENCE: the identity-verification refusal is what AWS returns for this input, and no tag-value syntax error is emitted. Hypothesis: AWS evaluates identity verification before any message-tag character check; whether AWS would reject this same tag value for a fully verified sender is not pinned by this recording, so the fix must not simply delete the tag-value check - it must stop it from preempting verification.

Emulator: answers HTTP 400 but with a different error class and an invented message - x-amzn-errortype "BadRequestException" and $.message "Invalid tag value <v2.1@news-letter_a>: only alphanumeric ASCII characters, '_',  '-' are allowed." (note the doubled space before '-' in that emulator-authored text; the bracketed value is the request's tag value). Replay reports "$.message: value_mismatch (expected 'Email address is not verified. The following identities failed the check in region US-WEST-2: sim2real-probe-sesv2-tagv-809c2f5f@example.com', got \"Invalid tag value <v2.1@news-letter_a>: only alphanumeric ASCII characters, '_',  '-' are allowed.\")" and "$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'BadRequestException')". Only the message and the error class diverge; the 400 status agrees.

Why: the client is told its request is syntactically malformed - that it must "fix" a tag value - when the actual reason AWS rejects the send is that an identity is unverified. The error class is wrong too, so code that branches on send rejections (MessageRejected-style, retried after verifying the identity) versus request-shape errors (BadRequestException, never retried) takes the wrong branch, and the real cause - the identity that needs verifying, named in AWS's message - is never surfaced to the caller or to logs.

Fix: the SendEmail handler's validation ordering and its message-tag validation (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/send_email.rs plus the shared send-path validation module that composes the "Invalid tag value ..." text) - the identity-verification gate must run and refuse first, as the recording shows, and the emulator-authored tag-value message must not preempt it.

Traces: sesv2-send-email-tag-value-grammar-and-length-bounds#task#2.

Related open issue that does NOT carry this step's replay key, so it does not close this cause (the replay report lists this cluster as unowned, owned_by empty, stub_path empty): 20261124T235503Z-sesv2-send-path-s-unverified-identity-refusal-wrong.md - its Cause 2 is the tag-NAME preemption on SendBulkEmail (fixture sesv2-send-bulk-email-entry-replacement-validation#task#3) and its Cause 3 is the identity-list membership of the SendEmail message (email-email-template-create-render-send-update-delete#task#6); neither covers the tag-VALUE preemption on SendEmail recorded here, and the other steps of this very fixture (task#1, task#4, task#6) are owned by that issue's Cause 3 cluster, which this step is explicitly not part of.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up true, /__admin/health 200; 396 steps, 342 passed, 54 mismatched, 14 divergence clusters). One unowned sesv2 cluster (stub_path empty, owned_by empty):

fixture sesv2-send-email-tag-value-grammar-and-length-bounds, step task#2, operation SendEmail, verdict mismatch, expected_status 400, actual_status 400, affected_count 1, divergence_key "service=sesv2|SendEmail|status=400->400|keys=$.message,$headers.x-amzn-errortype":
  $.message: value_mismatch (expected 'Email address is not verified. The following identities failed the check in region US-WEST-2: sim2real-probe-sesv2-tagv-809c2f5f@example.com', got "Invalid tag value <v2.1@news-letter_a>: only alphanumeric ASCII characters, '_',  '-' are allowed.")
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'BadRequestException')

Expected (AWS, recorded): HTTP 400, error shape asserted by shape only, message = fixed text "Email address is not verified. The following identities failed the check in region " + <REGION upper-case> + ": " + the identities that failed verification for the request.
Actual (emulator): HTTP 400, x-amzn-errortype BadRequestException, message = "Invalid tag value <" + request tag value + ">: only alphanumeric ASCII characters, '_',  '-' are allowed."

The probe addresses and the tag value are per-run fixture values; the requirements are the status, the error class and the message's fixed text with its identity list filled from the request and the emulator's identity state.

Model evidence (smithy_query on services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json): SendEmail POST /v2/email/outbound-emails (code 200), EmailTags -> MessageTagList -> MessageTag{Name, Value both required}; MessageTagValue is a plain string with prose-only criteria (ASCII letters/digits/underscore/dash, max 256 characters) and no pattern or length trait; errors include MessageRejected and BadRequestException at 400.

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Replay fixture sesv2-send-email-tag-value-grammar-and-length-bounds through step task#2: POST /v2/email/outbound-emails (SendEmail) with FromEmailAddress and Destination.ToAddresses naming addresses that are NOT verified identities, and EmailTags containing one entry whose Value includes characters outside the documented set (e.g. a value mixing '.' and '@' with letters, digits, '_' and '-').
4. Observe HTTP 400 with x-amzn-errortype BadRequestException and message "Invalid tag value <...>: only alphanumeric ASCII characters, '_',  '-' are allowed." instead of the recorded HTTP 400 unverified-identity refusal "Email address is not verified. The following identities failed the check in region US-WEST-2: <the unverified identity>".
5. After the fix, re-replay sesv2-send-email-tag-value-grammar-and-length-bounds#task#2 and confirm the cluster passes (and that the fixture's other steps, owned by 20261124T235503Z-sesv2-send-path-s-unverified-identity-refusal-wrong.md, are unaffected).
