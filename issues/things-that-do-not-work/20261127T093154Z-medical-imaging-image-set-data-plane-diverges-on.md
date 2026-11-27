Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medical-imaging.copy-image-set-non-latest-source-version-rejected medical-imaging.update-image-set-metadata-stale-version-not-found medical-imaging.get-image-set-metadata-unknown-image-set-id-rejected medical-imaging.get-image-set-unreached-version-not-found-rejected medical-imaging.list-image-set-versions-unknown-image-set-rejected medical-imaging.get-image-frame-unknown-frame-id-rejected medical-imaging.get-image-frame-of-deleted-image-set-rejected medical-imaging.get-image-frame-unknown-frame-id-not-found-rejected medical-imaging.get-image-set-metadata-copy-destination-locked-conflict medical-imaging.get-image-set-metadata-non-numeric-version-rejected medical-imaging.list-image-set-versions-max-results-out-of-range-rejected medical-imaging.start-dicom-import-job-https-input-uri-rejected medical-imaging.list-image-set-versions-unissued-next-token-rejected medical-imaging.search-image-sets-unissued-next-token-rejected medical-imaging.search-image-sets-between-with-one-value-rejected
Issue-Shape-Id: com.amazonaws.medicalimaging#AHIGatewayService
Issue-Key: medical-imaging.copy-image-set-non-latest-source-version-rejected|run#7|AssertionError|assert got == (
Issue-Key: medical-imaging.update-image-set-metadata-stale-version-not-found|run#10|AssertionError|assert got == expected, f"an update against superseded version '1' was refused with {got!r}, expected {expected!r}"
Issue-Key: medical-imaging.get-image-set-metadata-unknown-image-set-id-rejected|run#26|AssertionError|assert got == ("ResourceNotFoundException", _NO_IMAGE_SET), (
Issue-Key: medical-imaging.get-image-set-unreached-version-not-found-rejected|run#6|AssertionError|assert (code, message) == ("ResourceNotFoundException", _ABSENT), (
Issue-Key: medical-imaging.list-image-set-versions-unknown-image-set-rejected|run#13|AssertionError|assert got == (
Issue-Key: medical-imaging.get-image-frame-unknown-frame-id-rejected|run#12|AssertionError|assert body[:12] == _JP2_SIGNATURE_BOX, f"the frame starts {body[:12].hex()!r}, not a JPEG 2000 signature box"
Issue-Key: medical-imaging.get-image-frame-of-deleted-image-set-rejected|_codestream_geometry#6|AssertionError|assert blob[:12] == b"\x00\x00\x00\x0cjP \r\n\x87\n", f"frame lacks the JP2 signature box: {blob[:12].hex()}"
Issue-Key: medical-imaging.get-image-frame-unknown-frame-id-not-found-rejected|_codestream_geometry#6|AssertionError|assert blob[:12] == b"\x00\x00\x00\x0cjP \r\n\x87\n", f"frame lacks the JP2 signature box: {blob[:12].hex()}"
Issue-Key: medical-imaging.get-image-set-metadata-copy-destination-locked-conflict|run#10|AssertionError|assert early == refused, (
Issue-Key: medical-imaging.get-image-set-metadata-non-numeric-version-rejected|run#7|AssertionError|assert got == ("ValidationException", pattern_message), (
Issue-Key: medical-imaging.list-image-set-versions-max-results-out-of-range-rejected|run#9|AssertionError|assert low == ("ValidationException", f"{rule} greater than or equal to 1"), f"maxResults=0 answered {low!r}"
Issue-Key: medical-imaging.start-dicom-import-job-https-input-uri-rejected|run#7|AssertionError|assert got == ("ValidationException", _S3_URI_REFUSAL), (
Issue-Key: medical-imaging.list-image-set-versions-unissued-next-token-rejected|run#13|AssertionError|code == "ValidationException"
Issue-Key: medical-imaging.search-image-sets-unissued-next-token-rejected|run#13|AssertionError|code == "ValidationException"
Issue-Key: medical-imaging.search-image-sets-between-with-one-value-rejected|run#9|AssertionError|assert got == ("ValidationException", "Two values need to be provided for operator BETWEEN."), (
Issue-Commit: f91d45c7866d80628978681094afeabba34e454b

