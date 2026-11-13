Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.write-get-object-response-roundtrip s3.write-get-object-response-lambda-callback s3.write-get-object-response-lambda-pattern s3.write-get-object-response-metadata-headers-passthrough s3.e2e-object-lambda-error-passthrough-flow s3.s3-object-lock-compliance-cannot-be-bypassed-while-governance-can s3.bucket-default-retention-derives-the-retain-until-date-and-blocks-the-delete s3.get-object-nonexistent-version-errors s3.update-object-encryption-error-paths s3.update-object-encryption-error-preconditions s3.e2e-create-s3-bucket-with-config-lifecycle s3.deploy-s3-bucket-public-access-block-enforced-rejected
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|GetObject|NoSuchBucket
Issue-Key: s3.s3-object-lock-compliance-cannot-be-bypassed-while-governance-can|run#22|AssertionError|assert refused is not None, (
Issue-Key: s3.bucket-default-retention-derives-the-retain-until-date-and-blocks-the-delete|run#19|AssertionError|assert head.get("ObjectLockMode") == "GOVERNANCE", (
Issue-Key: s3.get-object-nonexistent-version-errors|run#8|AssertionError|assert err.get("Code") == "InvalidArgument", f"expected InvalidArgument for a malformed VersionId, got {err!r}"
Issue-Key: s3.update-object-encryption-error-paths|_error#18|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: s3.update-object-encryption-error-preconditions|_wait_for_deny_effective#26|Timeout|time.sleep(interval)
Issue-Key: s3.e2e-create-s3-bucket-with-config-lifecycle|phase#7|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: s3|PutObjectAcl|AccessControlListNotSupported
Issue-Commit: 4bb3c01901698ef95426c2799d1398daad63aada

# S3: 7 net-new root causes — access-point ARN addressing, COMPLIANCE lock bypass, HeadObject lock projection, VersionId validation, UpdateObjectEncryption validation, CFN bucket policy, ObjectWriter ownership

> _Note: this issue was identified against the code base at commit `4bb3c01901698ef95426c2799d1398daad63aada`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
Seven distinct net-new root causes in the s3 service, each grounded in its own failing behavioral check(s). Related open issues 20261025T234719Z-s3-object-lock-is-not-enforced-retained-and.md, 20261025T032458Z-s3-object-plane-10-root-causes-conditional-requests.md and 20261026T052114Z-s3-cloudformation-does-not-install-an-aws-s3-bucket.md are adjacent but do not carry any of the replay keys below.

=== Cause 1 of 7: GetObject does not resolve an S3 access-point ARN as a bucket identifier (checks: s3.write-get-object-response-roundtrip, s3.write-get-object-response-lambda-callback, s3.write-get-object-response-lambda-pattern, s3.write-get-object-response-metadata-headers-passthrough, s3.e2e-object-lambda-error-passthrough-flow) ===
AWS: Smithy: GetObject.Bucket targets BucketName and is an @httpLabel; S3 accepts an access-point ARN (arn:aws:s3:<region>:<account>:accesspoint/<name>) in that position and routes the request to the underlying bucket, returning the object bytes (200). GetObject's modeled errors are NoSuchKey (404) and InvalidObjectState (403); NoSuchBucket is not a legitimate answer for an existing access point over an existing bucket.
Emulator: after s3control CreateAccessPoint succeeds for the bucket, GetObject with Bucket=<that access-point ARN> is refused with 404 `NoSuchBucket` / message `The specified bucket does not exist` — the ARN is treated as a literal bucket name instead of being resolved. Fixed text of the message is `The specified bucket does not exist`; there are no variable parts, and the correct response here is the object body, not an error.
Why: a client that addresses data through an access point (the documented way to scope S3 access, and the only way to reach Object Lambda) can read nothing at all, and the failure is indistinguishable from a missing bucket, so it cannot be diagnosed as an addressing gap.
Fix: S3's request-addressing/bucket-resolution layer shared by the object handlers (the path that maps the Bucket label to stored bucket state), plus the s3control access-point registry it must consult.
Traces: s3.write-get-object-response-roundtrip, s3.write-get-object-response-lambda-callback, s3.write-get-object-response-lambda-pattern, s3.write-get-object-response-metadata-headers-passthrough, s3.e2e-object-lambda-error-passthrough-flow (all five fail in the positive control, before the Object Lambda eligibility assertion they exist to make).

=== Cause 2 of 7: DeleteObject honours BypassGovernanceRetention against a COMPLIANCE retention (check: s3.s3-object-lock-compliance-cannot-be-bypassed-while-governance-can) ===
AWS: Smithy: DeleteObject.BypassGovernanceRetention is the header x-amz-bypass-governance-retention; it applies ONLY to GOVERNANCE mode. A version under ObjectLockMode=COMPLIANCE cannot be deleted by any principal, including the account root, before its ObjectLockRetainUntilDate: the delete is refused 403 AccessDenied.
Emulator: DeleteObject with BypassGovernanceRetention=True removed a COMPLIANCE-retained version inside its retain window and returned success; no refusal was produced at all.
Why: the write-once guarantee the COMPLIANCE mode exists to provide is void — a client validating a regulatory retention design against this emulator is told immutability holds when it does not.
Fix: the DeleteObject handler's object-lock enforcement (the branch that consults the bypass flag must be reachable only for GOVERNANCE).
Traces: s3.s3-object-lock-compliance-cannot-be-bypassed-while-governance-can.

