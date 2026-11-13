Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.concurrent-template-create-exactly-one-body-renders sesv2.email-template-contract-limits-rejected
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: sesv2.concurrent-template-create-exactly-one-body-renders|run#15|AssertionError|assert code in refusals and message == refusals[code], (
Issue-Key: sesv2.email-template-contract-limits-rejected|run#12|AssertionError|assert msg_data == expected_data_msg, f"expected message {expected_data_msg!r}, got {msg_data!r}"
Issue-Commit: de2a758d888a26724f6774bb8d661e6e5775621b

# ses (sesv2 template plane): 3 net-new refusal-message divergences — the tagged CreateEmailTemplate duplicate refusal names the template and account, the templateData length violation echoes the whole 262145-byte value, and the 500 KB template-size refusal is reworded

> _Note: this issue was identified against the code base at commit `de2a758d888a26724f6774bb8d661e6e5775621b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEmailTemplate, TestRenderEmailTemplate

## Description
Three independent net-new root causes in the SESv2 email-template surface (crate services/service-com-amazonaws-sesv2-simpleemailservice-v2), found by replaying the behavioral suite. All three belong to the `ses` service token the grounding checks carry. 3 causes in this issue; ALL THREE must be closed before submitting.

=== Cause 1 of 3: the duplicate-name refusal of a TAGGED CreateEmailTemplate carries the name-and-account message AWS reserves for the untagged path ===

AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json) binds CreateEmailTemplate to POST /v2/email/templates with input {TemplateName (required, EmailTemplateName len 1..), TemplateContent (required), Tags} and models AlreadyExistsException (errorKind client, HTTP 400, doc "The resource specified in your request already exists.") and BadRequestException (client, HTTP 400, "The input you provided is invalid."). The model does not state the message text, so for the text this is recorded behavior only. The recording shows TWO distinct refusal texts for a duplicate name, selected by the code path, not by timing alone:
  * a create that carries Tags and loses the name to a concurrent create is refused HTTP 400 with Code `AlreadyExistsException` and the message exactly `Template already exists` — no template name and no account id — or, when it races the winner's commit, HTTP 400 `BadRequestException` with `Template is already created with <TemplateName>/EMAIL` (fixed text `Template is already created with ` + `/EMAIL`; the variable part is the TemplateName from the request);
  * a create issued WITHOUT Tags after the name is settled is refused HTTP 400 `AlreadyExistsException` with `Template <TemplateName> already exists for account id <accountId>` (fixed text `Template `, ` already exists for account id `; variable parts: the TemplateName from the request and the caller's 12-digit account id).
So the untagged message is the one that names the resource and the account; the tagged path's message is the bare `Template already exists`.

Emulator: every duplicate-name refusal uses the single name-and-account template. services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_email_template.rs lines 199-206 returns AlreadyExistsException with `crate::state::email_template_already_exists(&name)` regardless of whether the request carried Tags, so a tagged racing loser is refused with `Template <name> already exists for account id <account>` where AWS emits `Template already exists`. Status (400) and code (AlreadyExistsException) are correct, as are the race semantics the rest of the check grades (exactly one winner out of five, four refusals, all HTTP 400) — only the message of the tagged path is wrong. The untagged serial duplicate later in the same check expects, and would get, the name-and-account form.

Why: a client that distinguishes a lost race on a tagged create from a settled-duplicate rewrite (the standard idempotent-provisioning pattern: treat the bare `Template already exists` as benign, surface the named form) branches the wrong way, and the emulator additionally discloses the account id in a refusal where AWS discloses nothing about the resource.

Fix: the CreateEmailTemplate handler's duplicate-name branch (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_email_template.rs) together with the message helper it calls (`crate::state::email_template_already_exists` in src/state.rs) — select the refusal text by whether the request carried Tags, keeping the name-and-account form only for the untagged path.

Traces: ses.concurrent-template-create-exactly-one-body-renders.

=== Cause 2 of 3: the EmailTemplateData length-constraint violation message quotes the entire submitted value ===

AWS: Smithy: TestRenderEmailTemplate (POST /v2/email/templates/{TemplateName}/render) input TemplateData is required and targets com.amazonaws.sesv2#EmailTemplateData, a string with @length {min 0, max 262144}; the operation models BadRequestException (client, HTTP 400). A TemplateData of 262145 bytes is refused HTTP 400 with Code `BadRequestException` and the standard AWS constraint-violation envelope, which for this member does NOT echo the offending value:
  `<N> validation error detected: Value at '<member path>' failed to satisfy constraint: Member must have length less than or equal to <max>`
Fixed text: `<N> validation error detected: Value at '`, `' failed to satisfy constraint: Member must have length less than or equal to `. Variable parts: the violation count, the wire member path (`templateData`) and the modeled maximum (262144) — all taken from the request and the model. The absence of any quoted value is part of the fixed text.

Emulator: the same request is refused with the right status and code but with the value spliced in: `1 validation error detected: Value '{"k": "xxxx…[262145 bytes]"}' at 'templateData' failed to satisfy constraint: Member must have length less than or equal to 262144`. The renderer emits `Value '<submitted value>' at` where AWS emits `Value at`, so the refusal contains a quarter-megabyte copy of the caller's own input.

Why: a client asserting on the refusal text never matches, and the error payload balloons to the size of the rejected input — exactly the input AWS refused for being too large — so logs and error-handling paths carry the caller's rendering data (which routinely holds personalization data) back out. This is the same renderer defect already filed for the SES v1 crate's enum constraint in open issue 20261027T100221Z-ses-2-net-new-behavioral-root-causes-the.md (Cause 1, PutConfigurationSetDeliveryOptions); that issue covers neither this crate nor the length-constraint class, so it does not cover this cause — close them consistently.

Fix: the shared pre-handler constraint-violation renderer used by the sesv2 crate's deserialize→validate stage (the `Violation`/`ViolationKind` rendering of smithy_server_runtime, as used from services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/state.rs line 18) — emit the envelope without the submitted value for length violations.

Traces: ses.email-template-contract-limits-rejected (the assertion that failed this replay).

=== Cause 3 of 3: the 500 KB template-size refusal is reworded and does not name the breached quota ===

AWS: recorded behavior only (the model bounds none of EmailTemplateContent's parts; the 500 KB whole-template limit is an unmodeled service quota, and CreateEmailTemplate models BadRequestException, HTTP 400, for it). A CreateEmailTemplate whose content exceeds the quota is refused HTTP 400 `BadRequestException` with `Template <TemplateName> exceeds maximum allowed size of 500 KB.` — fixed text `Template `, ` exceeds maximum allowed size of 500 KB.`; the variable part is the TemplateName from the request. The message names the resource and states the quota in KB, which is what lets a caller tell this limit apart from the TemplateData breach of Cause 2; and the refused create must leave nothing behind, so a following GetEmailTemplate answers HTTP 404 `NotFoundException` with `Template <TemplateName> does not exist.`

Emulator: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_email_template.rs lines 166-176 refuses with BadRequestException carrying `The template is <size> bytes, which is larger than the maximum template size of <MAX_EMAIL_TEMPLATE_BYTES> bytes.` — a message AWS never emits: it omits the template name, states bytes instead of the 500 KB quota, and reports the measured size (a value, not part of any AWS template). The code is right, the text is not. This cause sits in the same failing check, downstream of the assertion Cause 2 stops on, so the replay has not yet compared it; the divergence is read off the handler source against the behavior the check records for AWS, and it will fail the check as soon as Cause 2 is fixed.

Why: the two template limits collapse into mutually unrecognizable messages, so a client cannot tell an over-size template from over-length rendering data, and nothing in the refusal names the template it rejected.

Fix: the size-guard branch of the CreateEmailTemplate handler (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_email_template.rs) and its size/limit helpers in src/state.rs — render the quota message AWS renders, naming the TemplateName and the 500 KB quota, and keep the no-side-effect guarantee.

Traces: ses.email-template-contract-limits-rejected.

## Evidence
Suite: 40/139 passed. These two are the only surviving ses failures that were marked neither ALREADY FILED (scaffold stub) nor OWNED by an open issue.

Cause 1 — check ses.concurrent-template-create-exactly-one-body-renders (verdict: mismatch):
  AssertionError: loser 0 must be refused because THIS template already exists -- a throttle or a quota refusal would prove nothing about atomicity; got {'Message': 'Template sim2real-behav-tplrace-df27c52121de already exists for account id 000000000000', 'Code': 'AlreadyExistsException'}
  Expected (AWS, for a create carrying Tags that loses the race): HTTP 400 with either ('AlreadyExistsException', 'Template already exists') or ('BadRequestException', 'Template is already created with <TemplateName>/EMAIL').
  Actual (emulator): HTTP 400 ('AlreadyExistsException', 'Template <TemplateName> already exists for account id <accountId>') — the untagged-path message. The name `sim2real-behav-tplrace-df27c52121de` and the account `000000000000` in the quote are this run's values, not part of the template.
  Passing parts of the same check: exactly 1 winner of 5, 4 refusals, every refusal HTTP 400.
  Source: src/ops/create_email_template.rs:199-206 (AlreadyExistsException { message: crate::state::email_template_already_exists(&name) }), taken for every duplicate regardless of Tags.

Cause 2 — check ses.email-template-contract-limits-rejected (verdict: mismatch):
  AssertionError: expected message "1 validation error detected: Value at 'templateData' failed to satisfy constraint: Member must have length less than or equal to 262144", got '1 validation error detected: Value \'{"k": "xxxxxxxx…[262145 bytes]\' at \'templateData\' failed to satisfy constraint: Member must have length less than or equal to 262144'
  Expected (AWS): HTTP 400, BadRequestException, "1 validation error detected: Value at 'templateData' failed to satisfy constraint: Member must have length less than or equal to 262144".
  Actual (emulator): HTTP 400, BadRequestException, same sentence with the whole 262145-byte submitted value quoted after `Value `.
  Status and code match; only the message diverges. Smithy: EmailTemplateData @length max 262144; TestRenderEmailTemplate errors BadRequestException(400)/NotFoundException(404)/TooManyRequestsException(429).

Cause 3 — same check, assertion downstream of Cause 2 (not reached this replay):
  Expected (AWS): CreateEmailTemplate with a 600000-byte Text -> HTTP 400 BadRequestException "Template <TemplateName> exceeds maximum allowed size of 500 KB.", then GetEmailTemplate for that name -> NotFoundException "Template <TemplateName> does not exist."
  Actual (emulator source, src/ops/create_email_template.rs:166-176): BadRequestException "The template is <size> bytes, which is larger than the maximum template size of <MAX_EMAIL_TEMPLATE_BYTES> bytes."

## Steps to Reproduce
Replay the behavioral suite and re-run exactly these checks.

Cause 1 (ses.concurrent-template-create-exactly-one-body-renders):
1. Build 5 sesv2 clients up front; pick one fresh TemplateName.
2. From 5 threads simultaneously call sesv2 create_email_template(TemplateName=<name>, TemplateContent={'Subject': 'subj-<i>', 'Text': 'body-<i>', 'Html': '<p>body-<i></p>'}, Tags=[{'Key':'sim2real','Value':<check>}]).
3. Exactly one call succeeds (correct today); inspect the 4 refusals: each must be HTTP 400 with either ('AlreadyExistsException', 'Template already exists') or ('BadRequestException', 'Template is already created with <name>/EMAIL'). Observe instead ('AlreadyExistsException', 'Template <name> already exists for account id <account>').
4. Then call create_email_template for the same name WITHOUT Tags: this one must be ('AlreadyExistsException', 'Template <name> already exists for account id <account>') — the form the emulator wrongly used in step 3.

Cause 2 (ses.email-template-contract-limits-rejected):
1. create_email_template(TemplateName=<fresh>, TemplateContent={'Subject':'limit probe','Text':'value={{k}}','Html':'<p>value={{k}}</p>'}).
2. test_render_email_template(TemplateName=<that name>, TemplateData=json.dumps({'k': 'x'*(262145-len(json.dumps({'k':''})))})) — exactly 262145 bytes.
3. Observe HTTP 400 BadRequestException whose message quotes the entire submitted value; AWS emits "1 validation error detected: Value at 'templateData' failed to satisfy constraint: Member must have length less than or equal to 262144".
4. Control: TemplateData of exactly 262144 bytes must be accepted and substituted whole into both the text/plain and text/html alternatives.

Cause 3 (same check, after Cause 2 is fixed):
1. create_email_template(TemplateName=<fresh>, TemplateContent={'Subject':'size probe','Text':'b'*600000}).
2. Observe the BadRequestException message: AWS says "Template <name> exceeds maximum allowed size of 500 KB."; the emulator says "The template is <size> bytes, which is larger than the maximum template size of <N> bytes."
3. get_email_template(TemplateName=<that name>) must fail NotFoundException "Template <name> does not exist." (nothing created), and a 400000-byte Text must be accepted and round-trip byte-for-byte.
