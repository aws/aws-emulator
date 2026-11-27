Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2__sts.create-tenant-resource-association-malformed-and-unknown-arn-rejected
Issue-Key: sesv2__sts.create-tenant-resource-association-malformed-and-unknown-arn-rejected|run#11|AssertionError|assert message == expected_malformed_msg, f"expected {expected_malformed_msg!r}, got {message!r}"
Issue-Commit: 7c686d71091806786cdd6101a8e36a3d7eadfcb8

# sesv2 CreateTenantResourceAssociation re-words the malformed-ResourceArn BadRequestException: 'Provided ARN is not in SES resource ARN format' instead of AWS's 'Provided resource identifier is not an SES resource'

> _Note: this issue was identified against the code base at commit `7c686d71091806786cdd6101a8e36a3d7eadfcb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTenantResourceAssociation

## Description
Cause 1 of 1: the refusal message for a ResourceArn that is not an ARN at all is emulator-authored.

AWS: Smithy: sesv2#CreateTenantResourceAssociation POST /v2/email/tenants/resources, inputs TenantName and ResourceArn both @required with @length min 1; modeled errors AlreadyExistsException (400), BadRequestException (400), NotFoundException (404), TooManyRequestsException (429). Recorded behavior: a ResourceArn that is a plain string rather than an ARN is refused with HTTP 400, code `BadRequestException`, and the message "Provided resource identifier is not an SES resource" — fixed text with NO variable part (the offending value is not echoed). An ARN that is well formed but names a configuration set that does not exist is a different class and is refused with NotFoundException naming it.
Emulator: HTTP 400 `BadRequestException` with the message "Provided ARN is not in SES resource ARN format". The status and code are right; the message is a different sentence.
Why: the message is the only thing distinguishing the two input-shape failures a caller must tell apart — "this identifier is not an SES resource at all" versus a malformed-ARN complaint — and clients and operators match on AWS's fixed text when classifying, logging or asserting on refusals. A re-worded sentence makes an exact-match comparison fail and sends a caller looking for an ARN-syntax problem when AWS is saying the identifier does not denote an SES resource.
Fix: the sesv2 CreateTenantResourceAssociation handler's ResourceArn validation — emit AWS's fixed sentence for this refusal class.
Traces: sesv2__sts.create-tenant-resource-association-malformed-and-unknown-arn-rejected.

## Evidence
Expected (AWS): HTTP 400, Error.Code 'BadRequestException', Error.Message 'Provided resource identifier is not an SES resource'.

Actual (emulator), verbatim from the replay: 'AssertionError: expected \'Provided resource identifier is not an SES resource\', got \'Provided ARN is not in SES resource ARN format\''.

The code (BadRequestException) and the 400 status matched; only the message diverges. Both sentences are fixed text — neither carries a value from the request.

## Steps to Reproduce
1. CreateTenant with any tenant name.
2. Call CreateTenantResourceAssociation(TenantName=<tenant>, ResourceArn='<a plain non-ARN string>').
3. Expect HTTP 400 BadRequestException with message 'Provided resource identifier is not an SES resource'; observe 'Provided ARN is not in SES resource ARN format'.
4. For contrast, pass a well-formed configuration-set ARN that does not exist: expect NotFoundException naming it, and ListTenantResources to stay empty.
