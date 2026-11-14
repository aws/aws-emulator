Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.s3-csvcols-get-object-acl-of-a-delete-marker-version-rejected-method-not-allowed
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.s3-csvcols-get-object-acl-of-a-delete-marker-version-rejected-method-not-allowed|run#9|AssertionError|assert (hdrs.get("x-amz-delete-marker"), hdrs.get("x-amz-version-id")) == ("true", marker["VersionId"]), (
Issue-Commit: 334866e49a2e1dd36c287c2dd48bc4975a5ca561

# s3 GetObjectAcl on a delete-marker version: the 405 MethodNotAllowed refusal omits the x-amz-delete-marker and x-amz-version-id response headers

> _Note: this issue was identified against the code base at commit `334866e49a2e1dd36c287c2dd48bc4975a5ca561`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObjectAcl

## Description
Cause 1 of 1: the delete-marker refusal of GetObjectAcl carries no delete-marker headers.
AWS: addressing GetObjectAcl at a delete marker's own VersionId is refused 405 Code 'MethodNotAllowed', Message 'The specified method is not allowed against this resource.' (fixed text, no variable parts), with Error.Method 'GET' and Error.ResourceType 'DeleteMarker', AND the response carries the two delete-marker headers that every S3 refusal over a delete marker carries: 'x-amz-delete-marker: true' and 'x-amz-version-id' set to the version id of the delete marker that was addressed (a value taken from state - the VersionId the DeleteObject that created the marker returned - not a fixed literal). This is the same header pair GetObject's own delete-marker refusal carries, which is how a client distinguishes 'this version is a delete marker' from any other 405. Smithy: GetObjectAcl (GET /{Bucket}/{Key+}?acl, VersionId as httpQuery 'versionId') models only NoSuchKey (404); the MethodNotAllowed refusal and its headers are recorded behavior only.
Emulator: the status, code, message and the Error.Method/Error.ResourceType members are all correct, but the refusal response carries only content-type, x-amz-request-id, x-amz-id-2, content-length and date - neither x-amz-delete-marker nor x-amz-version-id is set.
Why: a client walking a versioned listing and asking for each version's ACL sees a bare 405 and cannot tell from the response that the version it addressed is a delete marker, nor which version the refusal is about; SDK helpers and tooling that branch on the x-amz-delete-marker header (as they do for GetObject/HeadObject) treat the refusal as an unrelated method error and either abort the walk or retry it forever.
Fix: the delete-marker branch of the GetObjectAcl handler (its error serializer) must emit x-amz-delete-marker: true and x-amz-version-id with the addressed delete marker's version id, as the GetObject delete-marker path already must.
Traces: s3.s3-csvcols-get-object-acl-of-a-delete-marker-version-rejected-method-not-allowed.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1, service heading '### s3'. The check is net-new: the replay marked it neither ALREADY FILED nor OWNED by an open issue.

[mismatch] s3.s3-csvcols-get-object-acl-of-a-delete-marker-version-rejected-method-not-allowed - AssertionError: 'the refusal must flag the delete marker and echo its VersionId; headers were {content-type: application/xml, x-amz-request-id: ..., x-amz-id-2: ..., content-length: 322, date: ...}'. The preceding assertions in the same check passed, i.e. the emulator did return 405 / MethodNotAllowed / 'The specified method is not allowed against this resource.' with Error.Method 'GET' and Error.ResourceType 'DeleteMarker'; only the two headers are missing. (The request id, id-2, content-length and date in that dump are per-run values and are not part of the requirement.)

Smithy (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json): GetObjectAcl GET /{Bucket}/{Key+}?acl, success 200, VersionId httpQuery 'versionId', errors: [NoSuchKey -> 404]. The delete-marker refusal and its headers are recorded behavior only.

## Steps to Reproduce
1. Re-replay s3.s3-csvcols-get-object-acl-of-a-delete-marker-version-rejected-method-not-allowed.
2. Create a versioning-enabled bucket and PutObject a key; keep the returned VersionId.
3. DeleteObject the key without a VersionId; keep the delete marker's own VersionId from the response.
4. GetObjectAcl with VersionId = the data version's id: must still succeed and return Owner (positive control; it does).
5. GetObjectAcl with VersionId = the delete marker's id: expect HTTP 405 with Code 'MethodNotAllowed', Message 'The specified method is not allowed against this resource.', Error.Method 'GET', Error.ResourceType 'DeleteMarker', and response headers 'x-amz-delete-marker: true' and 'x-amz-version-id: <the delete marker's VersionId from step 3>'. The emulator returns the body correctly but omits both headers.
