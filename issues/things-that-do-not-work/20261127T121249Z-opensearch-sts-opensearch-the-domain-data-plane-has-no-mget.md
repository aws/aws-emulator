Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch__sts.multi-get-answers-found-and-missing-ids-in-request-order opensearch__sts.index-scoped-multi-get-against-missing-index-rejected
Issue-Key: opensearch__sts.multi-get-answers-found-and-missing-ids-in-request-order|run#11|AssertionError|assert status == 200, f"global _mget should answer HTTP 200, got {status}: {global_result}"
Issue-Key: opensearch__sts.index-scoped-multi-get-against-missing-index-rejected|run#11|AssertionError|assert status == 200, f"_mget against an existing index should answer HTTP 200, got {status}: {control_result}"
Issue-Commit: 7c686d71091806786cdd6101a8e36a3d7eadfcb8

# opensearch: the domain data plane has no _mget route — POST /_mget and POST /<index>/_mget answer 400 'no handler found', so multi-get is unavailable

> _Note: this issue was identified against the code base at commit `7c686d71091806786cdd6101a8e36a3d7eadfcb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
OpenSearch domain REST data plane (es:ESHttpPost) — _mget

## Description
Cause 1 of 1: the multi-get endpoint is unrouted, in both its global and its index-scoped form.

AWS: recorded behavior only (the OpenSearch REST multi-get API is not part of the opensearch Smithy model; it reaches the domain through es:ESHttpPost). POST /_mget with `{"docs": [{"_index": …, "_id": …}, …]}` answers HTTP 200 with a `docs` array that is POSITIONAL — one entry per requested id, in REQUEST ORDER, each entry carrying `found: true` with its `_source` or `found: false` for an id that does not exist, so a missing id never fails the call. The index-scoped form POST /<index>/_mget with `{"ids": [...]}` answers HTTP 200 the same way against an EXISTING index, and against an index that does not exist is refused with the index-missing response (an `index_not_found_exception`, not a routing error).
Emulator: HTTP 400 with body `{"error": "no handler found for uri [/_mget] and method [POST]"}` and `{"error": "no handler found for uri [/<index>/_mget] and method [POST]"}`. The fixed text of that envelope is `no handler found for uri [` … `] and method [` … `]`; the path and method are the request's own. This is a routing gap, not a per-request value: the answer does not depend on the body or on index state, so even the existing-index positive control is refused.
Why: batched document retrieval is impossible — a client must fall back to one GET per id, losing the positional/partial-result contract that is the whole point of _mget (found and missing ids reported side by side in one answer). Worse, the 400 is indistinguishable from a malformed request, and the index-scoped missing-index negative class is unreachable: a client cannot tell "this index does not exist" from "this endpoint does not exist", so retry and error-classification logic takes the wrong branch.
Fix: the OpenSearch domain data plane's REST router plus an _mget handler over the document store — register POST /_mget and POST /<index>/_mget, assemble a positional `docs` array with per-id `found` true/false, and refuse the index-scoped form against a missing index with index_not_found_exception.
Note: the open issue 20261111T041122Z-opensearch-ten-domain-rest-data-plane-endpoints-are.md reports the SAME class of routing gap for ten OTHER endpoints (including _msearch) and explicitly scopes itself to those routes; it does not cover _mget and does not carry these checks' replay keys.
Traces: opensearch__sts.multi-get-answers-found-and-missing-ids-in-request-order, opensearch__sts.index-scoped-multi-get-against-missing-index-rejected.

## Evidence
Expected (AWS): HTTP 200 with a positional `docs` array (each entry `found: true` with `_source`, or `found: false`) for both the global and the index-scoped form against an existing index; an index_not_found_exception for the index-scoped form against a missing index.

Actual (emulator), verbatim from the replay:
- 'AssertionError: global _mget should answer HTTP 200, got 400: {\'error\': \'no handler found for uri [/_mget] and method [POST]\'}'
- 'AssertionError: _mget against an existing index should answer HTTP 200, got 400: {\'error\': \'no handler found for uri [/sim2real-mgetx-present/_mget] and method [POST]\'}'

The index name in the second line is a per-run harness value; the requirement is the route and the response shape.

## Steps to Reproduce
1. Create an OpenSearch domain, wait for Processing=false and read DomainStatus.Endpoint.
2. Create an index and index a few documents with known ids.
3. SigV4-sign (service 'es') and send POST <endpoint>/_mget with {"docs":[{"_index":"<index>","_id":"<existing>"},{"_index":"<index>","_id":"<missing>"}]} — expect 200 with a positional docs array (found true then false); observe 400 {"error":"no handler found for uri [/_mget] and method [POST]"}.
4. Send POST <endpoint>/<index>/_mget with {"ids":[...]} against the EXISTING index — expect 200; observe the same 400 envelope with the index-scoped path.
5. Repeat step 4 against an index that does not exist — expect an index_not_found_exception; observe the same 400 routing envelope.
