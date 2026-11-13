Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudfront.deploy-origin-access-control-invalid-signing-protocol-rejected
Issue-Key: cloudformation__cloudfront.deploy-origin-access-control-invalid-signing-protocol-rejected|run#18|AssertionError|assert rows == [_VALIDATION], f"validation rows {rows!r}, expected {[_VALIDATION]!r}"
Issue-Commit: 9821cff84aed55823f685374d459e562c1c72f3b

# cloudformation__cloudfront: an invalid AWS::CloudFront::OriginAccessControl SigningProtocol produces no PROPERTY_VALIDATION row, so the stack's pre-provision validation reports nothing

> _Note: this issue was identified against the code base at commit `9821cff84aed55823f685374d459e562c1c72f3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause.

Cause 1 of 1: CloudFormation performs no property-pattern validation for AWS::CloudFront::OriginAccessControl, so a SigningProtocol outside the modeled enum yields an empty validation-row set.
  AWS:      Smithy (service com.amazonaws.cloudfront): OriginAccessControlConfig.SigningProtocol is required and targets the enum OriginAccessControlSigningProtocols whose only values are `sigv4` and `sigv4a` (the member doc states "The only valid values are sigv4 and sigv4a"). CloudFormation validates a resource type's property schema before provisioning and records the failure as a validation row for the offending logical resource: logical id, validation type PROPERTY_VALIDATION, status FAILED, verdict FAIL, message `Property value [<value>] does not match pattern: ^(sigv4|sigv4a)$`, and the property path `/Resources/<LogicalId>/Properties/OriginAccessControlConfig/SigningProtocol`. Fixed text of the message: `Property value [` … `] does not match pattern: ^(sigv4|sigv4a)$`; the variable parts are the rejected value taken from the request and the logical id in the property path taken from the template. (The pattern itself is the requirement the model states as the two-value enum.)
  Emulator: the validation-row set for the stack is empty — `[]`. No PROPERTY_VALIDATION row is produced for the out-of-enum SigningProtocol at all, so the pre-provision validation stage reports the template as having no property defects.
  Why:      a client relying on CloudFormation's pre-provision validation to tell it WHICH property failed and WHY gets nothing: no logical resource, no property path, no message. An invalid template is indistinguishable from a valid one at the validation stage, and the caller has no path to the defect it must fix before retrying.
  Fix:      the CloudFormation pre-provision property-validation stage for the AWS::CloudFront::OriginAccessControl resource type — the resource type's schema/constraint checking that emits PROPERTY_VALIDATION rows.
  Traces:   cloudformation__cloudfront.deploy-origin-access-control-invalid-signing-protocol-rejected.

## Evidence
cloudformation__cloudfront.deploy-origin-access-control-invalid-signing-protocol-rejected [mismatch]:
  AssertionError: validation rows [], expected [('OAC', 'PROPERTY_VALIDATION', 'FAILED', 'FAIL', 'Property value [<rejected-signing-protocol>] does not match pattern: ^(sigv4|sigv4a)$', '/Resources/OAC/Properties/OriginAccessControlConfig/SigningProtocol')]

Expected: exactly one PROPERTY_VALIDATION/FAILED/FAIL row naming the OAC logical resource, the pattern message, and the SigningProtocol property path.
Actual:   no validation rows at all.

Smithy grounding: com.amazonaws.cloudfront#OriginAccessControlConfig.SigningProtocol -> com.amazonaws.cloudfront#OriginAccessControlSigningProtocols (enum, required, values [sigv4, sigv4a]).

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite).
2. CreateStack with a template declaring a logical resource `OAC` of type AWS::CloudFront::OriginAccessControl whose OriginAccessControlConfig.SigningProtocol is a value outside the enum (anything other than `sigv4`/`sigv4a`).
3. Read the stack's property-validation rows. AWS reports one PROPERTY_VALIDATION FAILED/FAIL row with the pattern message and the `/Resources/OAC/Properties/OriginAccessControlConfig/SigningProtocol` path; the emulator reports none.
