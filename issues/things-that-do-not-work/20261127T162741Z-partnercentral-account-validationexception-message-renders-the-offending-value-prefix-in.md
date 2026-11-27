Issue-Source: trace-sweep-divergence
Issue-Trace: partnercentral-account-profile-update-task-input-validation
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: partnercentral-account-profile-update-task-input-validation partnercentral-account-profile-update-task-start-validation partnercentral-account-training-certification-email-domain-input-validation
Issue-Shape-Id: com.amazonaws.partnercentralaccount#PartnerCentralAccount
Issue-Key: partnercentral-account|StartProfileUpdateTask|4xx->4xx|$.Message
Issue-Key: partnercentral-account|AssociateAwsTrainingCertificationEmailDomain|4xx->4xx|$.Message
Issue-Commit: 7f3d8acfd1d18a62a70e68f35eeb77bb7bd3316f

# ValidationException message renders the offending-value prefix in the wrong form

> _Note: this issue was identified against the code base at commit `7f3d8acfd1d18a62a70e68f35eeb77bb7bd3316f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartProfileUpdateTask

## Description
The partnercentral-account input validator produces the right HTTP status (400) and the right error code (ValidationException), and the right constraint clause, but it builds the leading "Value ... at '<member path>'" segment of the message with the wrong form. AWS has two distinct forms for that segment and the emulator uses each one where AWS uses the other. Two causes, both in the same message renderer.

The AWS message template for a single constraint failure is:
  "1 validation error detected: Value <rendered-value> at '<memberPath>' failed to satisfy constraint: <constraint clause>"
Fixed text: "1 validation error detected: Value", " at '", "' failed to satisfy constraint: ", and the constraint clause itself ("Member must have length greater than or equal to 1", "Member must not be null"). Variable parts: the rendered value, taken from the request, and the member path, taken from the input shape's member names as they appear on the wire (lowerCamel, dotted through nested structures, e.g. taskDetails.industrySegments). The count prefix ("1 validation error detected") is the number of failures found in that request. Nothing in a recording is itself part of the template.

Cause 1 of 2: a present value that violates a constraint must be rendered into the message, and is not.
  AWS: StartProfileUpdateTask.TaskDetails is @required, and TaskDetails$IndustrySegments is @required targeting IndustrySegmentList with @length min 1, max 3 (Smithy: shape TaskDetails, member IndustrySegments; operation StartProfileUpdateTask error ValidationException httpError 400). When the member IS present but fails its length constraint, AWS renders the offending value after "Value", in single quotes - for an empty list that is the literal rendering '[]' - giving "1 validation error detected: Value '[]' at 'taskDetails.industrySegments' failed to satisfy constraint: Member must have length greater than or equal to 1".
  Emulator: returns 400 ValidationException with the value segment omitted entirely: "1 validation error detected: Value at 'taskDetails.industrySegments' failed to satisfy constraint: Member must have length greater than or equal to 1".
  Why: a client that parses the message to recover which submitted value was rejected (log scrapers, retry wrappers, and the AWS CLI's own error surfacing all do) sees no value at all, so an operator cannot tell an empty list from a 4-element list from the message, and a test asserting on the AWS message text fails against the emulator.
  Fix: the StartProfileUpdateTask input validation / the shared ValidationException message builder in the partnercentral-account crate - when a member is present and violates a length/range/pattern constraint, render its value into the "Value <v> at ..." slot, single-quoted, using the wire rendering of the value (an empty list as '[]').
  Traces: partnercentral-account-profile-update-task-input-validation#task#2 (also #task#8), partnercentral-account-profile-update-task-start-validation#task#2.

Cause 2 of 2: a missing/null required member must NOT have a value rendered, and a literal "null" is rendered.
  AWS: AssociateAwsTrainingCertificationEmailDomain.EmailVerificationCode is @required, targeting EmailVerificationCode (@length min 6 max 6, @pattern ^[0-9]+$, @sensitive) (Smithy: operation AssociateAwsTrainingCertificationEmailDomain, member EmailVerificationCode; error ValidationException httpError 400). For the missing-required-member class AWS uses the no-value form of the template - there is no submitted value to quote - so the message is "1 validation error detected: Value at 'emailVerificationCode' failed to satisfy constraint: Member must not be null". This is also the only correct form here because the member is @sensitive and its value must never be echoed.
  Emulator: returns 400 ValidationException with a literal value injected: "1 validation error detected: Value null at 'emailVerificationCode' failed to satisfy constraint: Member must not be null".
  Why: the emulator's text does not match AWS's for the single most common validation class, so client code and tests that match the missing-member message do not match; and the renderer's habit of printing the in-memory value for an absent member is exactly the path that would echo a @sensitive value when one is present but invalid.
  Fix: the same ValidationException message builder / the AssociateAwsTrainingCertificationEmailDomain input validation - for the "Member must not be null" class emit the no-value form ("Value at '<path>' ..."), never the string "null", and never render the value of a @sensitive member.
  Traces: partnercentral-account-training-certification-email-domain-email-domain-input-validation is not the id - the trace is partnercentral-account-training-certification-email-domain-input-validation#task#4.

Note: the two causes are opposite halves of one renderer bug (value dropped when it should be present, value invented when it should be absent), so they are expected to be fixed together. No open partnercentral-account issue covers them; the only open issues for this service are the GetConnectionPreferences and ListPartners scaffold-stub issues, which concern unimplemented operations, not validation message text.

## Evidence
Cause 1 - fixture partnercentral-account-profile-update-task-input-validation, step task#2, operation StartProfileUpdateTask. Status matches (400 expected, 400 actual); body member $.Message diverges.
  Expected: "1 validation error detected: Value '[]' at 'taskDetails.industrySegments' failed to satisfy constraint: Member must have length greater than or equal to 1"
  Actual:   "1 validation error detected: Value at 'taskDetails.industrySegments' failed to satisfy constraint: Member must have length greater than or equal to 1"
  Same cluster (3 further steps): partnercentral-account-profile-update-task-input-validation#task#2, partnercentral-account-profile-update-task-input-validation#task#8, partnercentral-account-profile-update-task-start-validation#task#2.

Cause 2 - fixture partnercentral-account-training-certification-email-domain-input-validation, step task#4, operation AssociateAwsTrainingCertificationEmailDomain. Status matches (400 expected, 400 actual); body member $.Message diverges.
  Expected: "1 validation error detected: Value at 'emailVerificationCode' failed to satisfy constraint: Member must not be null"
  Actual:   "1 validation error detected: Value null at 'emailVerificationCode' failed to satisfy constraint: Member must not be null"

Model evidence (Smithy, services/service-com-amazonaws-partnercentralaccount-partnercentralaccount/smithy-model/com-amazonaws-partnercentralaccount-partnercentralaccount.json): TaskDetails$IndustrySegments -> IndustrySegmentList, @required, @length{min:1,max:3}; AssociateAwsTrainingCertificationEmailDomainRequest$EmailVerificationCode -> EmailVerificationCode, @required, @length{min:6,max:6}, @pattern ^[0-9]+$, @sensitive; both operations declare ValidationException with httpError 400.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture partnercentral-account-profile-update-task-input-validation step task#2 - a StartProfileUpdateTask request whose TaskDetails.IndustrySegments is present but an empty list. Observe 400 ValidationException and compare $.Message: the emulator omits the "'[]'" value between "Value" and "at". Steps task#8 of the same fixture and task#2 of partnercentral-account-profile-update-task-start-validation reproduce the identical omission.
3. Cause 2: replay fixture partnercentral-account-training-certification-email-domain-input-validation step task#4 - an AssociateAwsTrainingCertificationEmailDomain request that omits EmailVerificationCode. Observe 400 ValidationException and compare $.Message: the emulator inserts a literal "null" between "Value" and "at", where AWS emits no value segment at all.
4. Both are message-body-only divergences: HTTP status and error code already match, so assert on $.Message.
