Issue-Source: trace-sweep-divergence
Issue-Trace: route53-recovery-control-config-validation-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-recovery-control-config-validation-error-paths
Issue-Shape-Id: com.amazonaws.route53recoverycontrolconfig#Route53RecoveryControlConfig
Issue-Key: route53-recovery-control-config|CreateSafetyRule|4xx->4xx|$.message
Issue-Key: route53-recovery-control-config|CreateSafetyRule|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: route53-recovery-control-config|CreateSafetyRule|4xx->4xx|status
Issue-Commit: e52cf748fe15a6a8583f9d9789193e5a6472c9ef

# CreateSafetyRule answers 400 ValidationException instead of the 401 UnauthorizedException refusal

> _Note: this issue was identified against the code base at commit `e52cf748fe15a6a8583f9d9789193e5a6472c9ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSafetyRule

## Description
Cause 1 of 1: a CreateSafetyRule request that Route 53 ARC refuses at its authorization front door is instead run through the operation's own input validation by the emulator, so the caller gets the operation's 400 ValidationException rather than the 401 UnauthorizedException refusal.

AWS: recorded behavior only. The service's Smithy model (services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/smithy-model/com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig.json) declares for CreateSafetyRule exactly two errors — ValidationException (HTTP 400) and InternalServerException (HTTP 500) — and the model contains NO UnauthorizedException shape at all (smithy_query query="shape" shape="UnauthorizedException" -> not found; nearest modelled shapes are AccessDeniedException / ValidationException / ThrottlingException). So the refusal this step records is not an operation-level modelled error but the Route 53 ARC control-plane front door's authorization refusal, which is decided from the request's SigV4 credential alone and therefore precedes any parsing or validation of the request body. The recording shows that refusal as: HTTP status 401, response header `x-amzn-errortype: UnauthorizedException`, and body member `message` = `Unauthorized`. That message is wholly FIXED text — it has no variable part, nothing is taken from the request, the state, the member names or the rule being created — so reproducing it means emitting the literal word `Unauthorized`, with the error code carried in `x-amzn-errortype` as `UnauthorizedException` and the status as 401. Hypothesis: the step presents a credential the front door will not accept (absent, unparseable, or scoped to another service/region), which is why the refusal is the front door's and not one of the operation's modelled errors; the recording states the response, not the trigger.

Emulator: the request is dispatched straight into the CreateSafetyRule handler, which refuses it on its input and answers HTTP 400 with `x-amzn-errortype: ValidationException` and `message` = `Invalid Rule provided.` (the fixed sentence crate::state::INVALID_RULE_MESSAGE, emitted when a request carries both AssertionRule and GatingRule or neither). Three fields diverge together: status 401 -> 400, `$headers.x-amzn-errortype` `UnauthorizedException` -> `ValidationException`, `$.message` `Unauthorized` -> `Invalid Rule provided.`. Nothing in this service's request path authenticates or authorizes the caller before the handler runs: the only thing that reads the SigV4 credential is crate::state::request_region, which extracts a region label for cluster placement and never rejects a credential, and CreateSafetyRule itself calls only crate::state::read_arn_format (form) and never crate::state::read_resource_arn / authorize_resource_arn, so even the service's own ARN-ownership gate is skipped on this path.

Why: a client relying on AWS's behavior cannot distinguish "I am not authorized to call this endpoint" from "my request body is wrong". AWS SDKs and retry/credential-refresh logic key off the 401/UnauthorizedException pair — a 401 drives credential refresh and re-signing, while a 400 ValidationException is a non-retryable client bug — so a caller whose credentials are bad is told to fix its payload and silently loops or fails, and a test asserting that an unauthorized call is rejected passes for the wrong reason. Worse, because the front-door gate is absent entirely rather than merely mis-coded, requests AWS would never execute can reach the handler and mutate the emulator's store whenever their body happens to be valid, so the emulator is permissive where AWS is closed.

Fix: this belongs in the service's request walk ahead of the operation handler — the front-door authentication/authorization stage for route53-recovery-control-config (the emulator_server SigV4 layer that crate::state::request_region currently only reads a region out of), which must refuse an unacceptable credential with status 401, `x-amzn-errortype: UnauthorizedException` and body `{"message":"Unauthorized"}` BEFORE CreateSafetyRule's input validation in crate::ops::create_safety_rule runs. The handler's own both/neither-rule refusal is correct where it applies; it is only its precedence relative to the authorization gate, and the absence of that gate, that is wrong.

Neither open route53-recovery-control-config issue covers this: 20261029T191943Z-route53-recovery-control-config-listassociatedroute53healthchecks-stub is an unimplemented-operation stub for ListAssociatedRoute53HealthChecks, and 20261030T111123Z-route53-recovery-control-config-configured-safety-rules-are-never-evaluated-an-unsatisfiable concerns data-plane evaluation of stored safety rules by UpdateRoutingControlState, not control-plane authorization precedence.

Traces: route53-recovery-control-config-validation-error-paths#task#4. Code: services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/src/ops/create_safety_rule.rs:186-213 (handler runs with no prior auth gate; line 212 raises the ValidationException seen), src/state.rs:1353 (INVALID_RULE_MESSAGE = "Invalid Rule provided."), src/state.rs:124-129 (request_region reads the SigV4 credential scope but never rejects it), src/state.rs:1241-1279 (read_resource_arn/authorize_resource_arn, the only authorization in the crate, which CreateSafetyRule does not call).

## Evidence
Replay of fixture route53-recovery-control-config-validation-error-paths, step task#4 (operation CreateSafetyRule, verdict mismatch):

Expected (recorded AWS):
  HTTP 401
  x-amzn-errortype: UnauthorizedException
  body: {"message": "Unauthorized"}

Actual (emulator):
  HTTP 400
  x-amzn-errortype: ValidationException
  body: {"message": "Invalid Rule provided."}

Replay detail verbatim:
  status: expected 401, got 400
  $.message: value_mismatch (expected 'Unauthorized', got 'Invalid Rule provided.')
  $headers.x-amzn-errortype: value_mismatch (expected 'UnauthorizedException', got 'ValidationException')

Divergence key: service=route53-recovery-control-config|CreateSafetyRule|status=401->400|keys=$.message,$headers.x-amzn-errortype,status
Affected steps in cluster (1): route53-recovery-control-config-validation-error-paths#task#4

Message-part breakdown: the expected message 'Unauthorized' is entirely fixed text with no request- or state-derived parts; the expected error code 'UnauthorizedException' is fixed and is carried in the x-amzn-errortype header; the expected status 401 is fixed. The emulator's 'Invalid Rule provided.' is likewise fixed text, but it is the operation's input refusal (crate::state::INVALID_RULE_MESSAGE) rather than the front door's authorization refusal.

Supporting model evidence: smithy_query(query="operation", operation="CreateSafetyRule") lists errors [InternalServerException 500, ValidationException 400] only; smithy_query(query="shape", shape="UnauthorizedException") returns "shape 'UnauthorizedException' not found" — confirming the 401 is a front-door refusal rather than an operation-modelled error, so it must be produced ahead of the handler.

## Steps to Reproduce
1. git_clone() the repo and build/boot the emulator from the checkout.
2. replay_wire_corpus() against trace set simparity-wire-aws-v1.
3. Observe fixture route53-recovery-control-config-validation-error-paths, step task#4 (CreateSafetyRule) report verdict=mismatch: expected status 401 with x-amzn-errortype UnauthorizedException and message 'Unauthorized', got status 400 with x-amzn-errortype ValidationException and message 'Invalid Rule provided.'.
4. Equivalently, by hand: send the fixture's recorded POST /safetyrule CreateSafetyRule request (the one whose credential the front door refuses) to the emulator's data listener. The emulator parses the body and answers the handler's 400 ValidationException 'Invalid Rule provided.' instead of refusing the request 401 UnauthorizedException 'Unauthorized' before the body is looked at.
5. All other 26 steps of the corpus pass, so the divergence is isolated to the missing front-door authorization refusal on this path.
