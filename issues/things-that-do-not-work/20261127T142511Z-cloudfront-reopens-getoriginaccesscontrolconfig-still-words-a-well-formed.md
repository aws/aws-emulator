Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.get-origin-access-control-config-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.get-origin-access-control-config-nonexistent-rejected|run#9|AssertionError|assert message == "The specified origin access control does not exist.", (
Issue-Commit: 9e2ba0678642d895992d831c5d4d97ae7316945d

# cloudfront: REOPENS — GetOriginAccessControlConfig still words a well-formed absent OAC id as 'Id format is invalid' instead of the NoSuchOriginAccessControl sentence

> _Note: this issue was identified against the code base at commit `9e2ba0678642d895992d831c5d4d97ae7316945d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOriginAccessControlConfig

## Description
REOPENS 20261118T131652Z-cloudfront-getoriginaccesscontrolconfig-reports-a-well-formed-absent-id.md: its fix left the call refused. That issue carried this very replay key (cloudfront.get-origin-access-control-config-nonexistent-rejected) and is marked fixed, but the check still fails on the same refusal wording on the same route, so this is not a new defect — it is the same inversion still live.

Cause 1 of 1: GetOriginAccessControlConfig reports a well-formed but absent origin-access-control id as a malformed identifier.
AWS:      HTTP 404 with error code NoSuchOriginAccessControl and Error.Message "The specified origin access control does not exist." Smithy (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json): GetOriginAccessControlConfig — GET /2020-05-31/origin-access-control/{Id}/config, success code 200; input Id is @required @httpLabel on a plain string with NO @pattern and NO @length constraint; modeled errors are only AccessDenied (httpError 403) and NoSuchOriginAccessControl (httpError 404, doc "The origin access control does not exist."). Because the model constrains the id's form in no way, AWS carries any supplied value through to the state lookup and reports the resource ABSENT; there is no modeled basis for a malformed-identifier refusal on this route. The sentence is entirely FIXED text for this error class — no id, name, value, limit or member name from the request or from state is interpolated into it — so reproducing the sentence verbatim is the whole requirement (wording is recorded behavior; the model supplies the error shape, not the prose).
Emulator: answers the same HTTP 404 and the same code NoSuchOriginAccessControl, but Error.Message reads 'Id format is invalid' — fixed text of a malformed-argument class AWS does not select here, emitted by a pre-lookup id-shape predicate on the /{Id}/config route. The check's code assertion passed; ONLY Error.Message diverges.
Why:      'Id format is invalid' tells a caller its own id handling is broken (a non-retryable programming error), while "The specified origin access control does not exist." tells it the OAC is gone and the correct response is to create or re-resolve it. Create-if-absent logic, idempotent teardown that swallows not-found by message, and tests that branch on the wording all take the wrong branch. Worse, the emulator's pre-lookup form predicate is narrower than AWS's (which has none), so ids AWS would resolve may never be looked up against state at all.
Fix:      GetOriginAccessControlConfig's request validation / handler for GET /2020-05-31/origin-access-control/{Id}/config in the cloudfront origin-access-control component — drop the pre-lookup malformed-id refusal for a form the model does not constrain, and emit NoSuchOriginAccessControl 404 carrying its fixed not-found sentence whenever the id names no origin access control. Note the sibling family: the still-open 20261121T003208Z-cloudfront-2-net-new-causes-an-associated-cloudfront.md records the identical inversion but carries only the op-contract check's key, and 20261126T214412Z-cloudfront-single-resource-error-paths-mis-word-an.md describes the OPPOSITE direction on GetOriginAccessControl (an unreadable crossed-in id that must keep 'Id format is invalid'); both branches must stay separately decidable, so a fix must not simply swap the two sentences.
Traces:   cloudfront.get-origin-access-control-config-nonexistent-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: 657/709 checks passed (92.7%), bootstrap_signal false; the check is listed under service heading '### cloudfront' and is marked neither ALREADY FILED nor OWNED.

Check cloudfront.get-origin-access-control-config-nonexistent-rejected, verdict mismatch:
  File "check_cloudfront_get_origin_access_control_config_nonexistent_rejected.py", line 36, in run
    assert message == "The specified origin access control does not exist.", (
  AssertionError: expected AWS's exact not-found message, got 'Id format is invalid'

The preceding assertion `code == "NoSuchOriginAccessControl"` PASSED, so the 404 status and the error code already match AWS; only Error.Message diverges.

Expected vs actual for GetOriginAccessControlConfig(Id=<well-formed id never created>):
  expected: HTTP 404, Code NoSuchOriginAccessControl, Error.Message 'The specified origin access control does not exist.'
  actual:   HTTP 404, Code NoSuchOriginAccessControl, Error.Message 'Id format is invalid'

Message anatomy: 'The specified origin access control does not exist.' is fixed text in full for this class — nothing is templated from the request or from emulator state, so no value from the recorded run is part of the requirement. The per-run id the check generates (an 'nonexistent-oac-' token) and the request id in the envelope are per-run values and are NOT requirements.

Positive control in the same check passed: an OAC created with {SigningProtocol 'sigv4', SigningBehavior 'always', OriginAccessControlOriginType 's3'} is read back by GetOriginAccessControlConfig with the matching Name/SigningProtocol/OriginAccessControlOriginType, so the success path of the route is intact.

Model evidence: GetOriginAccessControlConfig input {Id: string, @httpLabel, @required, no @pattern, no @length}; http {GET /2020-05-31/origin-access-control/{Id}/config, code 200}; errors AccessDenied(403), NoSuchOriginAccessControl(404).

## Steps to Reproduce
1. Boot the emulator built from this checkout and point a cloudfront client (us-east-1) at it.
2. CreateOriginAccessControl with OriginAccessControlConfig {Name:<unique>, Description:<any>, SigningProtocol:'sigv4', SigningBehavior:'always', OriginAccessControlOriginType:'s3'}; call GetOriginAccessControlConfig on the returned Id — it succeeds (positive control).
3. Call GetOriginAccessControlConfig with an Id that names no origin access control (any well-formed string never created, e.g. a 'nonexistent-oac-<token>' value).
4. Observe HTTP 404, Code 'NoSuchOriginAccessControl', Error.Message 'Id format is invalid'. Expected HTTP 404, Code 'NoSuchOriginAccessControl', Error.Message 'The specified origin access control does not exist.' Compare ONLY Error.Message — status and code already match.
5. Confirm the unreadable-identifier branch on the sibling route is not regressed in the other direction: GetOriginAccessControl with an identifier of another CloudFront resource kind must still answer 404 'Id format is invalid' (see 20261126T214412Z-cloudfront-single-resource-error-paths-mis-word-an.md).
6. Confirm the modeled contract with smithy_query(query="operation", operation="GetOriginAccessControlConfig", model_path="services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json") — Id carries no @pattern/@length, and NoSuchOriginAccessControl is the only not-found error on the route.
7. Re-replay check cloudfront.get-origin-access-control-config-nonexistent-rejected to confirm the fix.
