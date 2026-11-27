Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: datazone.lineage-abort-run-event-moves-job-run-to-abort datazone.lineage-complete-run-event-builds-dataset-graph-until-deleted datazone.lineage-event-client-token-reuse-with-changed-body-conflict-rejected datazone.lineage-event-dataset-event-payload-rejected datazone.lineage-event-processed-listed-then-deleted-not-found datazone.lineage-run-events-version-job-node-history datazone.post-lineage-event-non-openlineage-payload-rejected datazone.post-lineage-event-non-run-event-rejected datazone.post-lineage-event-reused-client-token-other-event-conflict-rejected datazone.post-lineage-event-to-deleted-domain-rejected datazone.post-lineage-event-without-event-time-rejected datazone.query-graph-max-path-length-extends-traversal datazone.query-graph-misordered-match-clauses-rejected datazone.query-graph-next-token-pages-downstream-lineage-once datazone.query-graph-pages-upstream-jobs-after-write datazone.query-graph-unissued-next-token-rejected
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|PostLineageEvent|NotImplemented
Issue-Key: datazone.lineage-event-dataset-event-payload-rejected|run#7|AssertionError|assert got == (
Issue-Key: datazone.post-lineage-event-non-openlineage-payload-rejected|run#5|AssertionError|assert got == expected, f"a plain-text lineage payload was answered {got!r}, expected {expected!r}"
Issue-Key: datazone.post-lineage-event-without-event-time-rejected|run#9|AssertionError|assert got == (
Issue-Commit: cd3ed932ac942c4bb826a6f713c43cf93575b0d9

# Implement datazone PostLineageEvent: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `cd3ed932ac942c4bb826a6f713c43cf93575b0d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostLineageEvent

## Description
`PostLineageEvent` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-datazone-datazone/src/ops/post_lineage_event.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

16 recorded unit(s) fail on their `PostLineageEvent` call, so each one unblocks only when `PostLineageEvent` works: `datazone.lineage-abort-run-event-moves-job-run-to-abort`, `datazone.lineage-complete-run-event-builds-dataset-graph-until-deleted`, `datazone.lineage-event-client-token-reuse-with-changed-body-conflict-rejected`, `datazone.lineage-event-dataset-event-payload-rejected`, `datazone.lineage-event-processed-listed-then-deleted-not-found`, `datazone.lineage-run-events-version-job-node-history`, `datazone.post-lineage-event-non-openlineage-payload-rejected`, `datazone.post-lineage-event-non-run-event-rejected`, `datazone.post-lineage-event-reused-client-token-other-event-conflict-rejected`, `datazone.post-lineage-event-to-deleted-domain-rejected`, `datazone.post-lineage-event-without-event-time-rejected`, `datazone.query-graph-max-path-length-extends-traversal`, `datazone.query-graph-misordered-match-clauses-rejected`, `datazone.query-graph-next-token-pages-downstream-lineage-once`, `datazone.query-graph-pages-upstream-jobs-after-write`, `datazone.query-graph-unissued-next-token-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-datazone-datazone/src/ops/post_lineage_event.rs`. Replaying the corpus, the emulator refused the `PostLineageEvent` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PostLineageEvent operation: operation PostLineageEvent is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the datazone emulator
Call PostLineageEvent and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
