Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-serverless-get-credentials-input-validation
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-serverless-get-credentials-input-validation redshift-serverless-get-credentials-unknown-target
Issue-Shape-Id: com.amazonaws.redshiftserverless#RedshiftServerless
Issue-Key: redshift-serverless|GetCredentials|4xx->4xx|$.message
Issue-Key: redshift-serverless|GetCredentials|4xx->4xx|$.__type
Issue-Commit: 7815a7c486f6433d295c8c37df76b6ba67dbf690

# GetCredentials input validation emits wrong error class and message

> _Note: this issue was identified against the code base at commit `7815a7c486f6433d295c8c37df76b6ba67dbf690`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentials

## Description
Redshift Serverless GetCredentials rejects bad input with a generic, generically-worded constraint violation instead of the error class and message wording AWS uses. This bundle has 2 causes, both in GetCredentials' input handling.

Cause 1 of 2: constraint-violation message wording for customDomainName (fixture redshift-serverless-get-credentials-input-validation, step task#6).
AWS: Smithy: GetCredentials.customDomainName targets CustomDomainName, @length min 1 max 253, @pattern ^(((?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9])\.)+((?!-)[A-Za-z0-9-]{1,62}[A-Za-z0-9])$; modeled error ValidationException (httpError 400, required member message). Recorded behavior for the over-long value: HTTP 400, __type ValidationException, message 'Invalid length for parameter customDomainName, value: 254, valid min length: 1, valid max length: 253'. The fixed text of that template is 'Invalid length for parameter ', ', value: ', ', valid min length: ', ', valid max length: '; its variable parts are the member name taken from the request ('customDomainName'), the actual length of the submitted value, and the min and max from the member's @length trait. The literal 254/1/253 in the recording are that one request's values filling the template, not part of it.
Emulator: same status (400) and same __type, but the message is the generic smithy-constraint form: "1 validation error detected: Value '<value>' at 'customDomainName' failed to satisfy constraint: Member must have length less than or equal to 253" — it echoes the whole offending value and uses the 'N validation error(s) detected' shape rather than the 'Invalid length for parameter ...' shape AWS uses here.
Why: the error message is a requirement, not cosmetic. Clients and test suites that assert on the message (or parse the limits out of it) see text that never appears in the real API, and operators debugging against the emulator learn the wrong wording. Echoing the full offending value also leaks an arbitrarily long request value into the error where AWS reports only its length.
Fix: GetCredentials' input validation / error serialization for this service — the constraint-violation message formatter must emit the 'Invalid length for parameter <member>, value: <actualLength>, valid min length: <min>, valid max length: <max>' template for a @length violation on this input, with the member name, actual length and bounds filled from the request and the model.

Cause 2 of 2: length check on workgroupName pre-empts workgroup resolution, so a missing workgroup is reported as a validation failure instead of ResourceNotFoundException (fixture redshift-serverless-get-credentials-unknown-target, step task#4; same cause at task#5).
AWS: Smithy: GetCredentials.workgroupName targets WorkgroupName, @length min 3 max 64, @pattern ^[a-z0-9-]+$; modeled errors ResourceNotFoundException (message required, optional resourceName) and ValidationException. Recorded behavior for a workgroupName that does not exist: HTTP 400 with __type ResourceNotFoundException and message 'Serverless workgroup <workgroupName> not found.' — i.e. AWS resolves the named workgroup and reports it missing rather than failing the request on the member's minimum length. The fixed text of that message is 'Serverless workgroup ' and ' not found.'; the only variable part is the workgroup name copied from the request. (Note the recorded status for this error is 400, not the modeled httpError 404; the recording is authoritative for the wire status here.)
Emulator: HTTP 400 with __type ValidationException and message "1 validation error detected: Value 'rs' at 'workgroupName' failed to satisfy constraint: Member must have length greater than or equal to 3" — the wrong error class (ValidationException instead of ResourceNotFoundException) and consequently the wrong message entirely.
Why: the error class is what clients branch on. Code that catches ResourceNotFoundException to create or pick a different workgroup never sees it and instead surfaces an unretryable, uncatchable validation failure; the SDK also models these as distinct exception types, so the emulator breaks exception-type-based control flow, not just message text.
Fix: GetCredentials' handler/validation ordering — workgroup lookup against emulator state must run and raise ResourceNotFoundException with the 'Serverless workgroup <name> not found.' message for a name that resolves to nothing, rather than the input-length check short-circuiting first for this member.

Traces: redshift-serverless-get-credentials-input-validation#task#6 (cause 1); redshift-serverless-get-credentials-unknown-target#task#4 and #task#5 (cause 2). No existing open issue names either replay key.

## Evidence
Replay of trace set simparity-wire-aws-v1 (11 fixtures, 112 steps, 80 passed, 31 mismatched).

Cause 1 — fixture redshift-serverless-get-credentials-input-validation, step task#6, GetCredentials, verdict mismatch, expected status 400 / actual status 400:
  $.message: value_mismatch
  expected: 'Invalid length for parameter customDomainName, value: 254, valid min length: 1, valid max length: 253'
  actual:   "1 validation error detected: Value 'aaaaaaa...<254-char domain>...example.com' at 'customDomainName' failed to satisfy constraint: Member must have length less than or equal to 253"
  ($.__type matched: ValidationException.)

Cause 2 — fixture redshift-serverless-get-credentials-unknown-target, step task#4 (cluster also covers task#5), GetCredentials, verdict mismatch, expected status 400 / actual status 400:
  $.__type:  value_mismatch (expected 'ResourceNotFoundException', got 'ValidationException')
  $.message: value_mismatch (expected 'Serverless workgroup rs not found.', got "1 validation error detected: Value 'rs' at 'workgroupName' failed to satisfy constraint: Member must have length greater than or equal to 3")

Model evidence (services/service-com-amazonaws-redshiftserverless-redshiftserverless/smithy-model/com-amazonaws-redshiftserverless-redshiftserverless.json):
  GetCredentials input: workgroupName -> WorkgroupName @length{min:3,max:64} @pattern ^[a-z0-9-]+$; customDomainName -> CustomDomainName @length{min:1,max:253}
  GetCredentials errors: InternalServerException(500, retryable), ResourceNotFoundException(client, httpError 404), ValidationException(client, httpError 400)
  ResourceNotFoundException members: message (required), resourceName (optional); ValidationException members: message (required)

## Steps to Reproduce
Cause 1: replay fixture redshift-serverless-get-credentials-input-validation and inspect step task#6 — call redshift-serverless GetCredentials with a customDomainName whose length exceeds the member's @length max of 253 (the fixture uses a 254-character dotted domain). Observe HTTP 400 / __type ValidationException, then compare $.message against the expected 'Invalid length for parameter customDomainName, value: <length>, valid min length: 1, valid max length: 253'.

Cause 2: replay fixture redshift-serverless-get-credentials-unknown-target and inspect steps task#4 and task#5 — call GetCredentials with a workgroupName that does not exist in emulator state and is shorter than the member's @length min of 3 (the fixture uses 'rs'). Observe HTTP 400 with $.__type 'ValidationException' and the 'N validation error detected ... length greater than or equal to 3' message, where AWS returns $.__type 'ResourceNotFoundException' with message 'Serverless workgroup rs not found.'.
