Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3__sqs.interface-endpoint-second-subnet-in-an-occupied-zone-rejected
Issue-Key: ec2__lambda__s3__sqs.interface-endpoint-second-subnet-in-an-occupied-zone-rejected|run#10|AssertionError|assert code == "DuplicateSubnetsInSameZone", (
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# Implement ec2 ModifyVpcEndpoint: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyVpcEndpoint

## Description
`ModifyVpcEndpoint` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_vpc_endpoint.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ModifyVpcEndpoint` call, so each one unblocks only when `ModifyVpcEndpoint` works: `ec2__lambda__s3__sqs.interface-endpoint-second-subnet-in-an-occupied-zone-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_vpc_endpoint.rs`. Replaying the corpus, the emulator refused the `ModifyVpcEndpoint` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_ec2__lambda__s3__sqs_interface_endpoint_second_subnet_in_an_occupied_zone_rejected.py", line 49, in run
    assert code == "DuplicateSubnetsInSameZone", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: adding a second subnet in us-west-2a answered 'NotImplemented': 'operation ModifyVpcEndpoint is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the ec2 emulator
Call ModifyVpcEndpoint and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
