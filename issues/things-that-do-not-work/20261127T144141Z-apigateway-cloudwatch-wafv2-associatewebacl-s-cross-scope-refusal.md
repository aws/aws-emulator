Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__wafv2.associate-web-acl-rejects-a-cloudfront-scope-acl-for-a-regional-resource
Issue-Key: apigateway__cloudwatch__wafv2.associate-web-acl-rejects-a-cloudfront-scope-acl-for-a-regional-resource|run#8|AssertionError|assert (code, message) == ("WAFInvalidParameterException", expected_message), (
Issue-Commit: 9f25bb510dd848135c623da12e090eb624e6ceaa

# apigateway__cloudwatch__wafv2: AssociateWebACL's cross-scope refusal carries a generic "resource is not supported in current region" message instead of WAF's invalid-ARN sentence

> _Note: this issue was identified against the code base at commit `9f25bb510dd848135c623da12e090eb624e6ceaa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWebACL

## Description
Cause 1 of 1: the message (and therefore the structured Reason/Field/Parameter) of the WAFInvalidParameterException raised when a web ACL's Scope does not match the protected resource's kind is not the sentence AWS emits.

AWS: Smithy (wafv2) — AssociateWebACL takes `WebACLArn` and `ResourceArn` (both target `ResourceArn`, len 20..2048, pattern \S) and models WAFInvalidParameterException (HTTP 400, client error), documented for "Your request references an ARN that is malformed, or corresponds to a resource with which a web ACL can't be associated." That exception shape carries `Reason` (ErrorReason), `Field` (ParameterExceptionField enum) and `Parameter` (ParameterExceptionParameter) alongside `message`, and WAF composes the message from them as `Error reason: <Reason>, field: <Field>, parameter: <Parameter>`. For this case the recorded response is HTTP 400, code `WAFInvalidParameterException`, message:

  Error reason: The ARN isn't valid. A valid ARN begins with arn: and includes other information separated by colons or slashes., field: RESOURCE_ARN, parameter: <the WebACLArn as sent>

Fixed text of that message: `Error reason: The ARN isn't valid. A valid ARN begins with arn: and includes other information separated by colons or slashes., field: RESOURCE_ARN, parameter: ` — every character of that, including the `field: RESOURCE_ARN` token, is constant for this error class. The only variable part is the trailing parameter value, which is the `WebACLArn` string copied verbatim from the request (note: AWS reports `field: RESOURCE_ARN` even though the value echoed is the WebACLArn — that asymmetry is part of AWS's behavior, not a per-run artifact). Nothing in that message comes from the emulator's own state, and nothing in it is a per-run id, host, timestamp or request id.

Emulator: for the same call — a CLOUDFRONT-scope web ACL ARN against a REGIONAL API Gateway stage ARN — it returns code `WAFInvalidParameterException` (correct class) with message `The resource is not supported in current region`. The handler reaches the scope comparison and raises `crate::state::resource_not_supported_in_region_exception()`, a different, generic refusal text, rather than the invalid-ARN exception it already uses for a malformed ARN on the two paths above it. Because the message is not of the `Error reason: ..., field: ..., parameter: ...` form, the structured `Reason`/`Field`/`Parameter` members that make up AWS's sentence are not populated with `The ARN isn't valid. ...` / `RESOURCE_ARN` / the offending ARN either.

Why: a client that validates or surfaces WAF refusals reads `Field` and `Parameter` to say which input it must fix, and matches on WAF's documented reason text. Against the emulator it gets neither: no `Field`, no `Parameter`, and a sentence that reads as a regional-availability/service-enablement problem rather than "this ARN cannot be associated". Operators and retry logic are pushed toward re-trying in another region or treating the deployment as lacking a feature, when the actual fix is to pass a REGIONAL-scope web ACL ARN. Tests and tooling that assert WAF's wording fail against the emulator while passing against AWS.

Fix: the AssociateWebACL handler's scope-mismatch refusal — it must raise the same invalid-ARN WAFInvalidParameterException (Reason = the ARN-isn't-valid sentence, Field = RESOURCE_ARN, Parameter = the WebACLArn as sent) that the handler's two earlier malformed-ARN branches raise, instead of the generic not-supported-in-region exception. The generic helper stays for cases that genuinely mean regional unavailability.

Traces: apigateway__cloudwatch__wafv2.associate-web-acl-rejects-a-cloudfront-scope-acl-for-a-regional-resource. (Not covered by the open elbv2__wafv2 AssociateWebACL issue 20261127T105702Z-elbv2-wafv2-associatewebacl-with-a-non-existent-load.md, which is about the error CLASS chosen for a ResourceArn naming no resource — a different branch of the same handler.)

## Evidence
Replayed check apigateway__cloudwatch__wafv2.associate-web-acl-rejects-a-cloudfront-scope-acl-for-a-regional-resource:

  AssertionError: AssociateWebACL with a CLOUDFRONT-scope web ACL ARN against the REGIONAL stage returned ('WAFInvalidParameterException', 'The resource is not supported in current region'), expected ('WAFInvalidParameterException', "Error reason: The ARN isn't valid. A valid ARN begins with arn: and includes other information separated by colons or slashes., field: RESOURCE_ARN, parameter: arn:aws:wafv2:us-east-1:000000000000:global/webacl/sim2real-wafscope-cf-7df9031496fb/bbd99e32-d243-4159-a5d5-0282fd363e42")

Expected: HTTP 400, Error.Code `WAFInvalidParameterException`, Error.Message `Error reason: The ARN isn't valid. A valid ARN begins with arn: and includes other information separated by colons or slashes., field: RESOURCE_ARN, parameter: <WebACLArn as sent>`.
Actual:   HTTP 400, Error.Code `WAFInvalidParameterException`, Error.Message `The resource is not supported in current region`.

The account id, web ACL name suffix and uuid in the quoted ARN are per-run values that only fill the message's trailing parameter slot; the requirement is the fixed text plus the request's own WebACLArn. The error code and HTTP status already match — only the message/structured reason diverges.

Emulator code path (for orientation; the defect is the behavior, not these lines): services/service-com-amazonaws-wafv2-awswaf-20190729/src/ops/associate_web_acl.rs lines 291-303 return `crate::state::resource_not_supported_in_region_exception()` on `scope != form.scope`, whereas lines 213-224 use `crate::state::invalid_arn_exception(..)` for a malformed ARN.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Against us-east-1, CreateWebACL(Name=<unique>, Scope='CLOUDFRONT', DefaultAction={'Allow':{}}, VisibilityConfig={...}) and keep Summary.ARN.
3. In the regional endpoint, create an API Gateway REST API with a MOCK 'ANY' method on the root and a '{proxy+}' resource, then CreateDeployment(stageName='prod'); the stage ARN is arn:aws:apigateway:<region>::/restapis/<api-id>/stages/prod.
4. Call wafv2 (regional) AssociateWebACL(WebACLArn=<the CLOUDFRONT-scope ACL ARN from step 2>, ResourceArn=<the stage ARN from step 3>).
5. Inspect Error.Code and Error.Message on the raised ClientError. Observed: ('WAFInvalidParameterException', 'The resource is not supported in current region'). Expected: ('WAFInvalidParameterException', "Error reason: The ARN isn't valid. A valid ARN begins with arn: and includes other information separated by colons or slashes., field: RESOURCE_ARN, parameter: <the WebACLArn sent in step 4>").
6. The rest of the check (GetWebACLForResource reporting no association, then a REGIONAL-scope ACL associating and reading back on the same stage) passes and is unaffected.
