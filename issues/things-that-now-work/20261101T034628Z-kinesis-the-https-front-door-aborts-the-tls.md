Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.conc-limits-and-fanout-shard-record-rate-quota-throttles-tiny-records-rejected
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis.conc-limits-and-fanout-shard-record-rate-quota-throttles-tiny-records-rejected|_producer#21|SSLError|resp = kp.put_records(StreamName=stream, Records=entries)
Issue-Commit: fecfddbbbf177cdbadf2635d78f2917398d0bf54

# kinesis: the HTTPS front door aborts the TLS session with a fatal decode_error alert under concurrent PutRecords connections, so the per-shard write-rate check never reaches the data plane

> _Note: this issue was identified against the code base at commit `fecfddbbbf177cdbadf2635d78f2917398d0bf54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecords

## Description
One net-new root cause (Cause 1 of 1), grounded in the single failing `kinesis` check of suite simparity-behavioral-aws-v1 that the replay report marks neither ALREADY FILED nor OWNED (overall 154/284; kinesis 132/207). A fixer must close this one cause before submitting.

Cause 1 of 1: the emulator's HTTPS listener kills the TLS session with a fatal alert when a single client drives many concurrent requests over a multi-connection pool.

AWS: recorded behavior only — the Smithy model describes no transport behavior. AWS's kinesis endpoint terminates TLS for an arbitrary number of simultaneous connections from one client; a producer that opens a connection pool and issues concurrent PutRecords calls receives, per call, either a modelled 200 PutRecordsOutput (`Records` with per-entry `SequenceNumber`/`ShardId`, `FailedRecordCount` only when >=1 entry was refused) or a modelled error from PutRecords' error set — Smithy: PutRecords errors include ProvisionedThroughputExceededException (client, httpError 400) and InvalidArgumentException; PutRecords.Records is @length(min 1, max 500). Transport is never the answer: AWS never answers a well-formed request with a TLS-layer abort, so per-shard write-quota refusal is always visible as a *per-entry* `ErrorCode` of `ProvisionedThroughputExceededException` on a 200 response, never as a broken connection.

Emulator: the check creates a 1-shard PROVISIONED stream, succeeds on a 3-record control PutRecords, then opens a second kinesis client with `retries.total_max_attempts=1` and `max_pool_connections=18` and runs 16 threads calling PutRecords (500 tiny entries each) back to back. The emulator's listener then sends a fatal TLS `decode_error` alert and the call dies in the transport, not in the API: `botocore.exceptions.SSLError: SSL validation failed for https://127.0.0.1:<port>/ [SSL: TLSV1_ALERT_DECODE_ERROR] tlsv1 alert decode error` (raised from botocore `httpsession.py` `send` -> `endpoint.py` `_send`). `TLSV1_ALERT_DECODE_ERROR` is an alert *received from the peer*, i.e. the emulator's TLS terminator failed to decode a TLS record the client sent; the same listener handshakes successfully for the other 206 kinesis checks in this suite, which are all low-concurrency, and for this check's own single-threaded control phase. The distinguishing input is concurrency: multiple simultaneous TLS connections from one pool. Hypothesis: the TLS/connection accept path is not safe for concurrent handshakes or multiplexes record state across sockets, so one connection decodes another's bytes.

Why: a client gets a transport failure instead of an AWS answer. Botocore surfaces it as SSLError, which is not a ClientError and carries no `Error.Code`, so no retry policy, no `ProvisionedThroughputExceededException` branch and no partial-batch handling in producer code (the KPL pattern of resending only entries whose result carries an `ErrorCode`) can ever run. Any realistically concurrent producer — the normal way Kinesis is written against — intermittently loses whole batches with an unclassifiable error, and the acknowledged-vs-stored invariant (refused entries absent from the shard, acknowledged entries readable) becomes untestable and unenforceable. It also hides every data-plane behaviour behind it: this check can never get as far as asserting the per-shard record-count quota.

Fix: the shared HTTPS front door — the TLS terminator and its per-connection accept/read loop in the emulator's server/transport layer, not any kinesis operation handler. Each accepted socket must own its TLS session state and handshake independently of concurrent ones; concurrent connections from one client pool must each complete a handshake and be served.