# medical-imaging image-set data plane diverges on seven points: stale-version error class, not-found and not-ACTIVE refusal texts, non-JPEG2000 image frames, echoed constraint values, nextToken refusal text and the BETWEEN-arity refusal (7 causes)

> _Note: this issue was identified against the code base at commit `f91d45c7866d80628978681094afeabba34e454b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetImageFrame, GetImageSetMetadata, GetImageSet, CopyImageSet, UpdateImageSetMetadata, ListImageSetVersions, SearchImageSets, StartDICOMImportJob

## Description
Seven independent root causes in the HealthImaging (medical-imaging) image-set data plane, all surfaced by the behavioral suite. A fixer must close all seven before submitting.

=== Cause 1 of 7: a stale `latestVersionId` is refused with the wrong error class and text ===
AWS: `CopyImageSet` (`copyImageSetInformation.sourceImageSet.latestVersionId`) and `UpdateImageSetMetadata` (`latestVersionId`, `@httpQuery latestVersion`) both refuse a version identifier that is not the image set's current latest with **HTTP 404 `ResourceNotFoundException`** and the message `Requested version(s) of ImageSet(s) is not the latest.` — the whole sentence is fixed text; it interpolates neither the supplied version nor the actual latest version. Smithy: both operations model `ResourceNotFoundException` (httpError 404) and `ConflictException` (httpError 409) as distinct errors; recorded behavior puts the stale-version case in the 404 class.
Emulator: answers **409 `ConflictException`** with `The latest image set version identifier provided does not match the image set's latest version.` (the constant `IMAGE_SET_VERSION_CONFLICT_MESSAGE`, services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:438-439), for both operations.
Why: a client cannot tell a stale optimistic-concurrency token from a genuine resource conflict. 409 is the retryable/"re-read and retry" class in every AWS SDK's error taxonomy and in `botocore`-based handling, while 404 is terminal-for-this-version; code branching on `ResourceNotFoundException` (or on status 404) to re-read the latest version and retry never fires, and code branching on `ConflictException` retries forever against an identifier that will never become latest again. The message text also differs, so message-matching clients break.
Fix: the stale-`latestVersionId` guard in the `CopyImageSet` handler and in the `UpdateImageSetMetadata` handler must raise `ResourceNotFoundException` with the fixed text above; the 409 constant in the state module stops being the answer for this rule.
Traces: medical-imaging.copy-image-set-non-latest-source-version-rejected, medical-imaging.update-image-set-metadata-stale-version-not-found.

=== Cause 2 of 7: the ResourceNotFoundException text for an unknown image set / unreached version is worded by resource kind instead of AWS's sentence ===
AWS: a request naming an image set the data store does not hold, and a request naming a `versionId` the image set has never reached, are both refused with HTTP 404 `ResourceNotFoundException` and the single fixed sentence `Requested ImageSet resource or its version does not exist.` One sentence covers both the unknown-image-set and the unknown-version case, and it interpolates nothing from the request. This is observed on `GetImageSetMetadata`, `GetImageSet` (`versionId`, `@httpQuery version`) and `ListImageSetVersions`.
Emulator: answers two different sentences — `ImageSet resource is not found.` (`IMAGE_SET_NOT_FOUND_MESSAGE`, state.rs:427) for the unknown image set, and `ImageSet version resource is not found.` (`IMAGE_SET_VERSION_NOT_FOUND_MESSAGE`, state.rs:431-432) for the unreached version. The code and status (404) are right; only the text diverges.
Why: the error message is part of the contract. Clients, log scrapers and test suites that assert on the HealthImaging sentence see an unknown message, and the emulator reveals a distinction (image set vs version) that AWS deliberately collapses into one sentence, so a client written against the emulator can grow a dependency on a distinction real AWS does not expose.
Fix: the not-found message constants in the state module, and the handlers of `GetImageSetMetadata`, `GetImageSet` and `ListImageSetVersions` that select between them — one constant with AWS's sentence for both cases.
Traces: medical-imaging.get-image-set-metadata-unknown-image-set-id-rejected, medical-imaging.get-image-set-unreached-version-not-found-rejected, medical-imaging.list-image-set-versions-unknown-image-set-rejected.

