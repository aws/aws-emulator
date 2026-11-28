Issue-Source: trace-sweep-divergence
Issue-Trace: iot-audit-suppression-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-audit-suppression-lifecycle
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|DescribeAuditSuppression|2xx->2xx|$.description
Issue-Key: iot|CreateAuditSuppression|4xx->4xx|$.message
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# iot audit-suppression plane: an update is never reflected and both refusals diverge from AWS

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAuditSuppression

## Description
Three independent net-new root causes in the iot audit-suppression plane. Each cause must be closed before submitting. Related but NOT covering these: open issue 20261126T020902Z-iot-updateauditsuppression-stub.md records that UpdateAuditSuppression is an unimplemented scaffold stub but carries no replay key for the read below and does not state what the read must report; the replay reports all three clusters as unowned.

Cause 1 of 3: an applied UpdateAuditSuppression is not reflected by DescribeAuditSuppression — the stored suppression keeps its original description.
AWS: Smithy UpdateAuditSuppression (PATCH /audit/suppressions/update) requires checkName (AuditCheckName) and resourceIdentifier (ResourceIdentifier) and takes expirationDate, suppressIndefinitely and description (AuditDescription @length 0..1000 @pattern ^[\p{Graph}\x20]*$) as the mutable members; DescribeAuditSuppression (POST /audit/suppressions/describe) returns checkName, resourceIdentifier, expirationDate, suppressIndefinitely and description. Recorded behavior: after an update that supplies a new description for an existing (checkName, resourceIdentifier) pair, the describe reports the NEW description.
Emulator: the describe still reports the description the suppression was created with; the update's value is lost (the suppression record is not rewritten).
Why: a client that edits a suppression's description, expiration or suppressIndefinitely flag and reads it back sees stale data and concludes the edit failed, or worse keeps acting on an expiration window the service appears not to have taken; audit findings stay suppressed under conditions the operator believes they changed.
Fix: UpdateAuditSuppression's write over the suppression record keyed by (checkName, resourceIdentifier), applying only the members the request names, and the state DescribeAuditSuppression reads (services/service-com-amazonaws-iot-awsiotservice/src/ops/update_audit_suppression.rs and the audit-suppression store in crate::state).
Traces: iot-audit-suppression-lifecycle#task#5.

Cause 2 of 3: CreateAuditSuppression's duplicate refusal message is emulator-authored and omits the colliding resource identifier.
AWS: Smithy CreateAuditSuppression (POST /audit/suppressions/create) requires checkName, resourceIdentifier and clientRequestToken, and models ResourceAlreadyExistsException (httpError 409). Recorded behavior: creating a second suppression for the same (checkName, resourceIdentifier) pair is refused 409 with the message "Audit Suppression with checkName=<checkName> and ResourceIdentifier=<resourceIdentifier as JSON> already exists". Fixed text: "Audit Suppression with checkName=", " and ResourceIdentifier=", " already exists"; the variable parts are the request's checkName and its resourceIdentifier rendered as a JSON object (e.g. {"deviceCertificateId":"<id>"}).
Emulator: status 409 is right, but the message is "Audit suppression for checkName=<checkName> already exists for this account" — different fixed text, lower-case "suppression", and the resourceIdentifier omitted entirely.
Why: the suppression's identity is the (checkName, resourceIdentifier) pair, so a client that suppresses one check for many resources cannot tell WHICH resource collided; and exact-match clients keyed on AWS's sentence never match.
Fix: CreateAuditSuppression's duplicate-detection refusal construction, including the JSON rendering of the request's resourceIdentifier (services/service-com-amazonaws-iot-awsiotservice/src/ops/create_audit_suppression.rs).
Traces: iot-audit-suppression-lifecycle#task#6.

