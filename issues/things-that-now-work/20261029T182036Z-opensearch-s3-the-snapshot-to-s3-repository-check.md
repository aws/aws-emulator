Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch__s3.snapshot-writes-objects-under-the-declared-base-path-and-delete-removes-them
Issue-Key: opensearch__s3.snapshot-writes-objects-under-the-declared-base-path-and-delete-removes-them|run#9|AssertionError|assert status == 200, f"creating index {index!r} returned HTTP {status!r}: {created!r}"
Issue-Commit: 1a0d537a6f45d8cc06f79b3c3e7692a297108dcd

# opensearch__s3: the snapshot-to-S3 repository check cannot run — signed requests to the domain endpoint answer 404 UnknownService 'es' (1 cause)

> _Note: this issue was identified against the code base at commit `1a0d537a6f45d8cc06f79b3c3e7692a297108dcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIndex

## Description
This issue carries 1 cause, in the opensearch/s3 pair (the leading token of the failing check id is opensearch__s3). The open issue 20261024T055421Z-opensearch-domain-data-plane-is-not-served-and.md describes the same unserved data plane for other checks; this check was not marked owned by it.

Cause 1 of 1: the OpenSearch snapshot REST API on the published domain endpoint is not served, so no snapshot repository can be registered and no object is ever written to S3.
AWS: an OpenSearch domain serves the snapshot API on its published endpoint: `PUT /_snapshot/<repo>` registers an S3 repository with {"type": "s3", "settings": {"bucket": <bucket>, "base_path": <path>, "role_arn": <arn>}} and answers 200 {"acknowledged": true}; `PUT /_snapshot/<repo>/<snapshot>?wait_for_completion=true` then writes repository objects into the bucket under the declared base_path (index-N, snap-*.dat, meta-*.dat, indices/...), and `DELETE /_snapshot/<repo>/<snapshot>` removes the snapshot's objects. Index creation and document indexing on the same endpoint are the precondition. Recorded behavior only — the snapshot plane is outside the opensearch Smithy model.
Emulator: every signed request to the published endpoint, starting with the index creation the check needs, answers HTTP 404 with {"__type": "UnknownService", "message": "no emulated service is registered for 'es'"}. The fixed text is `no emulated service is registered for '<signing name>'`; the variable part is the SigV4 signing name from the request's credential scope (`es`). That is the emulator front door reporting that nothing is bound for the domain host — it is not any OpenSearch snapshot-API status or error.
Why: a client cannot register a repository, cannot take or delete a snapshot, and no object ever appears in the S3 bucket under the declared base_path. The entire backup/restore path across opensearch and s3 — including the guarantee that objects stay under base_path and are removed on delete — is unobservable.
Fix: the request router must bind the domain endpoint host to an OpenSearch REST data-plane handler for the `es` signing name, and that handler must implement the `_snapshot` repository/snapshot surface against the S3 bucket named in the repository settings (same component as the opensearch data-plane issue filed in this sweep).
Traces: opensearch__s3.snapshot-writes-objects-under-the-declared-base-path-and-delete-removes-them.

## Evidence
Expected: HTTP 200 from a signed `PUT https://<Endpoint>/<index>` (then from `PUT /_snapshot/<repo>` and `PUT /_snapshot/<repo>/<snapshot>`), followed by S3 objects under the declared base_path.
Actual: `AssertionError: creating index 'sim2real-behav-snapbytes' returned HTTP 404: {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}` — the check never reaches the snapshot API, and no S3 object is written.

## Steps to Reproduce
1. Create an S3 bucket and an IAM role the domain can assume for snapshots.
2. CreateDomain (OpenSearch_2.11, t3.small.search, gp3) and wait for DomainStatus.Endpoint.
3. Send a SigV4-signed (service `es`) `PUT https://<Endpoint>/sim2real-behav-snapbytes` with a mappings/settings body — observe HTTP 404 {"__type":"UnknownService","message":"no emulated service is registered for 'es'"}.
4. The subsequent `PUT /_snapshot/<repo>` (type s3, bucket + base_path + role_arn), `PUT /_snapshot/<repo>/<snapshot>?wait_for_completion=true` and `DELETE /_snapshot/<repo>/<snapshot>` answer the same 404, and ListObjectsV2 on the bucket shows no objects under base_path.
