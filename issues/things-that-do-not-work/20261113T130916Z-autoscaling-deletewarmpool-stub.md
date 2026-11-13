Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.op-contract-deleteWarmPool autoscaling.op-contract-put-warm-pool autoscaling.warm-pool-lifecycle
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: auto-scaling|DeleteWarmPool|NotImplemented
Issue-Commit: 4370547da342f3cac1e3d2a524b52e751b2d7b18

# Implement autoscaling DeleteWarmPool: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `4370547da342f3cac1e3d2a524b52e751b2d7b18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteWarmPool

## Description
`DeleteWarmPool` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/delete_warm_pool.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `DeleteWarmPool` call, so each one unblocks only when `DeleteWarmPool` works: `autoscaling.op-contract-deleteWarmPool`, `autoscaling.op-contract-put-warm-pool`, `autoscaling.warm-pool-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/delete_warm_pool.rs`. Replaying the corpus, the emulator refused the `DeleteWarmPool` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteWarmPool operation: operation DeleteWarmPool is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the autoscaling emulator
Call DeleteWarmPool and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