Cause 3 of 3: CreateAuditSuppression validates checkName before the expiration definition, so the wrong refusal is reported.
AWS: CreateAuditSuppression models InvalidRequestException (400) and takes expirationDate and suppressIndefinitely as alternatives. Recorded behavior: for a request whose expiration definition is invalid (both an expirationDate and suppressIndefinitely set, or neither), the refusal reported is 400 InvalidRequestException with "Definition for suppression's expiration in request is invalid. Either set isSuppressIndefinitely flag to true or provide a valid expiration date but not both." — wholly fixed text with no variable part — even when the same request ALSO carries an unknown checkName. In other words the expiration-definition check precedes the checkName check.
Emulator: status 400 is right, but the message is "Invalid audit check name: <checkName>" — the checkName branch runs first, and with the emulator's own wording (AWS's unknown-checkName sentence is not exercised by this recording, so it is not the requirement here; the ordering and the expiration sentence are).
Why: a client fixing its request is told the wrong thing: it corrects the check name, resubmits, and is refused again on the expiration definition it was never told about — a two-round trip AWS does not require.
Fix: order CreateAuditSuppression's input validation so the expiration-definition rule is evaluated before the checkName lookup, and emit the expiration sentence above (same handler).
Traces: iot-audit-suppression-lifecycle#task#7.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout. The check name, certificate id and descriptions below are one run's values filling the templates described per cause.

Cause 1 — iot-audit-suppression-lifecycle#task#5, DescribeAuditSuppression, status 200 both sides:
  $.description: expected 'updated probe suppression' (the value the preceding UpdateAuditSuppression supplied), got 'probe suppression' (the create-time value).

Cause 2 — iot-audit-suppression-lifecycle#task#6, CreateAuditSuppression, status 409 both sides:
  expected $.message 'Audit Suppression with checkName=DEVICE_CERTIFICATE_EXPIRING_CHECK and ResourceIdentifier={"deviceCertificateId":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"} already exists'
  actual   $.message 'Audit suppression for checkName=DEVICE_CERTIFICATE_EXPIRING_CHECK already exists for this account'
  Fixed text: "Audit Suppression with checkName=" / " and ResourceIdentifier=" / " already exists"; variables: the request's checkName and its JSON-rendered resourceIdentifier (the 64-hex certificate id is one run's value).

Cause 3 — iot-audit-suppression-lifecycle#task#7, CreateAuditSuppression, status 400 both sides:
  expected $.message "Definition for suppression's expiration in request is invalid. Either set isSuppressIndefinitely flag to true or provide a valid expiration date but not both." (no variable part)
  actual   $.message 'Invalid audit check name: NOT_A_REAL_CHECK'

Model citations: CreateAuditSuppression POST /audit/suppressions/create; required checkName (AuditCheckName), resourceIdentifier (ResourceIdentifier), clientRequestToken (@idempotencyToken @length 1..64 @pattern ^[a-zA-Z0-9-_]+$); optional expirationDate (Timestamp), suppressIndefinitely (boolean), description (@length 0..1000); errors ResourceAlreadyExistsException(409), InvalidRequestException(400), LimitExceededException(410), ThrottlingException(400), InternalFailureException(500). UpdateAuditSuppression PATCH /audit/suppressions/update; same mutable members; errors InvalidRequestException(400), ResourceNotFoundException(404). DescribeAuditSuppression POST /audit/suppressions/describe returns checkName, resourceIdentifier, expirationDate, suppressIndefinitely, description.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. CreateAuditSuppression(checkName='DEVICE_CERTIFICATE_EXPIRING_CHECK', resourceIdentifier={'deviceCertificateId': <64-hex>}, suppressIndefinitely=true, description='probe suppression', clientRequestToken=<token>).
3. UpdateAuditSuppression(same checkName/resourceIdentifier, description='updated probe suppression'), then DescribeAuditSuppression(same key) — the description must be the updated one; the emulator returns the create-time value (Cause 1).
4. CreateAuditSuppression again with the same checkName and resourceIdentifier — the 409 message must name both the checkName and the JSON resourceIdentifier: "Audit Suppression with checkName=<checkName> and ResourceIdentifier=<json> already exists" (Cause 2).
5. CreateAuditSuppression with an unknown checkName AND an invalid expiration definition (both expirationDate and suppressIndefinitely=true) — AWS reports the expiration sentence "Definition for suppression's expiration in request is invalid. Either set isSuppressIndefinitely flag to true or provide a valid expiration date but not both."; the emulator reports 'Invalid audit check name: <checkName>' (Cause 3).
6. Re-replay iot-audit-suppression-lifecycle#task#5, #task#6 and #task#7 to confirm the fix.
