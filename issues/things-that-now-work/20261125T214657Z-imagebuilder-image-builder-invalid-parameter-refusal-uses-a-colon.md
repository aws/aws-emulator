Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-ibp-traj-infra-config-before-instance-profile
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-ibp-traj-infra-config-before-instance-profile
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|CreateInfrastructureConfiguration|4xx->4xx|$.message
Issue-Commit: 2c036246c52324f8f473f1b5e9b5314460be2843

# Image Builder invalid-parameter refusal uses a colon-prefixed template instead of AWS's "value supplied for parameter" wording

> _Note: this issue was identified against the code base at commit `2c036246c52324f8f473f1b5e9b5314460be2843`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInfrastructureConfiguration

## Description
Cause 1 of 1: CreateInfrastructureConfiguration rejects an instanceProfileName that names no existing IAM instance profile with the right HTTP status and error class, but the error message body is rendered from a locally-invented template rather than AWS's invalid-parameter-value wording.

AWS: Smithy: CreateInfrastructureConfiguration.instanceProfileName is @required (InstanceProfileNameType, @length min 1 max 256, pattern ^[\w+=,.@-]+$); the invalid-parameter error class is InvalidParameterValueException (errorKind client, httpError 400, doc "The value that you provided for the specified parameter is invalid."). The model does not specify message text, so the message wording is recorded behavior only: the recording shows HTTP 400 with the message "The value supplied for parameter 'instanceProfileName' is not valid. The provided instance profile does not exist. Please specify a different instance profile and try again." The fixed text of that template is "The value supplied for parameter '" + <member name> + "' is not valid. " followed by the parameter-specific explanation sentence(s) — here "The provided instance profile does not exist. Please specify a different instance profile and try again." The only variable part is the offending input member name (instanceProfileName), taken from the request; nothing in the message comes from a per-run id, timestamp or generated name.

Emulator: the emulator answers HTTP 400 as expected, but serves the message "Invalid parameter value for instanceProfileName: The provided instance profile does not exist. Please specify a different instance profile and try again." It replaces AWS's leading sentence "The value supplied for parameter '<member>' is not valid." with the prefix form "Invalid parameter value for <member>:", dropping the quoting of the member name and the sentence structure. The trailing explanation sentence matches.

Why: a client that keys off the refusal text — operators reading logs, and especially SDK/CLI users and test suites that assert on or regex-match the invalid-parameter sentence to tell "this named parameter was invalid" from a generic refusal — sees text that never appears in real Image Builder. Tooling that extracts the offending parameter name from the quoted-member form ('instanceProfileName') finds no quoted member and cannot identify which input was rejected, so a retry loop that would correct the instance profile instead treats the failure as opaque. Status and error class are right, so the divergence is invisible to code that only inspects those, and surfaces only once the human-readable contract matters.

Fix: the CreateInfrastructureConfiguration handler's instance-profile validation path in the imagebuilder service crate — specifically the helper that formats the invalid-parameter message before raising the 400. The explanation sentence is already correct; only the leading sentence template needs to be the AWS form, with the member name interpolated and single-quoted, and the same template should be shared by every invalid-parameter refusal in the service so the wording does not drift per operation.

Traces: imagebuilder-ibp-traj-infra-config-before-instance-profile#task#1 (CreateInfrastructureConfiguration, expected 400 / got 400, $.message value_mismatch).

## Evidence
Fixture imagebuilder-ibp-traj-infra-config-before-instance-profile, step task#1, operation CreateInfrastructureConfiguration, service imagebuilder.
Status: expected 400, actual 400 (matches).
$.message value_mismatch:
  Expected (AWS): "The value supplied for parameter 'instanceProfileName' is not valid. The provided instance profile does not exist. Please specify a different instance profile and try again."
  Actual (emulator): "Invalid parameter value for instanceProfileName: The provided instance profile does not exist. Please specify a different instance profile and try again."
Fixed text vs variable parts: fixed text is "The value supplied for parameter '...' is not valid." plus the instance-profile explanation sentence; the single variable part is the rejected input member name (instanceProfileName), taken from the request. No per-run value (request id, timestamp, generated name) participates in this message, so the mismatch is purely the template.
Model citation: Smithy imagebuilder — CreateInfrastructureConfiguration PUT /CreateInfrastructureConfiguration, instanceProfileName @required; InvalidParameterValueException errorKind=client httpError=400.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from it.
2. Replay the wire corpus (replay_wire_corpus) and look at fixture imagebuilder-ibp-traj-infra-config-before-instance-profile, step task#1.
3. Equivalently, by hand: call imagebuilder CreateInfrastructureConfiguration (PUT /CreateInfrastructureConfiguration) with a valid name, instanceTypes and clientToken, and an instanceProfileName that names an IAM instance profile which has NOT been created (the fixture deliberately orders the infrastructure configuration before the instance profile).
4. Observe HTTP 400 (correct) whose JSON body $.message reads "Invalid parameter value for instanceProfileName: The provided instance profile does not exist. Please specify a different instance profile and try again."
5. Expected: $.message is "The value supplied for parameter 'instanceProfileName' is not valid. The provided instance profile does not exist. Please specify a different instance profile and try again."
6. Re-replay imagebuilder-ibp-traj-infra-config-before-instance-profile#task#1 to confirm a fix.
