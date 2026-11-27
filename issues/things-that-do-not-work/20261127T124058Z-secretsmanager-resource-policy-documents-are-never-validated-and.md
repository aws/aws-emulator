Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-put-resource-policy-principal-not-an-arn
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-put-resource-policy-principal-not-an-arn secretsmanager-put-resource-policy-statement-without-principal secretsmanager-put-resource-policy-unknown-version secretsmanager-delete-resource-policy-secret-id-length-edges secretsmanager-validate-resource-policy-statement-without-action
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|PutResourcePolicy|4xx->2xx|$.Message
Issue-Key: secretsmanager|PutResourcePolicy|4xx->2xx|$.__type
Issue-Key: secretsmanager|PutResourcePolicy|4xx->2xx|status
Issue-Key: secretsmanager|GetResourcePolicy|2xx->2xx|$.PolicyName
Issue-Key: secretsmanager|DeleteResourcePolicy|2xx->2xx|$.PolicyName
Issue-Key: secretsmanager|GetResourcePolicy|2xx->2xx|$.ResourcePolicy
Issue-Key: secretsmanager|ValidateResourcePolicy|4xx->4xx|$.Message
Issue-Key: secretsmanager|ValidateResourcePolicy|4xx->2xx|$.Message
Issue-Key: secretsmanager|ValidateResourcePolicy|4xx->2xx|$.__type
Issue-Key: secretsmanager|ValidateResourcePolicy|4xx->2xx|status
Issue-Key: secretsmanager|ValidateResourcePolicy|2xx->2xx|$.PolicyValidationPassed
Issue-Key: secretsmanager|ValidateResourcePolicy|2xx->2xx|$.ValidationErrors[*]
Issue-Commit: 34735640b265050797646a7031263995ba2fdc83

# secretsmanager: resource-policy documents are never validated and the PolicyName field is omitted

