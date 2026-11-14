Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3.e2e-neptune-bridge-carve-lifecycle
Issue-Key: ec2__lambda__s3.e2e-neptune-bridge-carve-lifecycle|phase#7|_PhaseError|raise _PhaseError(
Issue-Commit: 1d457c6418d8d9996f08d1176214615fe9686e31

# Lambda function configuration omits VpcConfig.Ipv6AllowedForDualStack for a VPC-attached function

> _Note: this issue was identified against the code base at commit `1d457c6418d8d9996f08d1176214615fe9686e31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunctionConfiguration

## Description
Cause 1 of 1: the VpcConfig a VPC-attached Lambda function reports has no Ipv6AllowedForDualStack member, so reading it raises KeyError.
AWS: Smithy (services/service-com-amazonaws-lambda-awsgirapiservice) models the function-configuration VpcConfig as VpcConfigResponse with members SubnetIds, SecurityGroupIds, VpcId and Ipv6AllowedForDualStack (target NullableBoolean, documented "Allows outbound IPv6 traffic on VPC functions that are connected to dual-stack subnets"). For a function created with a VpcConfig that does not set it, AWS serialises the member with its default value false — it is present in the response, not absent.
Emulator: the function-configuration response's VpcConfig carries SubnetIds, SecurityGroupIds and VpcId but no Ipv6AllowedForDualStack key at all, so a client indexing it fails with KeyError: 'Ipv6AllowedForDualStack'. The whole multi-phase flow dies at that read (phase 4/7).
Why: a client that reads the flag to decide whether a VPC-attached function may egress over IPv6 — or any code that indexes the member rather than using .get() — crashes instead of seeing False, and the emulator never shows the dual-stack setting at all.
Fix: the Lambda function-configuration serializer for VpcConfigResponse (emit Ipv6AllowedForDualStack, defaulting to false when the function's VpcConfig did not set it) and the stored function state that backs it.
Traces: ec2__lambda__s3.e2e-neptune-bridge-carve-lifecycle.

## Evidence
Replayed behavioral suite, service heading `### ec2__lambda__s3`:
- ec2__lambda__s3.e2e-neptune-bridge-carve-lifecycle — _PhaseError: [phase 4/7 solve] KeyError: 'Ipv6AllowedForDualStack'. The check asserts, after confirming cfg['VpcConfig']['VpcId'] and cfg['VpcConfig']['SubnetIds'], that cfg['VpcConfig']['Ipv6AllowedForDualStack'] is False with the note "nothing set it, so the False default must be present rather than absent".

Smithy: com.amazonaws.lambda#VpcConfigResponse members = SubnetIds, SecurityGroupIds, VpcId, Ipv6AllowedForDualStack (NullableBoolean).

## Steps to Reproduce
Re-replay ec2__lambda__s3.e2e-neptune-bridge-carve-lifecycle. Minimal repro: create a Lambda function with VpcConfig={'SubnetIds': [<subnet>], 'SecurityGroupIds': [<sg>]} (leaving Ipv6AllowedForDualStack unset), then call GetFunctionConfiguration and index response['VpcConfig']['Ipv6AllowedForDualStack'] — AWS returns False, the emulator omits the key and the index raises KeyError.
