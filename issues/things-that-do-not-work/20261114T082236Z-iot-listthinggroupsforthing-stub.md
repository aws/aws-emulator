Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.thing-group-listings-for-thing
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.thing-group-listings-for-thing|_eventually#10|AssertionError|raise AssertionError(f"condition never became true; last error: {last_exc!r}")
Issue-Commit: dafb6a92f3304d60364f819845fa972c02211566

# Implement iot ListThingGroupsForThing: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `dafb6a92f3304d60364f819845fa972c02211566`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListThingGroupsForThing

## Description
`ListThingGroupsForThing` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iot-awsiotservice/src/ops/list_thing_groups_for_thing.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListThingGroupsForThing` call, so each one unblocks only when `ListThingGroupsForThing` works: `iot.thing-group-listings-for-thing`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iot-awsiotservice/src/ops/list_thing_groups_for_thing.rs`. Replaying the corpus, the emulator refused the `ListThingGroupsForThing` call: File "check_iot_thing_group_listings_for_thing.py", line 62, in run
    groups = _eventually(
             ^^^^^^^^^^^^
  File "check_iot_thing_group_listings_for_thing.py", line 32, in _eventually
    raise AssertionError(f"condition never became true; last error: {last_exc!r}")
AssertionError: condition never became true; last error: ClientError('An error occurred (NotImplemented) when calling the ListThingGroupsForThing operation: operation ListThingGroupsForThing is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the iot emulator
Call ListThingGroupsForThing and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
