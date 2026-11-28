Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.single-put-object-of-thirty-two-mib-round-trips-exactly s3.bucket-metadata-table-v1-surface-closed s3.upload-part-of-twenty-mib-completes-and-reads-back-exactly
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutObject|413
Issue-Key: s3|UploadPart|413
Issue-Key: s3.bucket-metadata-table-v1-surface-closed|run#7|AssertionError|assert get_code == "V1APIsNotAllowed", (
Issue-Commit: bfa988ebb91b850d9b371924f94981e6ac902256

# s3: 2 causes left open after 20261117T143723Z — a 32 MiB PutObject is refused 413 by the core's request-body cap, and the V1 metadata-table surface answers NotImplemented because AWS's sentence for V1APIsNotAllowed is unsourced

## Operation
PutObject

## Description
These are the two causes of 20261117T143723Z-s3-8-net-new-data-plane-and-validation.md (now in
things-that-now-work, with a Resolution section) that its fix did NOT close. They are filed
here so the emulator's state is recorded rather than implied by that report. The third cause
that report leaves open — a data-plane request carrying an `Origin` header is refused 403 — is
already owned by 20261117T213222Z-s3-3-net-new-browser-edge-refusal-causes.md and is not
re-filed here; see the note at the end, which corrects that issue's diagnosis.

Cause 1 of 2: a single PutObject of 32 MiB is refused with a bare HTTP 413 and no S3 error
envelope.
AWS: Smithy (s3) PutObject takes a streaming Body with no length constraint — a single PUT
carries up to 5 GiB — so a 32 MiB body must be stored and read back byte-exact, and PutObject
models no capacity error at all.
Emulator: `botocore.exceptions.ClientError: An error occurred (413) when calling the PutObject
operation: Request Entity Too Large`, with no `<Error><Code>` for a client to branch on. The
refusal is NOT in the S3 crate: that store's own budgets are 64 MiB per bucket and 128 MiB in
total, both above this write. It is `MAX_PRODUCTION_BODY_BYTES = 16 * 1024 * 1024` in
`vendor/server/src/lib.rs`, applied by `read_production_body` on the production serve path
before any service sees the request, so no service crate can admit the body or word the
refusal. The const is private and has no override.
Why: every ordinary-sized object (logs, images, archives) above 16 MiB is refused with a status
AWS never returns for it, and the answer carries no S3 error code.
Fix: the core's request-body cap — raise it (or make it configurable) so a service's own
documented limits are what bound a write, and render an over-cap refusal in the claiming
service's envelope rather than as a bare status. Same vendor cap as cause 4 of
20261111T165323Z-s3-emulator-invented-storage-ceilings-refuse-valid-writes.md (a 20 MiB
UploadPart): that report's other three causes are fixed and it has moved to things-that-now-work
with a Resolution section, so its cause-4 check
(`s3.upload-part-of-twenty-mib-completes-and-reads-back-exactly`, `Issue-Key: s3|UploadPart|413`)
is carried here from now on — it is the same cap, measured the same way, and no service crate
reaches it.
Traces: s3.single-put-object-of-thirty-two-mib-round-trips-exactly,
s3.upload-part-of-twenty-mib-completes-and-reads-back-exactly.

Cause 2 of 2: the V1 S3 Metadata table operations answer `NotImplemented` where AWS answers
`V1APIsNotAllowed`, and the sentence that refusal carries cannot be sourced from the material
in this repository.
AWS: `CreateBucketMetadataTableConfiguration`, `GetBucketMetadataTableConfiguration` and
`DeleteBucketMetadataTableConfiguration` are the superseded V1 surface ("make sure that you
update your processes to use the new V2 API operations (CreateBucketMetadataConfiguration,
GetBucketMetadataConfiguration, and DeleteBucketMetadataConfiguration) instead of the V1 API
operations", each operation's own `@documentation`). Where that surface is closed AWS refuses a
call on it with the error code `V1APIsNotAllowed` — a modelled refusal that tells a client to
migrate — rather than reporting a missing endpoint.
Emulator: all three operations are still scaffolds answering HTTP 501 `NotImplemented`.
What is missing to fix it: the error code and a status are known from the recorded evidence, but
an error message is as much a requirement as its code, and AWS's sentence for `V1APIsNotAllowed`
is in neither the Smithy model (which declares no error shape for the code, and whose
`@documentation` for the three operations words only the deprecation and the unrelated HTTP 405
for a V1 call against a V2 configuration) nor anything else this repository holds; the recorded
check compares the message against a value its own harness builds. A previous attempt shipped
the right code with a sentence of the service's own words and was refused review for publishing
a wording AWS never sends, so the surface was left scaffolded deliberately. A fixer holding
AWS's sentence can close this in one edit per operation.
`services/service-com-amazonaws-s3-amazons3/tests/all/metadata_table_v1_surface.rs` already
pins the part of the contract that does not depend on which refusal it is: no V1 call is served,
the three answer alike whichever of them and whichever bucket is addressed, and none disturbs
the bucket's V2 `?metadataConfiguration` state.
Traces: s3.bucket-metadata-table-v1-surface-closed.

## Evidence
Cause 1 — Expected: PutObject of 33554432 bytes answers 200 and GetObject returns the bytes
exactly. Actual: HTTP 413 with the reason phrase `Request Entity Too Large` as the whole
response (no `<Error>` document), produced before the S3 service is reached.
Cause 2 — Expected error code `V1APIsNotAllowed`. Actual `NotImplemented` (HTTP 501) from the
operation's scaffold, for all three V1 operations.

## Steps to Reproduce
Cause 1: CreateBucket, then PutObject a 32 MiB body in a single call; observe the bare 413. A
body of 16 MiB or less is served, which is the cap.
Cause 2: call GetBucketMetadataTableConfiguration (or the V1 create/delete) on any bucket;
observe HTTP 501 `NotImplemented`.

## Note on the third open cause of 20261117T143723Z (not re-filed here)
A data-plane request that carries an `Origin` header is refused 403 before any handler runs,
which is what leaves every CORS check of that report failing on its real request (the preflight
itself is answered correctly now). That family is owned by
20261117T213222Z-s3-3-net-new-browser-edge-refusal-causes.md, whose diagnosis should be
corrected: it attributes the refusal to the S3 service computing a different SigV4 signature
than botocore. Measured with an otherwise identical pair of requests through the router,
`GET /<bucket>/<key>` answers 200 with the object's bytes while the SAME header-signed request
carrying `Origin: https://reader.example.org` answers 403
`<Error><Code>InvalidAccessKeyId</Code>`, so no signature and no presigned query string is
involved: the refusal is `refuse_cross_site_browsers` in
`vendor/smithy-server-runtime/src/multi_service_router.rs`, which runs before authentication for
every request carrying an `Origin` header and is hardcoded to `RemoteOrigins::SameOriginOnly`
(`vendor/server/src/listener_guard.rs`); S3's authentication-refusal wording then restates the
framework's generic `AccessDenied` as `InvalidAccessKeyId`. A service crate cannot reach that
gate (it runs after the operation is resolved and before authentication, and the only earlier
seam is reached only when no operation is named, where serving the request would skip signature
verification). The core change it needs: let the claiming service decide an `Origin`-carrying
request — for S3, admit exactly the origins a bucket's stored cors configuration names — after
which the `Access-Control-*` decoration of a real request becomes implementable.
