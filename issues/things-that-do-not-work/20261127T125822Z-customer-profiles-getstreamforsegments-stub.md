Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: customer-profiles.segment-stream-association-reads-back-and-disassociation-stops-it
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles.segment-stream-association-reads-back-and-disassociation-stops-it|run#11|AssertionError|assert before == (
Issue-Commit: cd3ed932ac942c4bb826a6f713c43cf93575b0d9

# Implement customer-profiles GetStreamForSegments: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `cd3ed932ac942c4bb826a6f713c43cf93575b0d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStreamForSegments

## Description
`GetStreamForSegments` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/get_stream_for_segments.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetStreamForSegments` call, so each one unblocks only when `GetStreamForSegments` works: `customer-profiles.segment-stream-association-reads-back-and-disassociation-stops-it`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/get_stream_for_segments.rs`. Replaying the corpus, the emulator refused the `GetStreamForSegments` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_customer_profiles_segment_stream_association_reads_back_and_disassociation_stops_it.py", line 136, in run
    assert before == (
           ^^^^^^^^^^^
AssertionError: a domain with no segment stream answered ('NotImplemented', 'operation GetStreamForSegments is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the customer-profiles emulator
Call GetStreamForSegments and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
