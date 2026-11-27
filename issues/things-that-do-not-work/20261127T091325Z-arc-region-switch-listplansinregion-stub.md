Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: arc-region-switch.list-plans-in-region-follows-plan-updates-and-delete arc-region-switch.list-plans-in-region-tracks-plan-lifecycle
Issue-Shape-Id: com.amazonaws.arcregionswitch#ArcRegionSwitch
Issue-Key: arc-region-switch|ListPlansInRegion|NotImplemented
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# Implement arc-region-switch ListPlansInRegion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPlansInRegion

## Description
`ListPlansInRegion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-arcregionswitch-arcregionswitch/src/ops/list_plans_in_region.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ListPlansInRegion` call, so each one unblocks only when `ListPlansInRegion` works: `arc-region-switch.list-plans-in-region-follows-plan-updates-and-delete`, `arc-region-switch.list-plans-in-region-tracks-plan-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-arcregionswitch-arcregionswitch/src/ops/list_plans_in_region.rs`. Replaying the corpus, the emulator refused the `ListPlansInRegion` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListPlansInRegion operation: operation ListPlansInRegion is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the arc-region-switch emulator
Call ListPlansInRegion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
