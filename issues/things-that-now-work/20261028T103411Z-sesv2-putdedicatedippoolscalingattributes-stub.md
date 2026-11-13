Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.dedicated-ip-pool-lifecycle
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.dedicated-ip-pool-lifecycle|run#17|AssertionError|assert mutate_err == expected_mutate or mutate_err == expected, (
Issue-Commit: 48883a331e1fae405ebe84b115d9b023bc18cbad

# Implement sesv2 PutDedicatedIpPoolScalingAttributes: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `48883a331e1fae405ebe84b115d9b023bc18cbad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDedicatedIpPoolScalingAttributes

## Description
`PutDedicatedIpPoolScalingAttributes` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_dedicated_ip_pool_scaling_attributes.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `PutDedicatedIpPoolScalingAttributes` call, so each one unblocks only when `PutDedicatedIpPoolScalingAttributes` works: `sesv2.dedicated-ip-pool-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_dedicated_ip_pool_scaling_attributes.rs`. Replaying the corpus, the emulator refused the `PutDedicatedIpPoolScalingAttributes` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_sesv2_dedicated_ip_pool_lifecycle.py", line 43, in run
    assert mutate_err == expected_mutate or mutate_err == expected, (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected a mutation on the deleted pool to be rejected with ('BadRequestException', 'The ScalingMode parameter is invalid.') or ('NotFoundException', 'The requested pool <sim2real-behav-pool-e737623159a4> does not exist.'), got ('NotImplemented', 'operation PutDedicatedIpPoolScalingAttributes is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the sesv2 emulator
Call PutDedicatedIpPoolScalingAttributes and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
