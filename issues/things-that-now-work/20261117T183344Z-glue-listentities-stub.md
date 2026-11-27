Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.entity-apis-nonexistent-connection-rejected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.entity-apis-nonexistent-connection-rejected|run#7|AssertionError|assert list_result == expected, f"unexpected ListEntities refusal: {list_result!r}"
Issue-Commit: 666f7954233f9ddfcb1a0dcc5f57d6c1921794e2

# Implement glue ListEntities: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `666f7954233f9ddfcb1a0dcc5f57d6c1921794e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEntities

## Description
`ListEntities` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-glue-awsglue/src/ops/list_entities.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListEntities` call, so each one unblocks only when `ListEntities` works: `glue.entity-apis-nonexistent-connection-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-glue-awsglue/src/ops/list_entities.rs`. Replaying the corpus, the emulator refused the `ListEntities` call: File "run.py", line 456, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_glue_entity_apis_nonexistent_connection_rejected.py", line 34, in run
    assert list_result == expected, f"unexpected ListEntities refusal: {list_result!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: unexpected ListEntities refusal: ('NotImplemented', 'operation ListEntities is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the glue emulator
Call ListEntities and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