=== Cause 3 of 7: GetImageFrame serves bytes that are not a JPEG 2000 file at all ===
AWS: `GetImageFrame` returns the frame as an HTJ2K/JPEG 2000 **file**: the payload begins with the 12-byte JP2 signature box `00 00 00 0c 6a 50 20 20 0d 0a 87 0a`, is a sequence of well-formed JP2 boxes including a `jp2c` codestream box, and that codestream opens with the SOC+SIZ markers `ff 4f ff 51`, whose SIZ fields report the frame's geometry (Xsiz-XOsiz = Columns, Ysiz-YOsiz = Rows, Csiz = Samples per Pixel, Ssiz = bit depth) taken from the imported instance's `(0028,0011) Columns`, `(0028,0010) Rows`, `(0028,0002) SamplesPerPixel`, `(0028,0100) BitsAllocated`. Smithy: `GetImageFrame.imageFrameBlob` is a required, streaming `@httpPayload` blob with a `Content-Type` header; the model does not state the container, so the container requirement is recorded behavior only.
Emulator: serves a payload whose first 12 bytes are `00070e151c232a31383f464d` — a synthetic arithmetic ramp (0, 7, 14, 21, 28, …), not a JP2 signature box. There is therefore no JP2 box structure, no `jp2c` box and no SOC/SIZ marker pair, and no frame geometry can be read from it.
Why: every DICOM/medical-imaging client decodes this payload with a JPEG 2000 decoder (OpenJPEG, Grok, the AWS HealthImaging pixel-data helpers). Against the emulator each one fails at the first box, so no client can render or verify a frame, and the frame's geometry cannot be cross-checked against the image set's metadata.
Fix: the `GetImageFrame` handler's payload serialization, together with whatever the DICOM import pipeline stores as the frame's pixel payload — the stored frame must be a JP2-wrapped codestream whose SIZ geometry is derived from the imported instance's pixel-description attributes, not a generated byte ramp.
Traces: medical-imaging.get-image-frame-unknown-frame-id-rejected, medical-imaging.get-image-frame-of-deleted-image-set-rejected, medical-imaging.get-image-frame-unknown-frame-id-not-found-rejected (all three read the frame of a healthy image set as the positive control before exercising their refusal, and all three stop at the signature box).

=== Cause 4 of 7: GetImageSetMetadata does not refuse an image set that is not ACTIVE ===
AWS: `GetImageSetMetadata` on an image set that is not in the `ACTIVE` state — e.g. a `CopyImageSet` destination while it still reads `LOCKED` — is refused with HTTP 409 `ConflictException` and the fixed message `Requested ImageSet is not in ACTIVE state.` (fixed text in full; it names neither the image set nor the state it was in). Smithy: `GetImageSetMetadata` models `ConflictException` with httpError 409.
Emulator: answers HTTP 200 with a metadata document for the `LOCKED` destination (the check read `imageSetState == 'LOCKED'` from `GetImageSet` immediately afterwards, and a destination never returns to `LOCKED`, so it was `LOCKED` when the read was answered). The served document is also hollow — `{"SchemaVersion":"1.1","DatastoreID":…,"ImageSetID":…,"Patient":{"DICOM":{}},"Study":{"DICOM":{"NumberOfStudyRelatedSeries":0,"NumberOfStudyRelatedInstances":0},"Series":{}}}` — i.e. the copy's attributes are not there yet, which is exactly why AWS refuses the read rather than serving it. Note the emulator's related constants are worded differently too (state.rs:444-445 `The image set is not in the ACTIVE state, so it cannot take part in a copy.`, state.rs:462-463 `The image set is not in the ACTIVE state, so its metadata cannot be updated.`), so even once the guard exists the text must be AWS's single sentence.
Why: a client that reads a copy destination's metadata as soon as `CopyImageSet` returns gets a successful response holding an empty patient/study document instead of the 409 that tells it to wait. It will cache or act on metadata that does not describe the image set, where against AWS it would retry until the destination settles `ACTIVE`/`COPIED`.
Fix: the `GetImageSetMetadata` handler's precondition check (the not-ACTIVE guard its sibling operations already apply), plus the refusal text constant in the state module.
Traces: medical-imaging.get-image-set-metadata-copy-destination-locked-conflict.

