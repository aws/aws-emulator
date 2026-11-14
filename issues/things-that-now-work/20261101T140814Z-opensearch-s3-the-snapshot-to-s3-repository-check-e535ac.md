Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch__s3.snapshot-writes-objects-under-the-declared-base-path-and-delete-removes-them
Issue-Key: opensearch__s3.snapshot-writes-objects-under-the-declared-base-path-and-delete-removes-them|_bulk_load#11|AssertionError|assert status == 200, f"_bulk returned HTTP {status!r}, expected 200: {body!r}"
Issue-Commit: 67f5fadccaf81bef3549b8820a70ca0b0ed0533e

# opensearch__s3: the snapshot-to-S3 repository check cannot load documents — POST /_bulk on the domain endpoint answers 405 with the index route's method list (1 cause)

> _Note: this issue was identified against the code base at commit `67f5fadccaf81bef3549b8820a70ca0b0ed0533e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DomainRestDataPlane

## Description
Cause 1 of 1 — POST /_bulk is not served on the domain REST data plane; the path is resolved as an INDEX name, so the snapshot/restore-to-S3 flow can never seed the documents it snapshots.
AWS: Amazon OpenSearch Service serves the bulk API on the domain endpoint: `POST /_bulk` with a newline-delimited action/source body answers HTTP 200 `{"took":<n>,"errors":<bool>,"items":[...]}`. Only after documents exist does `PUT /_snapshot/<repo>` (an `s3` repository with a `base_path`) and `PUT /_snapshot/<repo>/<snap>` write the documented BlobStoreRepository layout under that base path in the S3 bucket. Recorded behavior only for the emulator's wording.
Emulator: the signed `POST <endpoint>/_bulk` answers HTTP 405 `{'error': 'Incorrect HTTP method for uri [/_bulk] and method [POST], allowed: [PUT, GET, HEAD, DELETE]'}`. Fixed text: "Incorrect HTTP method for uri [", "] and method [", "], allowed: ["; variable parts: the URI, the method, and the allowed-method list — and the list returned is the INDEX route's, which is the tell that `/_bulk` was matched as `/{index}`.
Why: a client that backs a domain up to its own S3 bucket cannot get any data into the domain to snapshot, so the whole cross-service snapshot/restore path — repository registration, the objects written under `base_path`, and the `snapshot_missing_exception` / `snapshot_restore_exception` refusals on a deleted snapshot — is unreachable. The data plane now answers (it no longer returns 404 UnknownService 'es' as open issue 20261029T182036Z-opensearch-s3-the-snapshot-to-s3-repository-check.md records), so this is a net-new refusal of the surface that answers.
Fix: the domain REST data-plane router in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/service/domain_rest.rs — reserve `_`-prefixed first path segments from index-name resolution and register the bulk handler.
Traces: opensearch__s3.snapshot-writes-objects-under-the-declared-base-path-and-delete-removes-them

## Evidence
Expected HTTP 200 from POST <domain endpoint>/_bulk; got HTTP 405 {'error': 'Incorrect HTTP method for uri [/_bulk] and method [POST], allowed: [PUT, GET, HEAD, DELETE]'}. The allowed-method list is the index route's, not the bulk endpoint's.

## Steps to Reproduce
1. CreateBucket in S3 for the repository.
2. CreateDomain (OpenSearch_2.11, one t3.small.search node, gp3/10) with an AccessPolicies document allowing es:* for the caller; wait for it to be active and read DomainStatus.Endpoint.
3. SigV4-sign and send POST <endpoint>/_bulk with an NDJSON body of {"index":{"_index":"<idx>","_id":"<id>"}} / source pairs -> 405 instead of 200, so no documents are loaded.
4. (Blocked by step 3) PUT <endpoint>/_snapshot/<repo> with an s3 repository naming the bucket and a base_path, PUT <endpoint>/_snapshot/<repo>/<snap>?wait_for_completion=true, and list the bucket under base_path.
