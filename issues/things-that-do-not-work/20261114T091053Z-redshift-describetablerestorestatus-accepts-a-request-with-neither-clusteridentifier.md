Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-empty-inventory-reads
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-empty-inventory-reads
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|DescribeTableRestoreStatus|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|DescribeTableRestoreStatus|4xx->2xx|status
Issue-Commit: 029c732db8092362c2d6a7aa89bcc4c3ae279d73

# Redshift DescribeTableRestoreStatus accepts a request with neither ClusterIdentifier nor TableRestoreRequestId instead of refusing it

> _Note: this issue was identified against the code base at commit `029c732db8092362c2d6a7aa89bcc4c3ae279d73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTableRestoreStatus

## Description
DescribeTableRestoreStatus requires at least one scoping parameter. The emulator treats the fully unscoped call as a legal list-everything read and answers HTTP 200 with an empty result set, so a client never learns that its request was malformed. This issue has 1 cause.

Cause 1 of 1: an unscoped DescribeTableRestoreStatus call is accepted.
AWS: Smithy: DescribeTableRestoreStatus input members ClusterIdentifier and TableRestoreRequestId are both optional in the model, and the modeled errors are ClusterNotFoundFault (httpError 404, code ClusterNotFound) and TableRestoreNotFoundFault (httpError 400, code TableRestoreNotFoundFault) — the model does NOT express the required-combination rule, so this is recorded behavior only: AWS refuses the call with HTTP 400, `Error.Type` `Sender`, `Error.Code` `InvalidParameterCombination` and the message `You must supply one or both of the ClusterIdentifier and TableRestoreRequestId parameters.` The message is entirely fixed text, naming the two member names from the model; no request values are interpolated.
Emulator: HTTP 200 carrying a TableRestoreStatusMessage with no TableRestoreStatusDetails — no validation of the parameter combination at all, and therefore no error body.
Why: a client that omitted (or failed to template) its cluster identifier reads the empty 200 as 'this cluster has no table restores' instead of 'your request was invalid'. That turns a programming error into a silent wrong answer: polling loops waiting for a restore to appear never terminate, and any logic branching on an empty list vs. a refusal takes the wrong branch. It also diverges in status class (2xx vs 4xx), so SDK retry/error handling never engages.
Fix: DescribeTableRestoreStatus' request validation in the redshift handler — reject a request in which both ClusterIdentifier and TableRestoreRequestId are absent/empty with the InvalidParameterCombination sender error and AWS's fixed sentence, before touching the table-restore state.
Traces: redshift-empty-inventory-reads#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1; cluster reported with empty owned_by and empty stub_path (divergence_key service=redshift|DescribeTableRestoreStatus|status=400->200).

redshift-empty-inventory-reads#task#4 DescribeTableRestoreStatus:
- status: expected 400, got 200
- $.ErrorResponse: missing — expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterCombination', 'Message': 'You must supply one or both of the ClusterIdentifier and TableRestoreRequestId parameters.'}, ...}, got None (the emulator returned a 200 DescribeTableRestoreStatus result instead).

The recorded RequestId in the expected body is a per-run value and is not part of the requirement; the required parts are HTTP 400, Error.Type 'Sender', Error.Code 'InvalidParameterCombination' and the fixed message text above.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus for trace set simparity-wire-aws-v1.
2. Call DescribeTableRestoreStatus with neither ClusterIdentifier nor TableRestoreRequestId set (fixture redshift-empty-inventory-reads, step task#4).
3. Observe HTTP 200 with an empty TableRestoreStatusDetails list; AWS answers HTTP 400 InvalidParameterCombination 'You must supply one or both of the ClusterIdentifier and TableRestoreRequestId parameters.'
4. Also confirm the positive paths still work: supplying either parameter alone must keep returning 200.
5. Re-replay redshift-empty-inventory-reads#task#4 after the fix.