=== Cause 5 of 7: constraint clauses echo `Value '<v>' at '<path>'` for members AWS withholds ===
AWS: HealthImaging words a constraint violation on `versionId`, `maxResults` and `inputS3Uri` with NO echoed value — the clause is `<member> failed to satisfy constraint: <requirement>` inside the usual envelope. Fixed text: `1 validation error detected: `, ` failed to satisfy constraint: `, and the requirement sentence; variable parts: the member path and the requirement's limit/pattern taken from the model. Observed sentences (HTTP 400 `ValidationException`):
  * `GetImageSetMetadata` with `versionId='v1'` -> `1 validation error detected: versionId failed to satisfy constraint: Member must satisfy regular expression pattern: \d+` (Smithy: `ImageSetExternalVersionId` @pattern `^\d+$`, quoted unanchored).
  * `ListImageSetVersions` with `maxResults=0` -> `1 validation error detected: maxResults failed to satisfy constraint: Member must have value greater than or equal to 1`, and with `maxResults=51` -> `… less than or equal to 50` (Smithy: `maxResults` @range min 1 max 50).
  * `StartDICOMImportJob` with an https URL in `inputS3Uri` -> `1 validation error detected: inputS3Uri failed to satisfy constraint: Member must satisfy regular expression pattern: s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?` (Smithy: `S3Uri` @pattern, quoted unanchored).
Emulator: prefixes each clause with the framework's `Value '<v>' at '<path>'` subject, e.g. `1 validation error detected: Value 'v1' at 'versionId' failed to satisfy constraint: Member must satisfy regular expression pattern: \d+`, `… Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1`, and `… Value 'https://<bucket>.s3.<region>.amazonaws.com/dicom/' at 'inputS3Uri' failed to satisfy constraint: Member must satisfy regular expression pattern: s3://…`. The withholding rule is an allowlist (`VALUE_WITHHOLDING_MEMBERS` at state.rs:678-685 and `URI_LABEL_MEMBERS` at state.rs:705-711, applied by `constraint_clause` at state.rs:608-620) that does not contain `versionId`, `maxResults` or `inputS3Uri`/`outputS3Uri`.
Why: the clause is the documented contract for a 400 from this service. A client (or a user-facing form) that parses the clause to name the offending member reads `Value 'v1' at 'versionId'` where AWS gives `versionId`, so the parse diverges; and the emulator reflects caller-supplied request values — including a full S3/HTTPS location — back into an error body this clinical service keeps value-free.
Fix: the constraint-clause formatter and its withholding classes in the state module — the member classes whose clause carries no value must include these members, so the clause is `<member> failed to satisfy constraint: <requirement>`.
Traces: medical-imaging.get-image-set-metadata-non-numeric-version-rejected, medical-imaging.list-image-set-versions-max-results-out-of-range-rejected, medical-imaging.start-dicom-import-job-https-input-uri-rejected.

