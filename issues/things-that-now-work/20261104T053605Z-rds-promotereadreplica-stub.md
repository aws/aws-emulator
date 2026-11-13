Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.promote-read-replica-non-replica-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|PromoteReadReplica|NotImplemented
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# Implement rds PromoteReadReplica: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PromoteReadReplica

## Description
`PromoteReadReplica` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/promote_read_replica.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PromoteReadReplica` call, so each one unblocks only when `PromoteReadReplica` works: `rds.promote-read-replica-non-replica-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/promote_read_replica.rs`. Replaying the corpus, the emulator refused the `PromoteReadReplica` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_rds_promote_read_replica_non_replica_rejected.py", line 47, in run
    assert code == "InvalidDBInstanceState", f"expected InvalidDBInstanceState, got {code!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected InvalidDBInstanceState, got 'NotImplemented'

## Steps to Reproduce
Boot the rds emulator
Call PromoteReadReplica and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
