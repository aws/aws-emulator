Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-content-type-routing-rejects-non-json-media-types
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-content-type-routing-rejects-non-json-media-types dynamodb-legacy-http-unknown-target-refused dynamodb-root-http-method-surface
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|ListTables|4xx->4xx|status
Issue-Key: dynamodb|ListTables|2xx->2xx|$headers.content-type
Issue-Key: dynamodb|ListTables|4xx->4xx|$headers.x-amz-crc32
Issue-Key: dynamodb|ListTables|4xx->5xx|$.__type
Issue-Key: dynamodb|ListTables|4xx->5xx|$.message
Issue-Key: dynamodb|ListTables|4xx->5xx|$headers.x-amz-crc32
Issue-Key: dynamodb|ListTables|4xx->5xx|status
Issue-Key: dynamodb|ListTables|2xx->2xx|$
Issue-Related: 20261111T221143Z-dynamodb-control-plane-and-tagging-operations-answer-validationexception.md
Issue-Related: 20261111T221143Z-dynamodb-malformed-request-bodies-and-explicit-nulls-are.md
Issue-Commit: 0e3e027770f68a801846f0512bd9649d89ae04a7

# dynamodb: the HTTP front door diverges on unknown targets, media types, root paths and response headers

> _Note: this issue was identified against the code base at commit `0e3e027770f68a801846f0512bd9649d89ae04a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables

## Description
The emulator's HTTP front door answers with the wrong status, the wrong error class and the wrong headers before any DynamoDB operation is reached. 7 causes.

Cause 1 of 7: a non-JSON media type is refused 400 instead of 404.
AWS: recorded behavior — a request whose Content-Type is a non-JSON media type is not routed to an operation at all; the endpoint answers status 404.
Emulator: answers status 400.
Why: a client that distinguishes "no such route/media type" (404) from "bad request for this operation" (400) mis-classifies the failure, and content-negotiation probes read as operation errors.
Fix: the HTTP router's media-type dispatch — unroutable media types must produce 404, not fall into the operation error path.
Traces: dynamodb-content-type-routing-rejects-non-json-media-types#task#4.

Cause 2 of 7: a successful response carries the wrong Content-Type.
AWS: recorded behavior — this ListTables 200 is served with Content-Type 'application/json'.
Emulator: served Content-Type 'application/x-amz-json-1.0'.
Why: proxies, gateways and clients that branch on the response media type mis-handle the body.
Fix: the response serializer's Content-Type selection in the HTTP layer.
Traces: dynamodb-content-type-routing-rejects-non-json-media-types#task#8.

Cause 3 of 7: an unparseable x-amz-target is reported as SerializationException with emulator prose, and the x-amz-crc32 header is missing.
AWS: recorded behavior — status 400 with __type 'com.amazon.coral.service#UnknownOperationException', NO 'message' member at all, and the response header x-amz-crc32 set to the CRC32 of the response body.
Emulator: status 400 with __type 'SerializationException' (also un-namespaced) and an emulator-authored $.message describing its own header parsing ('invalid header `x-amz-target`: expected `Service.Operation`, got ...'); x-amz-crc32 absent.
Why: a client that retries only on specific classes, or validates body integrity from x-amz-crc32, cannot: the class is wrong and the checksum header is gone. The invented message also leaks internal parser wording.
Fix: the HTTP target-dispatch layer — an x-amz-target that names no bound operation (including one that does not parse as Service.Operation) must answer com.amazon.coral.service#UnknownOperationException with no message member, and the response pipeline must always emit x-amz-crc32.
Traces: dynamodb-legacy-http-unknown-target-refused#task#1.

Cause 4 of 7: a well-formed x-amz-target naming an operation that does not exist answers 501 NotImplemented instead of 400 UnknownOperationException.
AWS: recorded behavior — status 400, __type 'com.amazon.coral.service#UnknownOperationException', no message member, x-amz-crc32 present. AWS has no notion of an unimplemented operation on this surface: a target it does not know is simply unknown.
Emulator: status 501 with __type 'NotImplemented' and the emulator-authored message 'operation <Name> is not implemented in this emulator'.
Why: 501 is a server-fault class, so SDK retry logic treats a permanently unknown operation as a transient server problem and retries it; the message also advertises that the endpoint is an emulator.
Fix: the target-dispatch layer must map an unrecognised-but-well-formed target to UnknownOperationException(400); the not-implemented path must not be reachable for a target AWS itself does not bind.
Traces: dynamodb-legacy-http-unknown-target-refused#task#2.

Cause 5 of 7: the same unknown-target refusal additionally carries the wrong Content-Type.
AWS: recorded behavior — 400 UnknownOperationException with no message, Content-Type 'application/json', x-amz-crc32 present.
Emulator: __type 'SerializationException', invented $.message, Content-Type 'application/x-amz-json-1.0', no x-amz-crc32.
Why: as causes 2 and 3 combined.
Fix: as causes 2 and 3, on the unknown-target refusal path.
Traces: dynamodb-legacy-http-unknown-target-refused#task#4.

Cause 6 of 7: the endpoint root serves a JSON ListTables result instead of its plain-text health response.
AWS: recorded behavior — the DynamoDB endpoint's root, reached with this method, answers a text/plain body of the form 'healthy: dynamodb.<region>.amazonaws.com ' (the host is taken from the endpoint being addressed; the literal word 'healthy: ' is the fixed part).
Emulator: answered Content-Type 'application/x-amz-json-1.0' with the JSON body {"TableNames": []} — it routed a root request to ListTables.
Why: health probes and load-balancer checks that read the root body get JSON where they expect the plain-text sentence, and a bare root request mutates nothing but is mis-attributed to an operation.
Fix: the HTTP router — bind the endpoint root to the plain-text health response rather than defaulting to the JSON operation dispatcher.
Traces: dynamodb-root-http-method-surface#task#2.

Cause 7 of 7: root requests with methods AWS does not serve answer 200 instead of 404.
AWS: recorded behavior — status 404 for these methods at the root.
Emulator: status 200.
Why: method-surface probes conclude the endpoint supports methods it does not, and a mistyped request succeeds where AWS refuses it.
Fix: the HTTP router's method table for the root path.
Traces: dynamodb-root-http-method-surface#task#3.

Related but not covering: open issue 20261111T170613Z-dynamodb-the-http-sigv4-layer-omits-x-amz.md records the x-amz-crc32 omission and the request-id/auth behaviour; it does not carry these clusters' replay keys and does not cover the UnknownOperationException class, the 501 mapping, the media-type routing status, the Content-Type values or the root-path surface.

## Evidence
Cause 1: ListTables — status expected 404, got 400.
Cause 2: ListTables 200 — $headers.content-type expected 'application/json', got 'application/x-amz-json-1.0'.
Cause 3: ListTables — $.__type expected 'com.amazon.coral.service#UnknownOperationException', got 'SerializationException'; $.message expected absent, got 'invalid header `x-amz-target`: expected `Service.Operation`, got <target>'; $headers.x-amz-crc32 expected present, got absent.
Cause 4: ListTables — status expected 400, got 501; $.__type expected 'com.amazon.coral.service#UnknownOperationException', got 'NotImplemented'; $.message expected absent, got 'operation <Name> is not implemented in this emulator'; $headers.x-amz-crc32 missing.
Cause 5: ListTables — as cause 3 plus $headers.content-type expected 'application/json', got 'application/x-amz-json-1.0'.
Cause 6: root — expected a text/plain body 'healthy: dynamodb.<region>.amazonaws.com ', got Content-Type 'application/x-amz-json-1.0' and body {'TableNames': []}.
Cause 7: root — status expected 404, got 200.

## Steps to Reproduce
1. Replay dynamodb-content-type-routing-rejects-non-json-media-types#task#4 (expect 404) and #task#8 (expect Content-Type application/json).
2. Replay dynamodb-legacy-http-unknown-target-refused#task#1, #task#2 and #task#4: each must answer 400 with __type com.amazon.coral.service#UnknownOperationException, no message member, Content-Type application/json and an x-amz-crc32 header; none may answer 501.
3. Replay dynamodb-root-http-method-surface#task#2 (expect the text/plain 'healthy: <endpoint host> ' body) and #task#3 (expect 404).
