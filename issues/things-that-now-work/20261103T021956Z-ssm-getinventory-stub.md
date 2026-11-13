Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.w22n-inventory-contains-filter-type-rejected-while-begin-with-is-accepted ec2__ssm.w22n-inventory-count-aggregator-on-instance-status-rejected
Issue-Key: ssm|GetInventory|NotImplemented
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# Implement ssm GetInventory: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInventory

## Description
`GetInventory` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ssm-amazonssm/src/ops/get_inventory.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `GetInventory` call, so each one unblocks only when `GetInventory` works: `ec2__ssm.w22n-inventory-contains-filter-type-rejected-while-begin-with-is-accepted`, `ec2__ssm.w22n-inventory-count-aggregator-on-instance-status-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ssm-amazonssm/src/ops/get_inventory.rs`. Replaying the corpus, the emulator refused the `GetInventory` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetInventory operation: operation GetInventory is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ssm emulator
Call GetInventory and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
