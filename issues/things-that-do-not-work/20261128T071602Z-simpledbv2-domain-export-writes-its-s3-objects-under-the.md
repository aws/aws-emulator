Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: simpledbv2.export-client-token-reused-for-other-domain-rejected
Issue-Shape-Id: com.amazonaws.simpledbv2#SimpleDBv2
Issue-Key: simpledbv2.export-client-token-reused-for-other-domain-rejected|run#21|AssertionError|raise AssertionError(
Issue-Commit: 794b6570c5e12ac0c4a18da454471fcedcd24ce4

# Domain export writes its S3 objects under the wrong key layout (AmazonSimpleDB/<domain>/<exportArn>/ instead of AWSSimpleDB/<exportId>/<domainName>/)

> _Note: this issue was identified against the code base at commit `794b6570c5e12ac0c4a18da454471fcedcd24ce4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDomainExport

## Description
One root cause.

Cause 1 of 1: the key space a domain export writes its objects into is composed with the wrong service segment and the wrong variable segments, so nothing an export wrote is findable at the key layout AWS uses.
Failing check id: simpledbv2.export-client-token-reused-for-other-domain-rejected

AWS: StartDomainExport (POST /v2/StartDomainExport; input `domainName` required, `s3Bucket` required, optional `s3KeyPrefix` with @length 1..850 documented as "The prefix string to be used to generate the S3 object keys for export artifacts") writes the export's artifacts into the named bucket under a service-fixed layout rooted at the caller's prefix. The recorded layout is:

    <s3KeyPrefix>/AWSSimpleDB/<exportId>/<domainName>/manifest-summary.json
    <s3KeyPrefix>/AWSSimpleDB/<exportId>/<domainName>/manifest-file.json
    <s3KeyPrefix>/AWSSimpleDB/<exportId>/<domainName>/<data file(s) named by manifest-file.json>

where `<exportId>` is the last segment of the minted `exportArn` (arn:aws:sdb:<region>:<account>:domain/<domainName>/export/<exportId>) and `<domainName>` is the exported domain. The fixed text of the layout is the literal segment `AWSSimpleDB` and the file names `manifest-summary.json` / `manifest-file.json`; the variable parts are the caller's prefix, the export id and the domain name. manifest-summary.json carries at least `domainName` and `itemsCount` (the domain's real item count), and manifest-file.json carries one entry per data object with the member `dataFileS3Key`. Recorded behavior only for the layout — the Smithy model constrains `s3KeyPrefix` but does not state the key template; both behavioral checks in this suite are recorded against the above. A consequence of that recording: GetExport.exportManifest does NOT resolve to any object the export wrote (it names a different service segment and swaps the domain/id order), so on AWS a client must locate the manifest by listing the prefix, not by trusting that field. Both calls are a plain HTTP 200 on AWS and no error code is involved.

Emulator: the export writes under `<s3KeyPrefix>/AmazonSimpleDB/<domainName>/<full exportArn>/` — the service segment is spelled `AmazonSimpleDB` instead of `AWSSimpleDB`, and instead of `<exportId>/<domainName>` it interpolates the domain name followed by the ENTIRE export ARN (so the key re-embeds `domain/<domainName>/export/<exportId>`, colons and all). The data object is written at `.../data/items.json` and the manifest at `.../manifest-summary.json`; no manifest-file.json is written, the manifest's own members are spelled `itemCount` and `dataFileS3Keys` rather than `itemsCount`/`dataFileS3Key` entries in a separate file, and because the record reports this same composed key as `exportManifest`, that field resolves to a real object where on AWS it resolves to none. Replay evidence: after two successful exports the bucket held only keys of the form `tokens/AmazonSimpleDB/<domain>/arn:aws:sdb:us-west-2:000000000000:domain/<domain>/export/<exportId>/data/items.json` and `.../manifest-summary.json`, and the check's probe of the AWS layout prefix `tokens/AWSSimpleDB/<exportId>/<domain>/` returned nothing.

Why: a client calibrated against AWS lists or heads `<prefix>/AWSSimpleDB/<exportId>/<domainName>/...` to consume an export, and parses manifest-file.json to learn which data objects to read. Against the emulator that prefix is empty, so the consumer concludes the export produced no data even though GetExport reports SUCCEEDED; conversely code written against the emulator learns a key template and a manifest schema that do not exist in production, and learns to trust GetExport.exportManifest, which in production points at nothing. Any pipeline that keys bucket policies, lifecycle rules, notifications or prefix-scoped IAM on the real `AWSSimpleDB/<exportId>/<domainName>/` space silently sees zero objects.

Fix: the export key composition and the export-run object set for simpledbv2 — the state's export key-space helper and the run that writes the objects (the operation's state layer, not its serializer). The fixed segment must be the one AWS uses and the variable segments must be the export id then the domain name, in that order, with the caller's `s3KeyPrefix` reproduced verbatim as the root; the objects written must be the manifest summary, the manifest file listing the data objects, and the data objects it names. Keep GetExport.exportManifest's divergence from the written keys that AWS exhibits, i.e. do not derive the reported field from the corrected write key.

Traces: simpledbv2.export-client-token-reused-for-other-domain-rejected (two exports into one bucket; after both reached SUCCEEDED the AWS-layout prefix held nothing). The sibling check simpledbv2.export-lifecycle-lands-verified-data-in-s3 asserts the same `/AWSSimpleDB/` layout and the manifest-summary/manifest-file contents, but it aborts earlier on the itemsCount defect already filed as 20261124T145339Z-simpledbv2-getexport-itemscount-returns-the-domain-s-real-item.md (that issue covers only itemsCount; it does not cover this key layout), so re-replay it too after this fix. File/line citations for the composition: services/service-com-amazonaws-simpledbv2-simpledbv2/src/state.rs:143 (EXPORT_KEY_SERVICE_SEGMENT = "AmazonSimpleDB"), state.rs:230-250 (export_key_prefix / manifest_key), src/state/domain_export_run.rs:155-188 (export_objects).

## Evidence
Check: simpledbv2.export-client-token-reused-for-other-domain-rejected (mismatch)

Setup (abbreviated): create one S3 bucket; sdb.create_domain for two domains A and B, one PutAttributes each; v2.start_domain_export(clientToken=T, domainName=A, s3Bucket=<bucket>, s3KeyPrefix="tokens") -> export ARN; reuse of T for B correctly refused with ConflictException/"Idempotent request failed due to parameter mismatch."; replay of T for A correctly returns the same ARN; a fresh token starts an export of B. Both exports polled to SUCCEEDED. Then, for each export, list the AWS layout prefix tokens/AWSSimpleDB/<exportId>/<domainName>/.

Expected (recorded AWS): at least one object under tokens/AWSSimpleDB/<exportId>/<domainName>/ — manifest-summary.json, manifest-file.json and the data object(s) manifest-file.json names. The literal segment AWSSimpleDB and the file names are the fixed text; <exportId>, <domainName>, the bucket and the prefix are values from the request/state.

Actual (emulator): that prefix is empty. The bucket held only:
  tokens/AmazonSimpleDB/<domainA>/arn:aws:sdb:us-west-2:000000000000:domain/<domainA>/export/<exportIdA>/data/items.json
  tokens/AmazonSimpleDB/<domainA>/arn:aws:sdb:us-west-2:000000000000:domain/<domainA>/export/<exportIdA>/manifest-summary.json
  tokens/AmazonSimpleDB/<domainB>/arn:aws:sdb:us-west-2:000000000000:domain/<domainB>/export/<exportIdB>/data/items.json
  tokens/AmazonSimpleDB/<domainB>/arn:aws:sdb:us-west-2:000000000000:domain/<domainB>/export/<exportIdB>/manifest-summary.json

Failure text (variable values are this run's only):
  File "check_simpledbv2_export_client_token_reused_for_other_domain_rejected.py", line 185, in run
    raise AssertionError(...)
  AssertionError: export of sim2real-behav-sdbtok-a-fff1fb6759bd wrote nothing under 'tokens/AWSSimpleDB/0b9a424b-de1d-427e-a801-7ccb2b04a7b1/sim2real-behav-sdbtok-a-fff1fb6759bd/'; the bucket holds ['tokens/AmazonSimpleDB/sim2real-behav-sdbtok-a-fff1fb6759bd/arn:aws:sdb:us-west-2:000000000000:domain/sim2real-behav-sdbtok-a-fff1fb6759bd/export/0b9a424b-de1d-427e-a801-7ccb2b04a7b1/data/items.json', ...]

Not requirements: the generated bucket/domain names, the account id 000000000000, the region us-west-2, the export uuids and timestamps. The requirement is the key TEMPLATE: <s3KeyPrefix>/AWSSimpleDB/<exportId>/<domainName>/<artifact>.

HTTP status and error code: none diverge. StartDomainExport and GetExport both answer 200 in AWS and in the emulator; the divergence is in the S3 object keys the export writes and the artifact set it writes there.

## Steps to Reproduce
1. Boot the emulator and point botocore at it with clients for `sdb`, `simpledbv2` and `s3`.
2. s3.create_bucket(Bucket=B) (with CreateBucketConfiguration.LocationConstraint outside us-east-1).
3. sdb.create_domain(DomainName=D); sdb.put_attributes(DomainName=D, ItemName="row-1", Attributes=[{"Name":"owner","Value":D}]).
4. r = simpledbv2.start_domain_export(domainName=D, s3Bucket=B, s3KeyPrefix="tokens"); arn = r["exportArn"]; export_id = arn.rsplit("/",1)[-1].
5. Poll simpledbv2.get_export(exportArn=arn) until exportStatus leaves PENDING/IN_PROGRESS; it reaches SUCCEEDED.
6. s3.list_objects_v2(Bucket=B, Prefix=f"tokens/AWSSimpleDB/{export_id}/{D}/") -> AWS returns the export's manifest-summary.json, manifest-file.json and data object(s); the emulator returns no Contents.
7. s3.list_objects_v2(Bucket=B) -> the emulator's keys are tokens/AmazonSimpleDB/<D>/<full exportArn>/data/items.json and .../manifest-summary.json.
8. Equivalent single command: replay the behavioral check simpledbv2.export-client-token-reused-for-other-domain-rejected.