=== Cause 3 of 7: HeadObject omits the inherited object-lock headers that GetObjectRetention reports (check: s3.bucket-default-retention-derives-the-retain-until-date-and-blocks-the-delete) ===
AWS: Smithy: HeadObject/GetObject output carry ObjectLockMode (x-amz-object-lock-mode) and ObjectLockRetainUntilDate (x-amz-object-lock-retain-until-date). When a bucket has an Object Lock DefaultRetention{Mode, Days}, every new version inherits it and HeadObject reports that mode and date, agreeing with GetObjectRetention for the same version.
Emulator: for a version written into a bucket whose default retention is {Mode: GOVERNANCE, Days: 1}, HeadObject reports ObjectLockMode absent (None) while GetObjectRetention reports the retention — the two views of the same version disagree.
Why: clients that gate a delete or a lifecycle decision on the HeadObject projection see an unlocked object and will attempt (or permit) a delete that AWS refuses.
Fix: the HeadObject/GetObject response serializer and the object-lock state it reads (inherited default retention must be materialised onto the version, not only answered by GetObjectRetention).
Traces: s3.bucket-default-retention-derives-the-retain-until-date-and-blocks-the-delete.

=== Cause 4 of 7: GetObject does not validate VersionId syntax (check: s3.get-object-nonexistent-version-errors) ===
AWS: Smithy: GetObject.VersionId is the `versionId` query parameter targeting ObjectVersionId. A syntactically malformed version id is a request error, not a lookup miss: S3 answers 400 `InvalidArgument` with message `Invalid version id specified` (fixed text; the offending value comes from the request, reported in the InvalidArgument ArgumentName/ArgumentValue fields). 404 `NoSuchVersion` is reserved for a well-formed id that does not exist.
Emulator: a malformed VersionId is answered 404 `NoSuchVersion` / `The specified version does not exist.` — the malformed-argument class is collapsed into the not-found class.
Why: a client cannot tell a bad parameter (retrying will never help, the code is a bug) from a version that has been deleted (retry/alternate path is correct); SDK retry and error-class handling diverge.
Fix: GetObject's input validation for the versionId parameter, ahead of the version lookup in state.
Traces: s3.get-object-nonexistent-version-errors.

=== Cause 5 of 7: UpdateObjectEncryption performs none of its precondition validation (checks: s3.update-object-encryption-error-paths, s3.update-object-encryption-error-preconditions) ===
AWS: Smithy: UpdateObjectEncryption (PUT /{Bucket}/{Key+}?encryption) models exactly three errors — AccessDenied (403), InvalidRequest (400), NoSuchKey (404). An SSE-KMS target that is not a resolvable, same-account KMS key is refused AccessDenied (message fixed text `The KMS key must be from the same account as the bucket`, so as not to disclose which keys exist); a key identifier that is not a valid KMS key ARN is refused 400 InvalidRequest with the fixed text `Requests modifying object encryption configuration to SSE-KMS require a valid target kms key arn: Malformed ARN - doesn't start with 'arn:'`; a missing object is NoSuchKey (`The specified key does not exist.`).
Emulator: the call SUCCEEDS with HTTP 200 (empty XML body) for an ARN-shaped-but-unresolvable / foreign-account KMS key instead of refusing; the sibling precondition check never reaches its assertions inside the 60s budget (it polls for the refusal classes that never arrive). No precondition appears to be evaluated at all.
Why: a client can re-encrypt an object onto a key it has no access to and be told it worked; the object's reported encryption state and the KMS grant model silently disagree, and negative paths every SDK surfaces as errors become successes.
Fix: the UpdateObjectEncryption handler's validation (object existence, target key ARN syntax, key resolution/ownership) before it mutates the object's encryption state.
Traces: s3.update-object-encryption-error-paths, s3.update-object-encryption-error-preconditions.

=== Cause 6 of 7: a CloudFormation-provisioned AWS::S3::Bucket comes up without its bucket policy (check: s3.e2e-create-s3-bucket-with-config-lifecycle) ===
AWS: recorded behavior only — a stack that declares a bucket together with its policy leaves the deployed bucket's GetBucketPolicy returning the policy document, so the statement Sids declared in the template are readable after deploy.
Emulator: after the deploy phase the bucket reads back versioning Enabled, SSE AES256 and all four PublicAccessBlock flags correctly, but `policy_sids` is empty — GetBucketPolicy reports no statements for the deployed bucket. The adjacent open issue 20261026T052114Z-s3-cloudformation-does-not-install-an-aws-s3-bucket.md does not carry this check's replay key.
Why: every authorization assertion a client makes after deploying an estate by template is evaluated against a bucket with no policy, so denials that AWS enforces (and allows that the policy grants) are both wrong.
Fix: the CloudFormation resource provider for the S3 bucket-policy resource and the S3 bucket-policy state it must write.
Traces: s3.e2e-create-s3-bucket-with-config-lifecycle (phase 1/5 deploy).

