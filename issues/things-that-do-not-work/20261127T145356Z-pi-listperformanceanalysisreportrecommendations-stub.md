Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pi.analysis-report-reads-reject-mismatched-service-type
Issue-Shape-Id: com.amazonaws.pi#PerformanceInsightsv20180227
Issue-Key: pi|ListPerformanceAnalysisReportRecommendations|NotImplemented
Issue-Commit: ae4983b2ee9be0c2b746a2748bc3435fe07ec59f

# Implement pi ListPerformanceAnalysisReportRecommendations: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ae4983b2ee9be0c2b746a2748bc3435fe07ec59f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPerformanceAnalysisReportRecommendations

## Description
`ListPerformanceAnalysisReportRecommendations` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/list_performance_analysis_report_recommendations.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListPerformanceAnalysisReportRecommendations` call, so each one unblocks only when `ListPerformanceAnalysisReportRecommendations` works: `pi.analysis-report-reads-reject-mismatched-service-type`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-pi-performanceinsightsv20180227/src/ops/list_performance_analysis_report_recommendations.rs`. Replaying the corpus, the emulator refused the `ListPerformanceAnalysisReportRecommendations` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListPerformanceAnalysisReportRecommendations operation: operation ListPerformanceAnalysisReportRecommendations is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the pi emulator
Call ListPerformanceAnalysisReportRecommendations and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
