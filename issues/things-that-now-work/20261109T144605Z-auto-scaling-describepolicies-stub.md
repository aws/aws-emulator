Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__ecs.obs-ecs-3-reservation-metrics-match-registered-resources-and-pressure-exceeds-100
Issue-Key: auto-scaling|DescribePolicies|NotImplemented
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# Implement auto-scaling DescribePolicies: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePolicies

## Description
`DescribePolicies` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/describe_policies.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribePolicies` call, so each one unblocks only when `DescribePolicies` works: `cloudwatch__ec2__ecs.obs-ecs-3-reservation-metrics-match-registered-resources-and-pressure-exceeds-100`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/describe_policies.rs`. Replaying the corpus, the emulator refused the `DescribePolicies` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribePolicies operation: operation DescribePolicies is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the auto-scaling emulator
Call DescribePolicies and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
