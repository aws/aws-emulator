Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.osq-deep-opensearch-2-async-delete-by-query-task-completes-with-exact-subset
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.osq-deep-opensearch-2-async-delete-by-query-task-completes-with-exact-subset|run#17|AssertionError|assert isinstance(task_id, str) and ":" in task_id and set(accepted) == {"task"}, (
Issue-Commit: badd561fcb6c071720040204b8eb8e1d859e17d8

# opensearch: POST /<index>/_delete_by_query ignores wait_for_completion=false — it runs synchronously and returns the bulk-by-query report instead of a '<node>:<id>' task handle, and registers no task under /_tasks

> _Note: this issue was identified against the code base at commit `badd561fcb6c071720040204b8eb8e1d859e17d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
es:ESHttpPost (OpenSearch domain REST data plane: POST /{index}/_delete_by_query, GET /_tasks/{task_id})

## Description
One net-new root cause on the OpenSearch domain REST data plane.

Context: open issue 20261111T041122Z-opensearch-ten-domain-rest-data-plane-endpoints-are.md, Cause 6 of 10, filed `POST /<index>/_delete_by_query` as UNROUTED (HTTP 400 `no handler found for uri [...]`). That route now answers, so that symptom is gone, but the handler that answers it is not the asynchronous API: it ignores the request's query string. The replay does not mark this check as owned by that issue, and no other open issue names the `wait_for_completion` / `_tasks` behaviour, so this is a net-new cause rather than a repeat of that one.

Cause 1 of 1: `_delete_by_query?wait_for_completion=false` executes inline and answers with the finished bulk-by-query report, so no task is minted and `/_tasks/<task_id>` can never be polled.

AWS:      recorded behavior only — the OpenSearch domain REST data plane is not part of the `opensearch` Smithy model (service AmazonOpenSearchService, arnNamespace `es`, 96 modeled control-plane operations only); these calls reach the domain through `es:ESHttpPost` / `es:ESHttpGet` against DomainStatus.Endpoint. The recording shows that when `wait_for_completion=false` is present on `POST /{index}/_delete_by_query`, the endpoint answers HTTP 200 with a body whose ONLY member is `task`, a string of the form `<node_id>:<task_number>` (the fixed part is the `:` separator; both halves are values the cluster assigns). The delete then proceeds in the background and is observed through `GET /_tasks/{task_id}`, which answers HTTP 200 with `completed` (false while running, true once finished), a `task` object carrying `action` = `indices:data/write/delete/byquery`, `node` and `id` that recompose the polled handle, and `status.deleted` which only ever increases across polls; once `completed` is true the same body additionally carries `response`, the bulk-by-query report (`total`, `deleted`, `batches`, `version_conflicts`, `noops`, `retries`, `failures`, `throttled_millis`, `requests_per_second`). The report is scoped to the MATCHED subset, not the index: `total` and `deleted` equal the number of documents the query matched, `batches` is `ceil(matched / scroll_size)` for the requested `scroll_size`, and `requests_per_second` echoes the requested `requests_per_second` as a float (it is `-1.0` only when the caller sent no throttle).
Emulator: the `wait_for_completion=false` query parameter is ignored. The call runs the deletion inline and answers HTTP 200 with the completed bulk-by-query report directly as the top-level body — observed verbatim: `{'took': 0, 'timed_out': False, 'total': 6, 'deleted': 6, 'batches': 1, 'version_conflicts': 0, 'noops': 0, 'retries': {'bulk': 0, 'search': 0}, 'throttled_millis': 0, 'requests_per_second': -1.0, 'throttled_until_millis': 0, 'failures': []}`. There is no `task` member, so no handle exists and `GET /_tasks/<handle>` is unreachable. The same body shows the other query parameters are ignored too: `requests_per_second` is reported as `-1.0` although the request asked for `1`, and `batches` is `1` although `scroll_size=2` over 6 matched documents requires 3 batches.
Why:      a client that issues the asynchronous form reads `response['task']` to get its handle; against the emulator that member is absent, so the client raises a KeyError/None instead of receiving a handle, and every progress-polling, cancellation and throttling workflow built on `/_tasks` is unreachable. A client that deliberately offloads a large query-scoped delete to the background instead gets a request that blocks until the whole delete is done — the exact behaviour `wait_for_completion=false` exists to avoid — and a client that throttles with `requests_per_second` is told its throttle was not applied (`-1.0`), so it cannot reason about how long the delete will take or confirm it was rate limited. The `batches` figure likewise misreports how the scroll was paged.
Fix:      the OpenSearch domain data plane's `_delete_by_query` handler — parse and honour the `wait_for_completion`, `scroll_size` and `requests_per_second` query parameters; when `wait_for_completion=false`, register a background task record (action `indices:data/write/delete/byquery`, node/id, incrementally updated `status.deleted`) and return ONLY `{"task": "<node>:<id>"}`; and add/extend the `/_tasks/{task_id}` handler so it serves `completed`, the `task` object and, once finished, the `response` report. The report's `batches` and `requests_per_second` must be derived from the request's own parameters rather than defaulted.
Traces:   opensearch.osq-deep-opensearch-2-async-delete-by-query-task-completes-with-exact-subset.

## Evidence
Check: opensearch.osq-deep-opensearch-2-async-delete-by-query-task-completes-with-exact-subset (verdict: mismatch)

Request replayed against the created domain's published DomainStatus.Endpoint, SigV4-signed for service `es`:

    POST /<index>/_delete_by_query?wait_for_completion=false&refresh=true&scroll_size=2&requests_per_second=1
    {"query": {"term": {"kind": "purge"}}}

Index state: 10 documents, 6 of which match the term query.

Expected (AWS, recorded): HTTP 200 with a body whose only member is the task handle —
    {"task": "<node_id>:<task_number>"}
followed by GET /_tasks/<node_id>:<task_number> answering HTTP 200 with `completed: false` on at least one poll (the delete was throttled to 1 doc/s over 6 documents), a monotonically increasing `status.deleted`, `task.action` == "indices:data/write/delete/byquery", `task.node`/`task.id` recomposing the polled handle, and once completed a `response` with deleted == 6, total == 6, batches == 3 (ceil(6/2) for scroll_size=2), version_conflicts == 0, failures == [] and requests_per_second == 1.0.

Actual (emulator), verbatim from the replay:
    AssertionError: wait_for_completion=false must return ONLY a '<node>:<id>' task handle, got
    {'took': 0, 'timed_out': False, 'total': 6, 'deleted': 6, 'batches': 1, 'version_conflicts': 0,
     'noops': 0, 'retries': {'bulk': 0, 'search': 0}, 'throttled_millis': 0,
     'requests_per_second': -1.0, 'throttled_until_millis': 0, 'failures': []}

Three divergences visible in that single body: (a) no `task` member — the completed report is returned as the top-level body, so the asynchronous contract is not honoured and no `/_tasks` handle exists; (b) `requests_per_second` is -1.0 (the no-throttle sentinel) although the request asked for 1; (c) `batches` is 1 although scroll_size=2 over 6 matched documents pages in 3 batches.

Note: the document ids (d0..d9), index name and domain name in this replay are harness-generated per-run values and are not part of the requirement; the requirement is the shape (`task` only), the `<node>:<id>` form of the handle, and that `batches`/`requests_per_second` are derived from the request's own scroll_size/requests_per_second.

## Steps to Reproduce
1. CreateDomain with the minimal single-node topology (InstanceType t3.small.search, InstanceCount 1, DedicatedMasterEnabled false, ZoneAwarenessEnabled false, EBSOptions gp3 10 GiB), EngineVersion OpenSearch_2.19, and an AccessPolicies document allowing this account's root principal es:* on the domain. Wait for Processing=false and read DomainStatus.Endpoint.
2. Poll a SigV4-signed GET / on that endpoint until it answers 200 (the data-plane authorizer lags the control plane).
3. PUT /<index> with settings {number_of_shards: 1, number_of_replicas: 0} and mappings {kind: keyword, n: integer}.
4. POST /_bulk?refresh=true indexing 10 documents, 6 with kind="purge" and 4 with kind="keep". Confirm GET /<index>/_count reports 10.
5. POST /<index>/_delete_by_query?wait_for_completion=false&refresh=true&scroll_size=2&requests_per_second=1 with body {"query":{"term":{"kind":"purge"}}}.
   Expected: HTTP 200 and a body that is exactly {"task": "<node>:<id>"}.
   Observed: HTTP 200 and the completed bulk-by-query report as the top-level body (no `task` member, requests_per_second -1.0, batches 1).
6. Take the value of `task` from step 5 and GET /_tasks/<task>. Expected: HTTP 200 with `completed`, `task.action` = indices:data/write/delete/byquery, increasing `task.status.deleted`, and a `response` report once completed. Against the emulator step 6 cannot be issued at all because step 5 returned no handle.
