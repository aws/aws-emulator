Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ram.disassociate-last-permission-from-share-rejected
Issue-Shape-Id: com.amazonaws.ram#AmazonResourceSharing
Issue-Key: ram.disassociate-last-permission-from-share-rejected|run#20|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}: {message!r}"
Issue-Commit: 029c732db8092362c2d6a7aa89bcc4c3ae279d73

# Implement ram DisassociateResourceSharePermission: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `029c732db8092362c2d6a7aa89bcc4c3ae279d73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisassociateResourceSharePermission

## Description
`DisassociateResourceSharePermission` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/disassociate_resource_share_permission.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DisassociateResourceSharePermission` call, so each one unblocks only when `DisassociateResourceSharePermission` works: `ram.disassociate-last-permission-from-share-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ram-amazonresourcesharing/src/ops/disassociate_resource_share_permission.rs`. Replaying the corpus, the emulator refused the `DisassociateResourceSharePermission` call: File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_ram_disassociate_last_permission_from_share_rejected.py", line 201, in run
    assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}: {message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected InvalidParameterException, got NotImplemented: 'operation DisassociateResourceSharePermission is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the ram emulator
Call DisassociateResourceSharePermission and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
