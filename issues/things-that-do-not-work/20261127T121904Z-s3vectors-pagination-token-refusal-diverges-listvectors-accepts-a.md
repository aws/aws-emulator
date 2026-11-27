Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3vectors.list-vectors-next-token-from-a-sibling-index-rejected s3vectors.query-vectors-next-token-from-a-sibling-index-rejected s3vectors.query-vectors-unissued-next-token-rejected
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors.list-vectors-next-token-from-a-sibling-index-rejected|_refusal#12|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {got!r}")
Issue-Key: s3vectors.query-vectors-next-token-from-a-sibling-index-rejected|run#15|AssertionError|assert got == ("ValidationException", "Invalid page token"), (
Issue-Key: s3vectors.query-vectors-unissued-next-token-rejected|run#18|AssertionError|assert got == ("ValidationException", "Invalid page token"), (
Issue-Commit: 33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5

# s3vectors pagination-token refusal diverges: ListVectors accepts a token minted for a sibling index (200 instead of 400), and QueryVectors refuses with its own message instead of AWS's "Invalid page token"

> _Note: this issue was identified against the code base at commit `33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListVectors

## Description
TWO net-new root causes in the s3vectors data-plane pagination-token path, both about how a nextToken that this listing/query did not mint is handled. Both are grounded in failing behavioral checks of service s3vectors; neither was marked ALREADY FILED nor OWNED by an open issue. Both must be closed before this issue is submitted.

Related open issue, named for context and NOT covering these checks: 20261103T033818Z-s3vectors-read-paths-serve-internal-values-on-the.md (a WIRE finding whose Causes 1-2 report the ListVectors nextToken being the raw vector key and a forged token being answered 200, keyed on fixture s3vectors-vbi-list-vectors-paging#task#4/#task#8). It carries no replay key for any check below, and Cause 2 here (QueryVectors' refusal message) is not mentioned by it at all.

Model consulted: services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/com-amazonaws-s3vectors-s3vectors.json (S3Vectors, restJson1).

=== Cause 1 of 2: ListVectors accepts a nextToken minted for a DIFFERENT index and serves a page (HTTP 200) where AWS refuses with 400 ValidationException "Invalid page token" ===
AWS: Smithy models ListVectors as @paginated (inputToken nextToken, outputToken nextToken, items vectors, pageSize maxResults), with ListVectorsNextToken a string of @length 1..2048, and models ValidationException on the operation (errorKind client, httpError 400, required `message`). The model does not describe token scoping, so: recorded behavior only — a nextToken AWS did not mint for the listing it is replayed against (here, the token a sibling index in the same vector bucket issued) is refused with HTTP 400 ValidationException whose `message` is the fixed sentence "Invalid page token". That sentence has NO variable part: the rejected token is not echoed, and the x-amzn-errortype header names ValidationException.
Emulator: the call succeeds with HTTP 200 and a normal page body. In the replayed check, ListVectors on index "sibling-idx" with the token "issuer-idx" had just issued returned 200 with vectors [{'key': 'sibling-0'}] and nextToken 'sibling-0' — i.e. the sibling's own FIRST page. The token is interpreted as a bare key cursor (services/.../src/ops/list_vectors.rs hands input.next_token straight to crate::state::page_of), and a key that is not present in the listing being paged silently positions the page at the start instead of being refused. Nothing binds the token to the index, segment or listing that minted it.
Why: a request AWS REFUSES is SERVED, and served with a mispositioned page. A paginating client that caches, crosses or corrupts tokens (two parallel listings over two indexes in the same bucket, a retry that reuses the wrong cursor, a truncated token) silently restarts the walk or skips records while believing it enumerated the index — the duplicate/omission is invisible because the response looks healthy. The refusal path a real client must handle is never exercised against the emulator.
Fix: ListVectors' pagination-input validation — the operation's handler (services/service-com-amazonaws-s3vectors-s3vectors/src/ops/list_vectors.rs) and the cursor decode/mint helper it calls in crate::state (page_of). Decode-or-refuse: a token not minted by THIS listing (same index, same segmentation) must raise ValidationException with AWS's fixed sentence rather than defaulting the cursor to position 0.
Traces: s3vectors.list-vectors-next-token-from-a-sibling-index-rejected.

=== Cause 2 of 2: QueryVectors refuses an unissued nextToken with the emulator's own sentence instead of AWS's "Invalid page token" ===
AWS: Smithy models QueryVectors as @paginated (inputToken/outputToken nextToken, items vectors) with QueryVectorsNextToken @length 1..4096, and models ValidationException (errorKind client, httpError 400, required `message`). Recorded behavior only for the text: a nextToken the service did not issue for this query — whether it was minted for another index or never minted at all — is refused with HTTP 400 ValidationException and `message` exactly "Invalid page token". The entire sentence is FIXED text; there is no variable part, and in particular the offending token value is NOT part of the message.
Emulator: the status (400) and the error code (ValidationException) are right, but the message is the emulator's own sentence, which echoes the rejected token: "`nextToken` is not a pagination token this service issued: <token>" — observed as '`nextToken` is not a pagination token this service issued: deep-099' for a sibling index's token and '`nextToken` is not a pagination token this service issued: cGFnZS10d28tb2YtYS1xdWVyeS1uZXZlci1ydW4=' for a token never issued. (Those token values are the per-run inputs of the two checks, not part of any requirement; what diverges is the whole sentence, which must be the fixed "Invalid page token".) The text is constructed in the handler at services/service-com-amazonaws-s3vectors-s3vectors/src/ops/query_vectors.rs (the `None =>` arm of the next_token match, around lines 318-331), where the token is also matched against vector KEYS of the current ranking rather than decoded as a minted token.
Why: the message is part of the contract a client sees. Code that matches on the refusal sentence (error-classification middleware, retry/resume logic that distinguishes an invalid page token from other validation failures, log assertions, parity tests) does not recognise the emulator's wording and treats a known, recoverable refusal as an unknown validation error. Echoing the submitted token additionally discloses an internal cursor form (a vector key) AWS never puts in the message, teaching clients a token format AWS does not offer.
Fix: QueryVectors' pagination-input validation and its refusal construction — the operation's handler (services/service-com-amazonaws-s3vectors-s3vectors/src/ops/query_vectors.rs) together with the LookupError::invalid refusal text it builds; emit AWS's fixed sentence for this error class and stop interpolating the rejected token.
Traces: s3vectors.query-vectors-next-token-from-a-sibling-index-rejected, s3vectors.query-vectors-unissued-next-token-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 139/151 passed, 9 mismatched, 3 errored, 0 skipped, bootstrap_signal false. Service s3vectors scored 88/92 with 4 failures; one of those (update-index-mode-classic-rejected-in-new-bucket) is ALREADY FILED as an unimplemented UpdateIndexMode stub and is not part of this issue. The three failures below are the surviving ones.

Cause 1 — [mismatch] s3vectors.list-vectors-next-token-from-a-sibling-index-rejected:
  Setup: CreateVectorBucket, then two indexes in it ("issuer-idx", "sibling-idx", dataType float32, dimension 2, distanceMetric euclidean), each seeded with several vectors; ListVectors(indexName="issuer-idx", maxResults=1) returns one key plus a nextToken.
  Expected: ListVectors(indexName="sibling-idx", maxResults=1, nextToken=<issuer's token>) -> HTTP 400 ('ValidationException', 'Invalid page token').
  Actual: HTTP 200, body {'nextToken': 'sibling-0', 'vectors': [{'key': 'sibling-0'}]}. Check assertion: "the call was expected to be refused but returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'nextToken': 'sibling-0', 'vectors': [{'key': 'sibling-0'}]}".

Cause 2 — [mismatch] s3vectors.query-vectors-next-token-from-a-sibling-index-rejected:
  Setup: one vector bucket with a "deep-idx" (more than one page of results) and a "shallow-idx"; QueryVectors on deep-idx returns a first page plus a nextToken.
  Expected: QueryVectors(indexName="shallow-idx", nextToken=<deep-idx's token>) -> ('ValidationException', 'Invalid page token').
  Actual: ('ValidationException', '`nextToken` is not a pagination token this service issued: deep-099').

Cause 2 — [mismatch] s3vectors.query-vectors-unissued-next-token-rejected:
  Expected: QueryVectors with a nextToken the service never issued -> ('ValidationException', 'Invalid page token').
  Actual: ('ValidationException', '`nextToken` is not a pagination token this service issued: cGFnZS10d28tb2YtYS1xdWVyeS1uZXZlci1ydW4=').

Per-run values in the above (bucket/index names generated by the harness, the key 'sibling-0', 'deep-099', the base64 literal, request ids and timestamps) are examples, not requirements. The requirements are: HTTP 400 + ValidationException + the fixed message "Invalid page token" for a token the service did not mint, on BOTH ListVectors and QueryVectors.

## Steps to Reproduce
0. git_clone, then run_behavioral_suite and re-replay the three checks named in trace_ids: s3vectors.list-vectors-next-token-from-a-sibling-index-rejected, s3vectors.query-vectors-next-token-from-a-sibling-index-rejected, s3vectors.query-vectors-unissued-next-token-rejected. All three must pass before submitting.
1. Shared setup: CreateVectorBucket(<fresh lower-case name>); CreateIndex twice in it (dataType=float32, dimension=2, distanceMetric=euclidean) as "issuer-idx" and "sibling-idx"; PutVectors several distinct keys into each.
2. Cause 1: POST /ListVectors {vectorBucketName, indexName: "issuer-idx", maxResults: 1} -> note the nextToken. Then POST /ListVectors {vectorBucketName, indexName: "sibling-idx", maxResults: 1, nextToken: <that token>}. Observe HTTP 200 with the sibling's first page; expected HTTP 400 ValidationException with message "Invalid page token" (and the x-amzn-errortype header naming ValidationException).
3. Cause 1 regression guard: re-issue ListVectors on "issuer-idx" with the same token after the refusal — it must still resume the issuer's walk and enumerate every remaining key exactly once.
4. Cause 2 (sibling token): seed one index with more results than one QueryVectors page holds and another with few; POST /QueryVectors on the deep index (topK large enough to page) -> note the nextToken; POST /QueryVectors on the shallow index with that nextToken. Observe message '`nextToken` is not a pagination token this service issued: <token>'; expected exactly "Invalid page token".
5. Cause 2 (unissued token): POST /QueryVectors with nextToken set to any string the service never issued (e.g. a hand-written base64 literal) -> same wrong message; expected exactly "Invalid page token".
6. Cause 2 regression guard: the issuer's own token must still be honoured after the refusal and return the remaining ranked results with no further token.
