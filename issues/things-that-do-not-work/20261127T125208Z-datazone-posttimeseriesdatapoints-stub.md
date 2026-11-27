Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: datazone.list-time-series-data-points-window-and-pages-select-by-timestamp datazone.post-time-series-client-token-reuse-with-different-forms-rejected datazone.post-time-series-data-points-listing-scope-rejected datazone.post-time-series-data-points-unknown-asset-rejected datazone.post-time-series-data-points-unregistered-form-type-rejected datazone.post-time-series-to-deleted-asset-rejected datazone.time-series-data-point-get-serves-each-point-and-delete-empties-series datazone.time-series-data-points-list-get-and-delete-by-form-name datazone.time-series-data-points-window-and-delete
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|PostTimeSeriesDataPoints|NotImplemented
Issue-Key: datazone.post-time-series-data-points-listing-scope-rejected|run#7|AssertionError|assert got == want, f"posting with entityType LISTING answered {got!r}, expected {want!r}"
Issue-Key: datazone.post-time-series-data-points-unregistered-form-type-rejected|run#11|AssertionError|assert code == "ValidationException" and message.startswith(head) and message.endswith(tail), (
Issue-Commit: cd3ed932ac942c4bb826a6f713c43cf93575b0d9

# Implement datazone PostTimeSeriesDataPoints: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `cd3ed932ac942c4bb826a6f713c43cf93575b0d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostTimeSeriesDataPoints

## Description
`PostTimeSeriesDataPoints` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-datazone-datazone/src/ops/post_time_series_data_points.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `PostTimeSeriesDataPoints` call, so each one unblocks only when `PostTimeSeriesDataPoints` works: `datazone.list-time-series-data-points-window-and-pages-select-by-timestamp`, `datazone.post-time-series-client-token-reuse-with-different-forms-rejected`, `datazone.post-time-series-data-points-listing-scope-rejected`, `datazone.post-time-series-data-points-unknown-asset-rejected`, `datazone.post-time-series-data-points-unregistered-form-type-rejected`, `datazone.post-time-series-to-deleted-asset-rejected`, `datazone.time-series-data-point-get-serves-each-point-and-delete-empties-series`, `datazone.time-series-data-points-list-get-and-delete-by-form-name`, `datazone.time-series-data-points-window-and-delete`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-datazone-datazone/src/ops/post_time_series_data_points.rs`. Replaying the corpus, the emulator refused the `PostTimeSeriesDataPoints` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PostTimeSeriesDataPoints operation: operation PostTimeSeriesDataPoints is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the datazone emulator
Call PostTimeSeriesDataPoints and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