=== Cause 6 of 7: a nextToken the service never issued is refused with the wrong message ===
AWS: `ListImageSetVersions` and `SearchImageSets` refuse a `nextToken` they never minted with HTTP 400 `ValidationException` whose message names the operation and the wrapping schema it read out of the token: it starts with the fixed text `Pagination token exception in operation '`, then the operation name, then the fixed text `': invalid payload wrapping schema `, then the schema number — the unsigned big-endian 16-bit integer formed by the first two bytes of the base64-decoded token. Fixed text: the two literal fragments; variable parts: the operation name from the request and the schema number read from the supplied token. Smithy: both operations are `@paginated` on `nextToken` (`NextToken`, @length 1-8192, @pattern `^\p{ASCII}{0,8192}$`) and model `ValidationException` (httpError 400); the token's internal format is recorded behavior only.
Emulator: answers `ValidationException` with the constant `Failed to perform requested operation due to invalid NextToken. Please try again.` (`INVALID_NEXT_TOKEN_MESSAGE`, state.rs:576-577) — it names neither the operation nor the schema it decoded.
Why: the code and status are right, so paging still terminates, but any client, test or operator diagnosing a bad cursor gets a message that does not match AWS's and cannot identify which operation's token namespace the cursor failed in. A client asserting on the AWS wording treats a correct refusal as an unknown failure.
Fix: the page-token decode path and its refusal text in the state module, plus the per-operation plumbing that gives it the operation name so the message can name it.
Traces: medical-imaging.list-image-set-versions-unissued-next-token-rejected, medical-imaging.search-image-sets-unissued-next-token-rejected.

=== Cause 7 of 7: a BETWEEN filter with one bound is refused with a generic constraint clause instead of AWS's sentence ===
AWS: `SearchImageSets` refuses a `searchCriteria.filters` entry whose `operator` is `BETWEEN` but which carries only one value with HTTP 400 `ValidationException` and the message `Two values need to be provided for operator BETWEEN.` — the whole sentence is fixed text: it is not a `… validation error detected …` envelope, it names no filter index and no member path, and it echoes nothing from the request. Smithy: `SearchImageSets` models `ValidationException` (httpError 400); the model states no arity rule for `BETWEEN`, so this is recorded behavior only.
Emulator: answers `ValidationException` with `1 validation error detected: searchCriteria.filters.1.member.values failed to satisfy constraint: Member must hold a lower bound and an upper bound when the operator is BETWEEN` — the service's generic one-violation envelope with a hand-written requirement and a positional member path.
Why: this is the refusal every caller composing a date/time range hits first. A client matching AWS's sentence (or surfacing it to a user) sees an unrelated message, and the emulator advertises a validation-path vocabulary for this rule that AWS does not use.
Fix: the `BETWEEN` arity check in the `SearchImageSets` handler's filter validation — it must raise the service's own fixed sentence rather than route through the generic constraint-clause envelope. (Related but distinct from the open issue 20261029T174835Z-medical-imaging-searchimagesets-error-response--r2.md, which is about the *enum-value* clause wording in the same handler; that issue does not cover this arity refusal.)
Traces: medical-imaging.search-image-sets-between-with-one-value-rejected.

## Evidence
Suite: 25/43 passed; medical-imaging 24/40 (16 failed). Expected vs actual per cause:

Cause 1 — CopyImageSet with sourceImageSet.latestVersionId='2' while the latest is '1':
  expected: ('ResourceNotFoundException', 'Requested version(s) of ImageSet(s) is not the latest.')  [HTTP 404]
  actual:   ('ConflictException', "The latest image set version identifier provided does not match the image set's latest version.")  [HTTP 409]
  UpdateImageSetMetadata with latestVersionId='1' after version 2 exists: same expected, same actual.