> _Note: this issue was identified against the code base at commit `34735640b265050797646a7031263995ba2fdc83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
7 net-new causes in the resource-policy surface (PutResourcePolicy / ValidateResourcePolicy / GetResourcePolicy / DeleteResourcePolicy). Two shared faults: (a) the emulator performs no IAM-policy-document validation, so malformed policies are ACCEPTED and persisted where AWS refuses them with MalformedPolicyDocumentException; (b) the resource-policy responses omit the PolicyName field AWS puts on the wire.

Cause 1 of 7: PutResourcePolicy accepts a policy with an unsupported principal.
AWS: Smithy PutResourcePolicy models MalformedPolicyDocumentException at httpError 400, and ResourcePolicy -> NonEmptyResourcePolicyType @required @length{1,20480}. A statement whose Principal.AWS is not a usable principal (not an ARN, account id, or service principal) is refused: HTTP 400, $.__type "MalformedPolicyDocumentException", $.Message "This resource policy contains an unsupported principal." — entirely fixed text for this error class, no variable part. The policy is NOT stored.
Emulator: HTTP 200, no $.__type and no $.Message; the policy is attached.
Why: a client that mis-renders a principal (a template that failed to interpolate, a bare role name instead of an ARN) is told its policy is live. The secret then looks protected by a policy that could never grant or deny anything, and the client never learns at write time.
Fix: PutResourcePolicy's handler must run the policy-document validator before writing to state.
Traces: secretsmanager-put-resource-policy-principal-not-an-arn#task#3 (cluster also covers secretsmanager-put-resource-policy-statement-without-principal#task#3 and secretsmanager-put-resource-policy-unknown-version#task#3).

Cause 2 of 7: GetResourcePolicy omits PolicyName.
AWS: recorded behavior only — the Smithy GetResourcePolicyResponse declares only ARN, Name and ResourcePolicy, so PolicyName is undocumented in the model, but the recorded wire response carries $.PolicyName with the constant value "default" (Secrets Manager's single resource policy is always named "default"; it is fixed text, not a per-run value).
Emulator: the member is absent from the response.
Why: a client or console that reads PolicyName to identify/label the attached policy gets null.
Fix: GetResourcePolicy's serializer — emit PolicyName "default".
Traces: secretsmanager-delete-resource-policy-secret-id-length-edges#task#6 (cluster also covers task#8 of that fixture).

Cause 3 of 7: DeleteResourcePolicy omits PolicyName.
AWS: recorded behavior only — DeleteResourcePolicyResponse is modeled with ARN and Name only, yet the recorded response carries $.PolicyName "default", the same constant as cause 2.
Emulator: the member is absent.
Why: same as cause 2 — the delete acknowledgement does not name the policy it removed.
Fix: DeleteResourcePolicy's serializer.
Traces: secretsmanager-delete-resource-policy-secret-id-length-edges#task#7.

Cause 4 of 7: after the refusal that cause 1 skipped, GetResourcePolicy returns a policy AWS never stored.
AWS: because PutResourcePolicy refused the malformed document, the follow-up GetResourcePolicy reports NO $.ResourcePolicy (the member is absent — NonEmptyResourcePolicyType is only present when a policy is attached) and carries $.PolicyName "default".
Emulator: $.ResourcePolicy is present and contains the malformed document verbatim, and $.PolicyName is absent.
Why: this is the read-side half of cause 1 — the state the emulator serves reflects a write AWS rejected, so a test that asserts "no policy attached after a refused put" passes against AWS and fails against the emulator (and vice versa).
Fix: follows from cause 1 (validate before mutating state) plus cause 2's serializer.
Traces: secretsmanager-put-resource-policy-principal-not-an-arn#task#4 (cluster also covers secretsmanager-put-resource-policy-statement-without-principal#task#4 and secretsmanager-put-resource-policy-unknown-version#task#4).

Cause 5 of 7: ValidateResourcePolicy classifies a statement with no Principal as an unsupported principal instead of a syntax error.
AWS: Smithy ValidateResourcePolicy models MalformedPolicyDocumentException at 400. A statement that OMITS Principal entirely is a structural/syntax defect, refused with 400, $.__type "MalformedPolicyDocumentException", $.Message "This resource policy contains a syntax error." (fixed text).
Emulator: right status and right $.__type, but $.Message "This resource policy contains an unsupported principal."
Why: the two AWS messages distinguish "you left a required field out" from "the value you gave is not a principal". A client (or a CI assertion) that matches on the message fixes the wrong thing.
Fix: the policy-document validator's error classification — a missing required statement field maps to the syntax-error message, not the unsupported-principal one.
Traces: secretsmanager-put-resource-policy-statement-without-principal#task#2.

Cause 6 of 7: ValidateResourcePolicy accepts an unknown policy Version.
AWS: an IAM policy document whose Version is not a recognised value is refused: 400, $.__type "MalformedPolicyDocumentException", $.Message "This resource policy contains a syntax error." (fixed text).
Emulator: HTTP 200 with neither $.__type nor $.Message — the document validates clean.
Why: ValidateResourcePolicy exists precisely so a client can pre-flight a document; an emulator that validates everything clean makes the pre-flight useless and lets a broken document through to PutResourcePolicy.
Fix: the policy-document validator must check the Version field's domain.
Traces: secretsmanager-put-resource-policy-unknown-version#task#2.

Cause 7 of 7: ValidateResourcePolicy reports PolicyValidationPassed=true for a statement with no Action.
AWS: Smithy ValidateResourcePolicyResponse.PolicyValidationPassed -> BooleanType with default false, and ValidationErrors -> ValidationErrorsType of ValidationErrorsEntry{CheckName -> NameType, ErrorMessage}. For a statement missing Action, AWS answers 200 with PolicyValidationPassed false and one ValidationErrors entry {CheckName: "MalformedCheck", ErrorMessage: "Missing required field Action"} — both strings are fixed text for this check (the check name identifies the validator, the message names the omitted member, here "Action").
Emulator: 200 with PolicyValidationPassed true and no ValidationErrors entries.
Why: this is the non-throwing validation path, so there is no error for a client to notice — it reads "passed" and ships a policy AWS would reject, and the per-check findings a client surfaces to users are empty.
Fix: ValidateResourcePolicy's handler — run the malformed-document checks and populate ValidationErrors / PolicyValidationPassed.
Traces: secretsmanager-validate-resource-policy-statement-without-action#task#1.

No open issue carries these replay keys. 20261109T223435Z-cloudformation-secretsmanager-4-net-new-causes mentions a ResourcePolicy Ref defect in the CloudFormation surface, which is a different component and does not cover document validation or PolicyName.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; 45/484 steps mismatched). Verbatim from the divergence clusters:

Cause 1 — PutResourcePolicy, expected 400 got 200:
  status: expected 400, got 200
  $.__type: missing (expected 'MalformedPolicyDocumentException', got None)
  $.Message: missing (expected 'This resource policy contains an unsupported principal.', got None)

Cause 2 — GetResourcePolicy, 200 -> 200:
  $.PolicyName: missing (expected 'default', got None)

Cause 3 — DeleteResourcePolicy, 200 -> 200:
  $.PolicyName: missing (expected 'default', got None)

Cause 4 — GetResourcePolicy, 200 -> 200:
  $.PolicyName: missing (expected 'default', got None)
  $.ResourcePolicy: unexpected_present — expected None, got the malformed document that PutResourcePolicy should have refused (a statement with Principal.AWS = a non-ARN string)

Cause 5 — ValidateResourcePolicy, 400 -> 400:
  $.Message: expected 'This resource policy contains a syntax error.', got 'This resource policy contains an unsupported principal.'

Cause 6 — ValidateResourcePolicy, expected 400 got 200:
  status: expected 400, got 200
  $.__type: missing (expected 'MalformedPolicyDocumentException', got None)
  $.Message: missing (expected 'This resource policy contains a syntax error.', got None)

Cause 7 — ValidateResourcePolicy, 200 -> 200:
  $.PolicyValidationPassed: expected False, got True
  $.ValidationErrors[0]: missing (expected {'CheckName': 'MalformedCheck', 'ErrorMessage': 'Missing required field Action'}, got None)

Model citations (smithy_query, services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/com-amazonaws-secretsmanager-secretsmanager.json): PutResourcePolicy errors include MalformedPolicyDocumentException (client, httpError 400) and PublicPolicyException; ValidateResourcePolicy errors include MalformedPolicyDocumentException (400); ValidateResourcePolicyResponse.PolicyValidationPassed -> BooleanType default false, ValidationErrors -> ValidationErrorsType; ValidationErrorsEntry members CheckName (NameType @length{1,512}) and ErrorMessage; GetResourcePolicyResponse members = {ARN, Name, ResourcePolicy} and DeleteResourcePolicyResponse members = {ARN, Name} — neither models PolicyName, hence "recorded behavior only" for causes 2-4.

## Steps to Reproduce
1. git clone the repo; boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 corpus.
3. Cause 1: secretsmanager-put-resource-policy-principal-not-an-arn#task#3 — PutResourcePolicy whose statement sets Principal.AWS to a non-ARN string; expect 400 / MalformedPolicyDocumentException / "This resource policy contains an unsupported principal.".
4. Cause 4: then replay task#4 of the same fixture — GetResourcePolicy must report no ResourcePolicy and PolicyName "default".
5. Causes 2 and 3: secretsmanager-delete-resource-policy-secret-id-length-edges#task#6 and #task#7 — GetResourcePolicy and DeleteResourcePolicy on a secret with a policy attached; assert PolicyName == "default" in both responses.
6. Cause 5: secretsmanager-put-resource-policy-statement-without-principal#task#2 — ValidateResourcePolicy on a statement with no Principal; expect Message "This resource policy contains a syntax error.".
7. Cause 6: secretsmanager-put-resource-policy-unknown-version#task#2 — ValidateResourcePolicy on a document with an unrecognised Version; expect 400 / MalformedPolicyDocumentException / "This resource policy contains a syntax error.".
8. Cause 7: secretsmanager-validate-resource-policy-statement-without-action#task#1 — ValidateResourcePolicy on a statement with no Action; expect 200 with PolicyValidationPassed=false and ValidationErrors[0] = {CheckName: "MalformedCheck", ErrorMessage: "Missing required field Action"}.
