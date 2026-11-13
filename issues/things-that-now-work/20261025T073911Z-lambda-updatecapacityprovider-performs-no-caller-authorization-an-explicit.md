Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.update-capacity-provider-without-permission-denied
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.update-capacity-provider-without-permission-denied|_error#9|AssertionError|raise AssertionError(
Issue-Commit: b0d906bf2f6d2f0ea5fb379cb763f1de287c532e

# lambda UpdateCapacityProvider performs no caller authorization: an explicit IAM deny is accepted with 202 and the scaling config is mutated

> _Note: this issue was identified against the code base at commit `b0d906bf2f6d2f0ea5fb379cb763f1de287c532e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCapacityProvider

## Description
Cause 1 of 1 - lambda:UpdateCapacityProvider is served without evaluating the caller's identity policy, so a principal that is explicitly DENIED the action still mutates the capacity provider.

AWS: Smithy (services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json) models UpdateCapacityProvider as PUT /2025-11-30/capacity-providers/{CapacityProviderName} with success code 202 and client errors InvalidParameterValueException (400), ResourceConflictException (409), ResourceNotFoundException (404), TooManyRequestsException (429) plus ServiceException (500); an authorization refusal is the service-common AccessDeniedException and is therefore not in the per-operation list. Recorded behavior: a caller whose identity-based policy allows `lambda:*` on `*` but carries an explicit `Deny` for `lambda:UpdateCapacityProvider` is refused - HTTP 403, error code `AccessDeniedException` - and the refusal message is produced by IAM's explicit-deny rule from the request itself, i.e. `User: <caller role ARN> is not authorized to perform: lambda:UpdateCapacityProvider on resource: <capacity provider ARN> with an explicit deny in an identity-based policy`. The denied call has no effect (the stored CapacityProviderScalingConfig is unchanged), and the identical call from a fully permitted caller succeeds and persists.

Emulator: the denied caller's call is accepted. The emulator answers HTTP 202 with the complete CapacityProvider body and the requested change applied - the recorded response carried CapacityProviderScalingConfig.MaxVCpuCount updated to the requested value and a refreshed LastModified - so both halves of the contract diverge: there is no AccessDeniedException and the mutation is persisted. The handler bears this out: services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/update_capacity_provider.rs resolves the record, checks only the deleting state, then writes the request's members; nothing in it consults the caller principal or any IAM policy.

Why: a client relying on AWS's behavior gets a silent privilege escalation. Least-privilege policies, SCP/boundary regression tests and any negative-path test that asserts a denied principal cannot rescale a capacity provider all pass against the emulator while the write actually lands, so a policy mistake that AWS would refuse is validated as correct. Callers also never see the 403/AccessDeniedException they are written to catch, so retry/fallback branches are never exercised.

Fix: the authorization step of the UpdateCapacityProvider handler (and the shared principal/policy-evaluation seam Lambda control-plane writes should go through) - services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/update_capacity_provider.rs, whose handle() mutates state with no caller check. Note the open issues 20261025T022656Z-lambda-ten-control-plane-writes-are-accepted-that.md (missing validation/conflict checks on control-plane writes) and 20261025T022656Z-lambda-six-further-causes-aws-lambda-capacityprovider-unknown.md (capacity-provider surface) look adjacent but neither covers IAM authorization of UpdateCapacityProvider; this check was not reported as owned by either.

Traces: lambda.update-capacity-provider-without-permission-denied.

## Evidence
Failing check lambda.update-capacity-provider-without-permission-denied, verdict mismatch:

AssertionError: expected the explicit-deny caller's call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'CapacityProvider': {'CapacityProviderArn': 'arn:aws:lambda:us-west-2:000000000000:capacity-provider:<name>', 'State': 'Active', ..., 'CapacityProviderScalingConfig': {'MaxVCpuCount': 48}, 'LastModified': '...'}}

Expected (AWS): HTTP 403 with error code AccessDeniedException and the explicit-deny message built from the request (`User: <denied role ARN> is not authorized to perform: lambda:UpdateCapacityProvider on resource: <capacity provider ARN> with an explicit deny in an identity-based policy`), and GetCapacityProvider still reporting the pre-call MaxVCpuCount (32).
Actual (emulator): HTTP 202, the full CapacityProvider body, and MaxVCpuCount changed to the requested 48.

## Steps to Reproduce
1. CreateCapacityProvider for a provider with CapacityProviderScalingConfig={'MaxVCpuCount': 32} and wait for State=Active.
2. Create an IAM role with an inline identity policy containing {Effect: Allow, Action: 'lambda:*', Resource: '*'} and {Effect: Deny, Action: 'lambda:UpdateCapacityProvider', Resource: '*'}; assume it and build a lambda client from those credentials.
3. From that restricted client call UpdateCapacityProvider(CapacityProviderName=<name>, CapacityProviderScalingConfig={'MaxVCpuCount': 48}).
   - AWS: HTTP 403, AccessDeniedException, explicit-deny message naming the caller ARN, lambda:UpdateCapacityProvider and the capacity-provider ARN.
   - Emulator: HTTP 202 with the updated CapacityProvider.
4. GetCapacityProvider(CapacityProviderName=<name>) from the unrestricted client: AWS still reports MaxVCpuCount=32; the emulator reports 48, so the denied call also took effect.