Cause 2 — GetImageSetMetadata for an imageSetId never created:
  expected: ('ResourceNotFoundException', 'Requested ImageSet resource or its version does not exist.')
  actual:   ('ResourceNotFoundException', 'ImageSet resource is not found.')
  GetImageSet(versionId='2') on a version-1 image set:
  expected: ('ResourceNotFoundException', 'Requested ImageSet resource or its version does not exist.')
  actual:   ('ResourceNotFoundException', 'ImageSet version resource is not found.')
  ListImageSetVersions for an imageSetId the data store never held:
  expected: ('ResourceNotFoundException', 'Requested ImageSet resource or its version does not exist.')
  actual:   ('ResourceNotFoundException', 'ImageSet resource is not found.')

Cause 3 — GetImageFrame on a healthy, imported, ACTIVE image set:
  expected: payload[:12] == 00 00 00 0c 6a 50 20 20 0d 0a 87 0a (JP2 signature box), then JP2 boxes including jp2c, whose codestream opens ff4fff51 (SOC+SIZ)
  actual:   payload[:12] == 00070e151c232a31383f464d — an 0,7,14,21,… byte ramp; assertion 'frame lacks the JP2 signature box: 00070e151c232a31383f464d' / 'the frame starts \'00070e151c232a31383f464d\', not a JPEG 2000 signature box' in all three frame checks.

Cause 4 — GetImageSetMetadata on a CopyImageSet destination read as 'LOCKED' immediately after:
  expected: ('ConflictException', 'Requested ImageSet is not in ACTIVE state.')  [HTTP 409]
  actual:   HTTP 200 with {'SchemaVersion': '1.1', 'DatastoreID': <id>, 'ImageSetID': <id>, 'Patient': {'DICOM': {}}, 'Study': {'DICOM': {'NumberOfStudyRelatedSeries': 0, 'NumberOfStudyRelatedInstances': 0}, 'Series': {}}} — a hollow document for an image set the copy had not finished populating.

Cause 5 — ValidationException clauses (HTTP 400):
  GetImageSetMetadata versionId='v1'
    expected: "1 validation error detected: versionId failed to satisfy constraint: Member must satisfy regular expression pattern: \\d+"
    actual:   "1 validation error detected: Value 'v1' at 'versionId' failed to satisfy constraint: Member must satisfy regular expression pattern: \\d+"
  ListImageSetVersions maxResults=0
    expected: "1 validation error detected: maxResults failed to satisfy constraint: Member must have value greater than or equal to 1"
    actual:   "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1"
    (same divergence for maxResults=51 / '… less than or equal to 50')
  StartDICOMImportJob inputS3Uri='https://<bucket>.s3.<region>.amazonaws.com/dicom/'
    expected: "1 validation error detected: inputS3Uri failed to satisfy constraint: Member must satisfy regular expression pattern: s3://[a-z0-9][\\.\\-a-z0-9]{1,61}[a-z0-9](/.*)?"
    actual:   "1 validation error detected: Value '<the https URL the request sent>' at 'inputS3Uri' failed to satisfy constraint: Member must satisfy regular expression pattern: s3://[a-z0-9][\\.\\-a-z0-9]{1,61}[a-z0-9](/.*)?"
  (bucket name, region and the concrete version/maxResults values are per-run request values, not part of the required text; the required text is the member-named clause with no echoed value.)

Cause 6 — ListImageSetVersions / SearchImageSets with nextToken='c2ltMnJlYWwtbmV2ZXItaXNzdWVk' (a token the service never issued):
  expected: ValidationException, message starting "Pagination token exception in operation '" and ending "': invalid payload wrapping schema <N>", where N is the big-endian uint16 of the first two decoded bytes
  actual:   ('ValidationException', 'Failed to perform requested operation due to invalid NextToken. Please try again.') for both operations

Cause 7 — SearchImageSets with filters=[{operator: BETWEEN, values: [<one value>]}]:
  expected: ('ValidationException', 'Two values need to be provided for operator BETWEEN.')
  actual:   ('ValidationException', '1 validation error detected: searchCriteria.filters.1.member.values failed to satisfy constraint: Member must hold a lower bound and an upper bound when the operator is BETWEEN')

