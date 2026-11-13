Issue-Source: trace-sweep-divergence
Issue-Trace: s3-mbes-traj-delete-objects-batch-mistakes
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-mbes-traj-delete-objects-batch-mistakes
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|DeleteObjects|2xx->4xx|$.DeleteResult
Issue-Key: s3|DeleteObjects|2xx->4xx|status
Issue-Commit: f1473582b6746d116724ae43d0fc6ffbe3d6e185

# s3 DeleteObjects refuses a valid versioned batch delete with 400 instead of deleting the version

> _Note: this issue was identified against the code base at commit `f1473582b6746d116724ae43d0fc6ffbe3d6e185`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteObjects

## Description
One net-new cause in the S3 multi-object delete path.

Cause 1 of 1: a well-formed DeleteObjects request that names a single object identifier (Key plus VersionId) is refused at the request level with HTTP 400 and an empty-of-DeleteResult body, instead of being carried out and reported as a per-key result.

AWS: Smithy: DeleteObjects -> POST /{Bucket}?delete, success status 200, output shape DeleteObjectsOutput serialized with xmlName `DeleteResult` and members `Deleted` (xmlFlattened list of DeletedObject{Key,VersionId,DeleteMarker,DeleteMarkerVersionId}) and `Errors` (xmlName `Error`, xmlFlattened). The operation declares NO modeled errors. Its own documentation: "For each key, Amazon S3 performs a delete operation and returns the result of that delete, success or failure, in the response. If the object specified in the request isn't found, Amazon S3 confirms the deletion by returning the result as deleted", and "The request can contain a list of up to 1,000 keys". Delete$Objects is @required with at least one Object element, and ObjectIdentifier$VersionId is the documented way to "delete a specific version of the object". So the ONLY request-level refusals AWS makes here are a payload outside the published schema (400 MalformedXML, fixed text "The XML you provided was not well-formed or did not validate against our published schema"), a bucket that does not exist (404 NoSuchBucket), an ExpectedBucketOwner mismatch (403 AccessDenied) and an MFA-Delete token problem. Everything a single identifier can get wrong -- an absent key, an unresolvable version, a failed ETag/size/time precondition, an Object Lock refusal, an authorization denial -- is that key's own entry under `Errors` inside a 200 response, never a status on the whole request. A one-identifier batch naming an existing version is therefore a 200 whose DeleteResult carries exactly one Deleted entry echoing that Key and VersionId.

Emulator: the replayed step is answered HTTP 400 and the response carries no DeleteResult element at all (the expected Deleted entry for the key and its version id is absent). The delete is not performed and the caller is told nothing about any key.

Hypothesis: in services/service-com-amazonaws-s3-amazons3/src/ops/delete_objects.rs the handler's only request-level 400 is the guard `if identifiers.is_empty() || identifiers.len() > MAX_KEYS_PER_DELETE` which answers the unmodelled code `MalformedXML` / status 400 / message "The XML you provided was not well-formed or did not validate against our published schema" (crate::state::MALFORMED_XML_CODE / _STATUS / _MESSAGE). Every other branch in that handler answers 403, 404, 501 or a per-key `Errors` entry under 200. So the 400 is most consistent with the `Delete` payload of this step deserializing with an EMPTY `Objects` list -- i.e. the xmlFlattened `Object` members of the `Delete` httpPayload are not bound for this payload shape -- after which the malformed-payload guard rejects a batch that in fact named one valid identifier. The status, the error code and that fixed sentence are the requirement being broken: AWS reserves MalformedXML for a document that really does not validate, and its message has no variable parts, so there is nothing in it that could be request-specific.

Why: a client that batches deletes -- the CLI's `s3 rm`/`s3 rb --force`, the SDK transfer managers, CDK's bucket auto-delete, any cleanup loop -- gets a hard 400 for a request AWS accepts. Because the body carries no DeleteResult, the client cannot even learn which keys succeeded and which failed, so it has no way to retry the remainder; the objects it believes it removed stay stored, and teardown of versioned buckets wedges. A MalformedXML verdict also misdirects the operator towards their own request payload, which is correct.

Fix: the DeleteObjects request path -- the deserialization/binding of the `Delete` httpPayload into `Delete$Objects` (the xmlFlattened `Object` list) and the malformed-payload guard in the operation's handler that judges that list, so that a payload naming one or more identifiers reaches the per-identifier loop and the operation returns 200 with a DeleteResult whose `Deleted`/`Errors` entries report each identifier, reserving 400 MalformedXML for a list that is genuinely empty or longer than 1,000.

Traces: s3-mbes-traj-delete-objects-batch-mistakes#task#6 (the preceding step task#5 of the same fixture is a deliberately malformed batch and IS answered 400 correctly, so the refusal is not a blanket one).

## Evidence
Replay of fixture s3-mbes-traj-delete-objects-batch-mistakes, step task#6, operation DeleteObjects (service s3):

  verdict: mismatch
  expected_status: 200
  actual_status: 400
  detail: status: expected 200, got 400
          $.DeleteResult: missing (expected {'Deleted': {'Key': 'svc-file1.txt', 'VersionId': '05c40377da890ce460298ea841a46d6d'}}, got None)
  divergence_key: service=s3|DeleteObjects|status=200->400|keys=$.DeleteResult,status
  affected: 1 step (s3-mbes-traj-delete-objects-batch-mistakes#task#6)

Expected (AWS): 200 with <DeleteResult><Deleted><Key>{the key named by the request's Object element}</Key><VersionId>{the version id that identifier named}</VersionId></Deleted></DeleteResult>. The key and version id are values taken from the request and from the bucket's stored versions; only the element structure is fixed.

Actual (emulator): 400, no DeleteResult element, no Deleted and no Errors entry -- nothing reported for the identifier. The handler's only 400 channel is the unmodelled MalformedXML / "The XML you provided was not well-formed or did not validate against our published schema" (a code with no modeled shape and a message whose text is entirely fixed), which is not a verdict AWS reaches for a batch that names one resolvable version.

Contrast within the same fixture: step task#5 is a deliberately malformed batch, expects 400 and IS answered 400 (its only remaining divergence is the request-id format, owned by 20261102T111455Z-s3-request-ids-are-uuids-instead-of-aws.md). So the emulator distinguishes the steps but misjudges the valid one.

## Steps to Reproduce
1. Boot the emulator from this checkout and point an AWS SDK/CLI at it.
2. Replay fixture s3-mbes-traj-delete-objects-batch-mistakes up to and including step task#6 (`replay_wire_corpus`), or reproduce by hand:
   a. Create a bucket and enable versioning on it (PutBucketVersioning Status=Enabled).
   b. PutObject a key (e.g. the fixture's svc-file1.txt) and note the VersionId the response returns.
   c. Optionally issue the fixture's earlier malformed DeleteObjects batches (step task#5) to reach the same state.
   d. POST /<bucket>?delete with the Delete payload naming exactly that one identifier:
      <Delete><Object><Key>svc-file1.txt</Key><VersionId>{version id from step b}</VersionId></Object></Delete>
3. Observe HTTP 400 with no <DeleteResult> element, and the version still present in ListObjectVersions.
4. AWS answers 200 with <DeleteResult><Deleted><Key>svc-file1.txt</Key><VersionId>{that version id}</VersionId></Deleted></DeleteResult> and removes the version.
