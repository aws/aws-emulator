Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.ipam-default-scope-deletion-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DeleteIpamScope|NotImplemented
Issue-Commit: abd75e982d55cd9e91541cb3d125feb2d8edb60f

# Implement ec2 DeleteIpamScope: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `abd75e982d55cd9e91541cb3d125feb2d8edb60f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIpamScope

## Description
`DeleteIpamScope` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_ipam_scope.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteIpamScope` call, so each one unblocks only when `DeleteIpamScope` works: `ec2.ipam-default-scope-deletion-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_ipam_scope.rs`. Replaying the corpus, the emulator refused the `DeleteIpamScope` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteIpamScope operation: operation DeleteIpamScope is scaffolded but not implemented in this emulator

## Resolution

`DeleteIpamScope` is implemented: it resolves the scope in the service's state, refuses a
default scope (`IncorrectState`), a scope that still holds pools (`DependencyViolation`), a
repeated delete (`IncorrectState`), a malformed id (`InvalidIpamScopeId.Malformed`) and an
unknown id (`InvalidIpamScopeId.NotFound`), and otherwise answers the scope in
`delete-in-progress` with the clock owing its removal — after which `DescribeIpamScopes`
reports the id as not existing. Covered end-to-end by
`services/service-com-amazonaws-ec2-amazonec2/tests/all/ipam_scope_deletion.rs`.

Residual divergence, left for a follow-up: the unit `ec2.ipam-default-scope-deletion-rejected`
also asserts the exact MESSAGE of the default-scope refusal. Its HTTP status (400) and error
code (`IncorrectState`) now match, but the recorded sentence is not visible to the replay
(it reports only the emulator's own string), so the message this emulator answers states the
documented rule in this resource's recorded wording family rather than the recorded sentence
itself.

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteIpamScope and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
