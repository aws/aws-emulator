Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-http-origin-header-carries-cors-allow-origin
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-http-origin-header-carries-cors-allow-origin
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|ListTables|2xx->4xx|$.TableNames
Issue-Key: dynamodb|ListTables|2xx->4xx|status
Issue-Related: 20261117T221515Z-dynamodb-http-edge-omits-cors-response-headers-and.md
Issue-Commit: 0ebf0829cad241d824be0f94d62bd70dc38785d4

# DynamoDB ListTables refuses a well-formed Origin-bearing request with 403 instead of serving it with CORS headers

> _Note: this issue was identified against the code base at commit `0ebf0829cad241d824be0f94d62bd70dc38785d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
THIS BUNDLE CONTAINS 1 CAUSE.

Cause 1 of 1 — a correctly signed, well-formed ListTables that carries an Origin request header is refused HTTP 403 instead of being answered 200, and the cross-origin response headers DynamoDB adds are absent.
AWS: Smithy: ListTables takes ExclusiveStartTableName (@length 3..255, @pattern ^[a-zA-Z0-9_.-]+$) and Limit (@range 1..100), both optional, and returns ListTablesOutput{TableNames: TableNameList, LastEvaluatedTableName}; its only modeled errors are InternalServerError (500) and InvalidEndpointException (421) — there is no modeled client-side refusal for this request at all, and the service shape declares no @cors trait, so the message/header behavior is recorded behavior only. The recording shows DynamoDB serving the request: HTTP 200 with TableNames = [] (an empty list member, not an omitted member, when the account has no tables), plus 'access-control-allow-origin: *' and 'access-control-expose-headers: x-amzn-RequestId,x-amzn-ErrorType,x-amzn-ErrorMessage,Date,smithy-protocol'. Both header values are fixed text, not per-request values. The requirement is that the Origin header takes no part in request authorization: a request that differs from an accepted one only by carrying Origin is still accepted.
Emulator: HTTP 403 — no TableNames member and neither CORS header. The response body is an authorization refusal the model does not list for ListTables.
Why: every browser-origin call to the emulator fails. A web client is handed a 403 for a request a real DynamoDB endpoint serves, so it cannot distinguish a credentials problem from the emulator's Origin handling; and even on the responses it does get, the missing access-control-allow-origin makes the fetch unreadable and the missing access-control-expose-headers hides the request-id and error-type headers an SDK reads to classify errors.
Fix: the HTTP front door's request-authorization path must exclude the Origin header from the material it authenticates/authorizes on, and the shared response-header stage must add the two cross-origin headers to success responses as well as error responses.
Traces: dynamodb-http-origin-header-carries-cors-allow-origin#task#2.

Note on an adjacent open issue: 20261117T221515Z-dynamodb-http-edge-omits-cors-response-headers-and.md describes this same behavior as its cause 2, but its front-matter replay key is Issue-Trace: dynamodb-http-origin-header-carries-cors-allow-origin / Issue-Step: task#1 and its Issue-Trace-Ids names the fixture with no step, so it does NOT carry this step's replay key — the replay report accordingly reports this cluster as unowned. This issue carries task#2 so the fix is gated on the step that actually diverges; the two should be closed together.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. Cluster: service=dynamodb, operation=ListTables, verdict=mismatch, stub_path empty, owned_by empty, 1 affected step (dynamodb-http-origin-header-carries-cors-allow-origin#task#2).

  status: expected 200, got 403
  $.TableNames: missing (expected [], got None)
  $headers.access-control-allow-origin: missing (expected '*', got None)
  $headers.access-control-expose-headers: missing (expected 'x-amzn-RequestId,x-amzn-ErrorType,x-amzn-ErrorMessage,Date,smithy-protocol', got None)

Model citation: ListTables input ExclusiveStartTableName @length 3..255 @pattern ^[a-zA-Z0-9_.-]+$, Limit @range 1..100; output TableNames -> TableNameList; modeled errors only InternalServerError (500) and InvalidEndpointException (421) — no client authorization error is modeled for this operation. The DynamoDB service shape declares no @cors trait, so the header values are recorded behavior only.

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the wire corpus (replay_wire_corpus).
2. Replay fixture dynamodb-http-origin-header-carries-cors-allow-origin step task#2.
3. Equivalently by hand: POST to the DynamoDB endpoint with X-Amz-Target: DynamoDB_20120810.ListTables, an empty JSON body, a valid sigv4 signature and an 'Origin: https://example.com' request header.
4. Observe HTTP 403 with no TableNames and no access-control-* headers; AWS answers 200 with TableNames=[] and both cross-origin headers.
5. Re-send the identical request WITHOUT the Origin header to confirm the refusal is caused by Origin participating in authorization.