=== Cause 7 of 7: a bucket whose OwnershipControls are ObjectWriter is treated as ACLs-disabled (check: s3.deploy-s3-bucket-public-access-block-enforced-rejected) ===
AWS: recorded behavior only — with ObjectOwnership=ObjectWriter, ACLs are ENABLED on the bucket, so PutObjectAcl is a legal call; when the bucket's PublicAccessBlockConfiguration has BlockPublicAcls=true, a public canned ACL is refused 403 `AccessDenied`. `AccessControlListNotSupported` (400, message `The bucket does not allow ACLs`) is the answer only for ObjectOwnership=BucketOwnerEnforced.
Emulator: PutObjectAcl against the stack-deployed bucket is refused `AccessControlListNotSupported` / `The bucket does not allow ACLs` — the ObjectWriter ownership control the template set is ignored and the bucket behaves as BucketOwnerEnforced, so the public-ACL refusal the check grades (AccessDenied under BlockPublicAcls) can never be observed.
Why: a client cannot distinguish 'ACLs are turned off on this bucket' from 'this particular ACL is blocked by public-access-block', and ACL-based designs on ObjectWriter buckets are refused outright.
Fix: the PutObjectAcl handler's ownership-controls check and the bucket state that records OwnershipControls (including the path that provisions it from a template).
Traces: s3.deploy-s3-bucket-public-access-block-enforced-rejected.

## Evidence
Cause 1: `botocore.errorfactory.NoSuchBucket: An error occurred (NoSuchBucket) when calling the GetObject operation: The specified bucket does not exist` — raised on `s3.get_object(Bucket=<accesspoint ARN>, Key=...)` after `s3control.create_access_point` succeeded; expected the object bytes (200).
Cause 2: `AssertionError: DeleteObject with BypassGovernanceRetention=True removed a COMPLIANCE-retained version.` Expected 403 AccessDenied, got a successful delete.
Cause 3: `AssertionError: HeadObject reports ObjectLockMode None, expected 'GOVERNANCE' — GetObjectRetention and HeadObject disagree about the same version`.
Cause 4: `AssertionError: expected InvalidArgument for a malformed VersionId, got {'Code': 'NoSuchVersion', 'Message': 'The specified version does not exist.'}` — expected 400 InvalidArgument / `Invalid version id specified`.
Cause 5: `AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/xml', 'content-length': '113', ...}}}` for UpdateObjectEncryption with a non-resolvable/foreign SSE-KMS key ARN; expected 403 AccessDenied (`The KMS key must be from the same account as the bucket`). The sibling preconditions check ends as `Check timed out after 60.0s` without reaching its NoSuchKey/InvalidRequest/AccessDenied assertions.
Cause 6: `AssertionError: [phase 1/5 deploy] the deployed estate bucket reads {'versioning': 'Enabled', 'sse': ['AES256'], 'public_access_block': {...all true...}, 'tags': {...}, 'policy_sids': [], 'object_versions': 0}` — policy_sids must list the template's statement Sids.
Cause 7: `botocore.exceptions.ClientError: An error occurred (AccessControlListNotSupported) when calling the PutObjectAcl operation: The bucket does not allow ACLs` on a bucket deployed with `OwnershipControls.Rules[0].ObjectOwnership: ObjectWriter` and all four PublicAccessBlock flags true; expected 403 AccessDenied for the public ACL.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly these checks:
1. s3.write-get-object-response-roundtrip / -lambda-callback / -lambda-pattern / -metadata-headers-passthrough / s3.e2e-object-lambda-error-passthrough-flow — create a bucket, PutObject, s3control CreateAccessPoint over it, then GetObject with Bucket set to the access-point ARN.
2. s3.s3-object-lock-compliance-cannot-be-bypassed-while-governance-can — PutObject with ObjectLockMode=COMPLIANCE and a future RetainUntilDate, then DeleteObject(VersionId, BypassGovernanceRetention=True).
3. s3.bucket-default-retention-derives-the-retain-until-date-and-blocks-the-delete — PutObjectLockConfiguration with DefaultRetention{GOVERNANCE,Days:1}, PutObject, then compare HeadObject's ObjectLockMode with GetObjectRetention.
4. s3.get-object-nonexistent-version-errors — GetObject with a malformed VersionId on a versioned bucket.
5. s3.update-object-encryption-error-paths and s3.update-object-encryption-error-preconditions — UpdateObjectEncryption with (a) a missing key, (b) a malformed key identifier, (c) an ARN-shaped KMS key owned by another account.
6. s3.e2e-create-s3-bucket-with-config-lifecycle — deploy the bucket+policy template and read GetBucketPolicy on the deployed bucket.
7. s3.deploy-s3-bucket-public-access-block-enforced-rejected — deploy a bucket with ObjectOwnership=ObjectWriter and BlockPublicAcls=true, then PutObjectAcl with a public canned ACL.