Related open issues, named so they are not duplicated and so a fixer does not assume they cover this: 20261027T172504Z-kinesis-10-net-new-root-causes-the-per.md carries this same check id in its Issue-Trace-Ids for a DIFFERENT failure mode — its replay key is `kinesis.conc-limits-and-fanout-shard-record-rate-quota-throttles-tiny-records-rejected|run#23|AssertionError|assert refused, (`, i.e. the check used to reach `assert refused` and fail because the per-shard WRITE quota is never enforced (PutRecords is never partial). That cause stays that issue's. This issue is only the transport abort that now stops the check before it can make that assertion — the key here is an `SSLError` from botocore's `_send`, which no open issue carries. Fixing the TLS layer will re-expose that quota assertion, which 20261027T172504Z owns.

Traces: kinesis.conc-limits-and-fanout-shard-record-rate-quota-throttles-tiny-records-rejected (verdict: error).

## Evidence
Check: kinesis.conc-limits-and-fanout-shard-record-rate-quota-throttles-tiny-records-rejected (verdict: error).

Expected (AWS): every concurrent PutRecords call over a pooled HTTPS client returns an HTTP response — 200 with a `Records` list of 500 per-entry results (each either a `SequenceNumber`+`ShardId` or an `ErrorCode`/`ErrorMessage`, with `FailedRecordCount` matching the number of entries carrying an `ErrorCode`), or one of PutRecords' modelled 400 errors. Per-shard write-rate refusal appears as per-entry `ErrorCode` = `ProvisionedThroughputExceededException` (Smithy: PutRecords error ProvisionedThroughputExceededException, client, httpError 400); the fixed text of that error class is AWS's throughput-exceeded sentence, whose variable parts are the shard id, the stream name and the account — never values from one run.

Actual (emulator): the TLS session is aborted mid-run and botocore raises, with no HTTP status, no error code and no error message at all:

    File "endpoint.py", line 385, in _send
        return self.http_session.send(request)
    File "httpsession.py", line 537, in send
        raise SSLError(endpoint_url=request.url, error=e)
    botocore.exceptions.SSLError: SSL validation failed for https://127.0.0.1:41833/ [SSL: TLSV1_ALERT_DECODE_ERROR] tlsv1 alert decode error (_ssl.c:1010)

In that message the fixed text is botocore's own `SSL validation failed for <endpoint_url> [SSL: TLSV1_ALERT_DECODE_ERROR] tlsv1 alert decode error`; `https://127.0.0.1:41833/` is this run's emulator endpoint (host/port vary per environment and are not part of the requirement). The requirement being violated is that there is a TLS alert at all: `TLSV1_ALERT_DECODE_ERROR` is a fatal alert sent BY the emulator's listener, so the abort originates server-side.

Isolation: the same listener and the same client configuration handshake fine throughout the rest of the suite (the other 206 kinesis checks, and this check's own single-threaded setup: create_stream, stream_exists waiter, add_tags_to_stream, list_shards and the 3-record control put_records all succeed over the same HTTPS endpoint). The failure appears only once 16 threads issue PutRecords concurrently through a client with `max_pool_connections=18` and retries disabled (`retries={'total_max_attempts': 1}`), i.e. only when several TLS connections are handshaking/streaming at once.

## Steps to Reproduce
1. Boot the emulator and point a botocore/boto3 kinesis client at its HTTPS endpoint.
2. `create_stream(StreamName=<unique>, ShardCount=1, StreamModeDetails={'StreamMode': 'PROVISIONED'})`, wait for the `stream_exists` waiter, and confirm `list_shards` reports exactly 1 shard.
3. With a first client, `put_records` a 3-entry batch of 16-byte records on PartitionKey 'k' — this succeeds with `FailedRecordCount` absent/0.
4. Build a SECOND kinesis client with `Config(retries={'total_max_attempts': 1}, max_pool_connections=18)` so retries cannot mask a transport failure and the pool can hold many sockets.
5. Run 16 threads (ThreadPoolExecutor(max_workers=16)) that each call `put_records(StreamName=<stream>, Records=[...500 entries of 16-byte Data, PartitionKey 'k'...])` in a loop for ~45 s.
6. Observe that instead of 200 responses (or a modelled 400), a call fails in the transport with `botocore.exceptions.SSLError: ... [SSL: TLSV1_ALERT_DECODE_ERROR] tlsv1 alert decode error` — a fatal TLS alert from the emulator's listener. The check therefore never reaches its per-shard record-count-quota assertions.
7. Re-run the same loop with 1 thread: no TLS alert, confirming the trigger is concurrent connections rather than the request content.