Code references for the diverging texts/rules (commit as cloned): services/service-com-amazonaws-medicalimaging-ahigatewayservice/src/state.rs:427 (IMAGE_SET_NOT_FOUND_MESSAGE), :431-432 (IMAGE_SET_VERSION_NOT_FOUND_MESSAGE), :438-439 (IMAGE_SET_VERSION_CONFLICT_MESSAGE), :444-445 and :462-463 (not-ACTIVE texts), :576-577 (INVALID_NEXT_TOKEN_MESSAGE), :608-620 (constraint_clause), :678-685 (VALUE_WITHHOLDING_MEMBERS), :705-711 (URI_LABEL_MEMBERS).

## Steps to Reproduce
Common setup for every cause: create a data store and wait for ACTIVE; create an S3 bucket plus an IAM role HealthImaging can assume; put one DICOM Part 10 Secondary Capture instance (Rows/Columns set, BitsAllocated 8, SamplesPerPixel 1, MONOCHROME2, PixelData present) under s3://<bucket>/dicom/; StartDICOMImportJob and wait for COMPLETED; SearchImageSets on DICOMStudyInstanceUID to get the imported imageSetId (version '1', ACTIVE).

Cause 1: CopyImageSet(datastoreId, sourceImageSetId=<imported>, copyImageSetInformation={'sourceImageSet': {'latestVersionId': '2'}}) while the latest is '1'. Expect 404 ResourceNotFoundException 'Requested version(s) of ImageSet(s) is not the latest.'; observe 409 ConflictException. Then UpdateImageSetMetadata(latestVersionId='1', DICOMUpdates=<any>) after an accepted update has made version '2'; same expectation, same divergence.

Cause 2: GetImageSetMetadata(imageSetId=<32 hex digits never created>); GetImageSet(imageSetId=<imported>, versionId='2') on a version-1 image set; ListImageSetVersions(imageSetId=<32 hex digits never created>). Each must answer 404 ResourceNotFoundException 'Requested ImageSet resource or its version does not exist.'

Cause 3: GetImageFrame(datastoreId, imageSetId=<imported>, imageFrameInformation={'imageFrameId': <the frame id from the image set's metadata>}) and read the payload. The first 12 bytes must be the JP2 signature box 000000 0c6a502020 0d0a870a and the file must contain a jp2c box whose codestream opens ff4fff51; observe the 00070e151c… ramp instead.

Cause 4: CopyImageSet(sourceImageSetId=<imported>, copyImageSetInformation={'sourceImageSet': {'latestVersionId': '1'}}), then immediately GetImageSetMetadata on destinationImageSetProperties.imageSetId, then GetImageSet on it. While GetImageSet reports imageSetState 'LOCKED', the metadata read must have been refused 409 ConflictException 'Requested ImageSet is not in ACTIVE state.'; observe HTTP 200 with a hollow document.

Cause 5: GetImageSetMetadata(versionId='v1'); ListImageSetVersions(maxResults=0) and (maxResults=51); StartDICOMImportJob(inputS3Uri='https://<bucket>.s3.<region>.amazonaws.com/dicom/'). Each 400 ValidationException message must name only the member ('versionId …', 'maxResults …', 'inputS3Uri …') with no "Value '<v>' at '<path>'" prefix.

Cause 6: create two versions of an image set (one accepted UpdateImageSetMetadata) and two matching image sets for one study; call ListImageSetVersions(maxResults=1, nextToken='c2ltMnJlYWwtbmV2ZXItaXNzdWVk') and SearchImageSets(maxResults=1, nextToken='c2ltMnJlYWwtbmV2ZXItaXNzdWVk'). Each must answer ValidationException "Pagination token exception in operation '<Operation>': invalid payload wrapping schema <uint16 of the token's first two decoded bytes>".

Cause 7: SearchImageSets(datastoreId, searchCriteria={'filters': [{'operator': 'BETWEEN', 'values': [{'createdAt': <one timestamp>}]}]}). Must answer 400 ValidationException 'Two values need to be provided for operator BETWEEN.'
