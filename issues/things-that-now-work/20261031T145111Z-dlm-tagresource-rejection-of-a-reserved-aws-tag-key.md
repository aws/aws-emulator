Issue-Source: trace-sweep-divergence
Issue-Trace: dlm-tag-resource-reserved-aws-prefix-key-rejected
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dlm-tag-resource-reserved-aws-prefix-key-rejected
Issue-Shape-Id: com.amazonaws.dlm#dlm_20180112
Issue-Key: dlm|TagResource|4xx->4xx|$.message
Issue-Key: dlm|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 920b0b9a9be741523e1b0de158cd9409636a148e

# TagResource rejection of a reserved aws: tag key omits the error message and reports the wrong error type

> _Note: this issue was identified against the code base at commit `920b0b9a9be741523e1b0de158cd9409636a148e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
One net-new wire cause in the dlm crate's TagResource tag-key validation path.

Cause 1 of 1: a TagResource call whose Tags map carries a key with the reserved `aws:` prefix is refused with HTTP 400 (correct), but the refusal body carries no error message at all and the refusal is classified as the wrong error shape.

AWS: Smithy (services/service-com-amazonaws-dlm-dlm-20180112): TagResource is POST /tags/{ResourceArn}, ResourceArn -> PolicyArn (@httpLabel, required, @pattern ^arn:aws(-[a-z]{1,3}){0,2}:dlm:[A-Za-z0-9_/.-]{0,63}:\d+:policy/[0-9A-Za-z_-]{1,128}$), Tags -> TagMap (@length min 1 max 200) whose key TagKey is @length min 1 max 128 with @pattern ^(?!aws:)[a-zA-Z+-=._:/]+$ — the reserved `aws:` prefix is a shape-level constraint on the tag key. Modeled errors are InternalServerException(500), InvalidRequestException(400) and ResourceNotFoundException(404); the error shape DLM actually returns for a reserved-prefix tag key is NOT in that list, so: recorded behavior only — the recording refuses the call with HTTP 400 and the error message 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)', carried in the error body under the `message` member, and with an x-amzn-errortype naming a shape other than InvalidRequestException. That message is entirely FIXED text: no part of it is taken from the request or from state — the offending key is not interpolated, no id, name, limit or timestamp appears in it, so it must be emitted verbatim for every reserved-prefix key. The separately recorded behavioral run of the same scenario (check id dlm.tag-resource-reserved-aws-prefix-key-rejected, see open issue 20261029T183853Z-dlm-arn-keyed-tagging-operations-return-the-wrong.md) shows the recorded error code for this class is 'BadRequestException', i.e. a constraint-pattern violation is classified separately from the InvalidRequestException DLM uses for semantic/business-rule refusals.

Emulator: answers HTTP 400 (matching the recorded status) but the response body has NO message at `$.message` (replay detail: "$.message: missing (expected 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)', got None)"), and the response header x-amzn-errortype is 'InvalidRequestException', which does not match the recorded error shape (replay detail: "$headers.x-amzn-errortype: shape_mismatch"). So the refusal is message-less and mis-classified while the status is right.

Why: a client relying on AWS's behavior gets two things wrong. First, the SDK surfaces an empty/absent error message, so the caller cannot tell WHY the tag call was refused — the reserved-prefix rule is invisible, and anything that logs or displays err['Message'] shows nothing. Second, error handling keyed on the error type/code DLM actually emits for malformed input (BadRequestException — fix the request, never retry) does not match the emulator's InvalidRequestException, which is DLM's domain-rule class; input-validation failures and policy-semantics failures therefore become indistinguishable, so a never-retryable bad request can be routed into a retry/business-rule branch.

Fix: the TagResource handler's tag-key validation in the dlm service crate (the TagKey length/pattern and reserved `aws:`-prefix check) plus that path's error serializer: it must populate the error body's `message` member with AWS's fixed text and set the error shape/x-amzn-errortype to the recorded refusal class (BadRequestException) rather than the generic InvalidRequestException used for domain refusals. The same classification and message applies wherever tag keys are validated on this ARN-keyed tagging path.

Traces: dlm-tag-resource-reserved-aws-prefix-key-rejected#task#3 — the fixture's other steps (creating the policy and tagging it with a normal key) pass, so only this refusal's body and error type diverge. Not covered by the open behavioral issue 20261029T183853Z-dlm-arn-keyed-tagging-operations-return-the-wrong.md, which carries no replay key for this wire step and does not state the missing-`message` defect; also unrelated to 20261029T180848Z-dlm-untagresource-wrong-behavior.md (UntagResource DateModified).

## Evidence
Fixture dlm-tag-resource-reserved-aws-prefix-key-rejected, step task#3 (service=dlm, operation=TagResource, verdict=mismatch):
  expected HTTP status: 400
  actual HTTP status:   400  (status matches)
  $.message            expected 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)'  ->  actual: missing (None)
  $headers.x-amzn-errortype  expected the recorded refusal shape (asserted by shape, not literal)  ->  actual 'InvalidRequestException'  (shape_mismatch)

divergence_key: service=dlm|TagResource|status=400->400|keys=$.message,$headers.x-amzn-errortype

Corroborating recorded run of the same scenario (behavioral check dlm.tag-resource-reserved-aws-prefix-key-rejected): expected Code='BadRequestException', Message='Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)'; actual Code='InvalidRequestException'.

Fixed vs variable parts of the required message: the whole string 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)' is fixed text — it interpolates no value from the request (not the tag key, not the policy ARN) and no value from emulator state.

## Steps to Reproduce
1. Create an IAM role DLM can assume and a custom EBS_SNAPSHOT_MANAGEMENT lifecycle policy with one schedule; read its ARN from GetLifecyclePolicy(PolicyId=...).Policy.PolicyArn.
2. Call TagResource(ResourceArn=<that ARN>, Tags={'aws:reserved': 'nope'}) — i.e. POST /tags/<url-encoded ARN> with body {"Tags":{"aws:reserved":"nope"}}.
3. Observe HTTP 400 with an error body that has no `message` member and header x-amzn-errortype: InvalidRequestException, where AWS returns HTTP 400 with x-amzn-errortype naming BadRequestException and body message 'Invalid tag key. Customers cannot set tag keys starting with aws: (case insensitive)'.
4. Replay: replay_wire_corpus and re-check fixture dlm-tag-resource-reserved-aws-prefix-key-rejected step task#3.
