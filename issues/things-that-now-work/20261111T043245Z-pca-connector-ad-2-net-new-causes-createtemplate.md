Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pca-connector-ad.create-template-nonexistent-connector-not-found pca-connector-ad.create-service-principal-name-arn-type-mismatch-rejected
Issue-Shape-Id: com.amazonaws.pcaconnectorad#PcaConnectorAd
Issue-Key: pca-connector-ad.create-template-nonexistent-connector-not-found|run#9|AssertionError|assert control_code == "ValidationException", f"expected ValidationException, got {control_code}"
Issue-Key: pca-connector-ad.create-service-principal-name-arn-type-mismatch-rejected|run#9|AssertionError|assert control_code == "ResourceNotFoundException", (
Issue-Commit: 2d1f8bf5471c7181f318f860d2c7fc1188aa0d11

# pca-connector-ad: 2 net-new causes — CreateTemplate's Name @pattern is never enforced (connector lookup wins, answering 404), and CreateServicePrincipalName is a 501 NotImplemented scaffold stub so its ResourceNotFoundException never happens

> _Note: this issue was identified against the code base at commit `2d1f8bf5471c7181f318f860d2c7fc1188aa0d11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTemplate

## Description
Two net-new root causes in pca-connector-ad's rejection paths, both grounded in failing behavioral checks. Bundled because they share the service; they are independent defects and BOTH must be closed.

Cause 1 of 2: CreateTemplate does not evaluate Name's modeled @pattern, and resolves the parent connector before input validation, so a Name AWS forbids is answered with ResourceNotFoundException (404) instead of ValidationException (400).
AWS: HTTP 400, error code ValidationException. Smithy: CreateTemplate.Name -> TemplateName @required @length 1..64 @pattern `^(?!^\s+$)((?![\x5c'\x2b,;<=>#\x22])([\x20-\x7E]))+$`; CreateTemplate models ValidationException @httpError 400 and ResourceNotFoundException @httpError 404; the ValidationException doc reads "An input validation error occurred. For example, invalid characters in a template name". Modeled input validation runs BEFORE the parent lookup, so AWS answers ValidationException even when ConnectorArn names a connector that does not exist. The message is this service's bracketed JSON-schema family: `[ECMA 262 regex "<the modeled pattern, anchors included>" does not match input string "<the rejected Name>"]`. Fixed text: `[ECMA 262 regex `, ` does not match input string `, the trailing `]`, and the double-quoting of both operands. Variable parts: the pattern exactly as the model declares it for TemplateName, and the Name value taken verbatim from the request. There is no member path and no "N validation error(s) detected" prefix in this family. Recorded behavior only for the choice of family and for the ordering (validation before lookup).
Emulator: the same call is answered HTTP 404 ResourceNotFoundException with message `Connector was not found: <connector arn>.` — the Name constraint is never applied. The companion assertion in the same check (a well-formed, nonexistent ConnectorArn with a VALID Name) does get the correct 404 and message, which proves the handler returns ResourceNotFoundException unconditionally rather than discriminating by failure class.
Why: a client is told the parent connector is missing when the real fault is its own input, so repair logic keyed on ValidationException never fires and retry-with-the-same-name loops forever. Worse, against a connector that DOES exist, a template name AWS forbids is accepted and persisted — the emulator mints a resource AWS refuses, and downstream enrollment keyed on name legality then diverges.
Fix: CreateTemplate's input validation — the modeled @pattern/@length constraints for Name must be compiled and enforced in the service's request-validation layer, evaluated before the connector lookup in the operation's handler. Hypothesis: the pattern uses negative lookahead `(?!...)`, which the Rust `regex` crate does not support, so the compiled constraint is dropped silently instead of enforced and the request reaches the handler unchecked.
Traces: check pca-connector-ad.create-template-nonexistent-connector-not-found (its positive control, Name="bad,name" on a nonexistent ConnectorArn). Handler ordering is visible at services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/create_template.rs lines 190-204 ("The parent is resolved FIRST").
Related open issue, which does NOT cover this check: 20261029T224504Z-pca-connector-ad-constraint-rejections-use-the-generic.md reports the same fault from the WIRE corpus (its Cause 2) and carries only wire replay keys, so this behavioral check is not gated by it.

Cause 2 of 2: CreateServicePrincipalName is an unimplemented scaffold stub — every request that clears modeled validation is answered HTTP 501 with the non-AWS error code NotImplemented, so the operation's modeled ResourceNotFoundException is unreachable.
AWS: for a DirectoryRegistrationArn that satisfies its @pattern but names a directory registration the account does not hold, HTTP 404, error code ResourceNotFoundException, message `DirectoryRegistration was not found: <the directoryRegistrationArn from the request>.` Fixed text: `DirectoryRegistration was not found: ` and the terminating `.`; the only variable part is the ARN copied from the request. Smithy: CreateServicePrincipalName models ResourceNotFoundException @httpError 404 ("The operation tried to access a nonexistent resource") alongside ValidationException @httpError 400, and on success answers 202 with an empty (Unit) body; DirectoryRegistrationArn is @required @httpLabel with @pattern `^arn:[\w-]+:pca-connector-ad:[\w-]+:[0-9]+:directory-registration\/d-[0-9a-f]{10}$`. NotImplemented is not a code this service models at all.
Emulator: answers HTTP 501 with error code NotImplemented. The pattern-violation path (a ConnectorArn passed as DirectoryRegistrationArn) IS refused correctly, which confirms only the handler body is missing, not the validation layer.
Why: a client cannot distinguish "this directory registration does not exist" from "this SPN already exists" (ConflictException) from "you are not permitted" (AccessDeniedException); 501 is not retryable or interpretable by any AWS SDK error handler, and the SPN half of the connector lifecycle cannot be set up at all, so nothing downstream of it can be exercised.
Fix: implement the handler body for CreateServicePrincipalName — resolve the DirectoryRegistrationArn and the ConnectorArn against the service state, refuse an unknown registration with the modeled ResourceNotFoundException, and record the SPN; remove the scaffold 501 return.
Traces: check pca-connector-ad.create-service-principal-name-arn-type-mismatch-rejected (its positive control, a pattern-valid directory-registration ARN). The stub is at services/service-com-amazonaws-pcaconnectorad-pcaconnectorad/src/ops/create_service_principal_name.rs lines 4-6 (SCAFFOLDED marker) and line 202 (the `Scaffolded::not_implemented` return).

## Evidence
Cause 1 — pca-connector-ad.create-template-nonexistent-connector-not-found
  First call (ConnectorArn = well-formed ARN of a nonexistent connector, valid Name, valid TemplateV2 Definition): PASSES — 404 ResourceNotFoundException, message 'Connector was not found: <arn>.'
  Positive control, SAME nonexistent ConnectorArn but Name='bad,name':
    expected error code: ValidationException (HTTP 400)
    actual   error code: ResourceNotFoundException (HTTP 404)
    expected message: '[ECMA 262 regex "^(?!^\\s+$)((?![\\x5c\'\\x2b,;<=>#\\x22])([\\x20-\\x7E]))+$" does not match input string "bad,name"]'  (fixed text = the ECMA-262 framing; variables = the modeled TemplateName pattern and the Name from the request)
    actual   message: 'Connector was not found: <arn>.'
  Suite assertion: AssertionError: expected ValidationException, got ResourceNotFoundException

Cause 2 — pca-connector-ad.create-service-principal-name-arn-type-mismatch-rejected
  First call (DirectoryRegistrationArn deliberately set to a ConnectorArn): PASSES — ValidationException with the pattern message.
  Positive control, DirectoryRegistrationArn = 'arn:aws:pca-connector-ad:<region>:<account>:directory-registration/d-1234567890' (pattern-valid, nonexistent):
    expected error code: ResourceNotFoundException (HTTP 404)
    actual   error code: NotImplemented (HTTP 501)
    expected message: 'DirectoryRegistration was not found: <that directoryRegistrationArn>.'  (fixed text = 'DirectoryRegistration was not found: ' + '.'; variable = the ARN from the request)
  Suite assertion: AssertionError: expected ResourceNotFoundException once the ARN shape is a well-formed directory-registration ARN pointing at a nonexistent resource, got 'NotImplemented'

## Steps to Reproduce
Boot the emulator and replay the two checks below, or drive it directly (region us-west-2, default credentials):

Cause 1:
1. pcad.create_template(ConnectorArn='arn:aws:pca-connector-ad:<region>:<account>:connector/<random uuid>', Name='bad,name', Definition={'TemplateV2': {...a complete v2 definition...}})
   -> emulator answers 404 ResourceNotFoundException 'Connector was not found: <arn>.'
   -> AWS answers 400 ValidationException '[ECMA 262 regex "<TemplateName pattern>" does not match input string "bad,name"]', because the Name constraint is evaluated before the connector is looked up.
2. For contrast, the same ConnectorArn with a legal Name correctly yields 404 ResourceNotFoundException — confirming the handler returns 404 regardless of input legality.

Cause 2:
1. pcad.create_service_principal_name(DirectoryRegistrationArn='arn:aws:pca-connector-ad:<region>:<account>:directory-registration/d-1234567890', ConnectorArn='arn:aws:pca-connector-ad:<region>:<account>:connector/<random uuid>')
   -> emulator answers 501 NotImplemented
   -> AWS answers 404 ResourceNotFoundException 'DirectoryRegistration was not found: arn:aws:pca-connector-ad:<region>:<account>:directory-registration/d-1234567890.'
2. For contrast, passing a ConnectorArn in the DirectoryRegistrationArn position IS correctly refused with ValidationException, so the gap is the handler body only.
