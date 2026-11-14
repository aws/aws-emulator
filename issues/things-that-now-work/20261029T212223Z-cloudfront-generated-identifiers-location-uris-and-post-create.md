Issue-Source: trace-sweep-divergence
Issue-Trace: cfdeep-test-function-derived-output
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfdeep-test-function-derived-output cfetag-function-etag-chain cloudfront-cloudfront-function-get-workflow cloudfront-function-get-lifecycle cloudfront-function-list-lifecycle cloudfront-function-publish-and-get cfetag-connection-group-etag-chain cfetag-response-headers-policy-etag-chain cfdeep-origin-access-inuse-conflicts cfetag-origin-access-identity-etag-chain cloudfront-anycast-ip-list-lifecycle cloudfront-connection-function-publish-lifecycle
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|CreateFunction|2xx->2xx|$headers.location
Issue-Key: cloudfront|CreateConnectionGroup|2xx->2xx|$.ConnectionGroup.Arn
Issue-Key: cloudfront|CreateConnectionGroup|2xx->2xx|$.ConnectionGroup.Id
Issue-Key: cloudfront|CreateResponseHeadersPolicy|2xx->2xx|$headers.location
Issue-Key: cloudfront|GetCloudFrontOriginAccessIdentity|2xx->2xx|$.CloudFrontOriginAccessIdentity.S3CanonicalUserId
Issue-Key: cloudfront|CreateCloudFrontOriginAccessIdentity|2xx->2xx|$.CloudFrontOriginAccessIdentity.S3CanonicalUserId
Issue-Key: cloudfront|ListAnycastIpLists|2xx->2xx|$.AnycastIpListCollection.Items
Issue-Key: cloudfront|ListAnycastIpLists|2xx->2xx|$.AnycastIpListCollection.Quantity
Issue-Key: cloudfront|PublishConnectionFunction|2xx->2xx|$.ConnectionFunctionSummary.Status
Issue-Commit: e6d9764b7c2fffef8d3218e52b84d80407a24432

# CloudFront generated identifiers, Location URIs and post-create status values diverge

> _Note: this issue was identified against the code base at commit `e6d9764b7c2fffef8d3218e52b84d80407a24432`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Seven causes, one shared fault: the values CloudFront itself derives at create time - the Location URI, the generated id/ARN, the canonical user id, and the status a resource holds immediately after a create or publish - are not derived the way AWS derives them.

Cause 1 of 7: CreateFunction's Location header is built from the function NAME, not its ARN.
AWS: the Location header is `https://cloudfront.amazonaws.com/2020-05-31/function/<function ARN>`, i.e. the path segment is the full `arn:aws:cloudfront::<account>:function/<name>` (Smithy: CreateFunction output Location is an httpHeader; recorded responses carry the ARN form).
Emulator: `.../2020-05-31/function/<name>` - the bare name.
Why: a client that follows Location to address the new function builds a URI AWS would not have returned, and the ARN it needs for later calls is not discoverable from the create response.
Fix: CreateFunction's response construction (Location derivation).
Traces: cfdeep-test-function-derived-output#task#1 (also cfetag-function-etag-chain#task#1, cloudfront-cloudfront-function-get-workflow#setup#1, cloudfront-function-get-lifecycle#task#1, cloudfront-function-list-lifecycle#setup#1 and #setup#2, cloudfront-function-publish-and-get#task#1).

Cause 2 of 7: CreateConnectionGroup's generated Id/Arn do not have the shape AWS generates.
AWS: the id matches CloudFront's connection-group id form and the ARN embeds it; the fixture asserts the shape, not a literal value.
Emulator: emits `cg_7IYCGPKTANSF3` (and the matching ARN), which fails that shape assertion.
Why: clients and ARN parsers that validate the id/ARN form reject the emulator's value.
Fix: the connection-group id generator in state.
Traces: cfetag-connection-group-etag-chain#task#1.

Cause 3 of 7: CreateResponseHeadersPolicy's Location URI carries an id of the wrong shape.
AWS: `https://cloudfront.amazonaws.com/2020-05-31/response-headers-policy/<policy id>` where the id has CloudFront's response-headers-policy id form.
Emulator: the path segment is `e220a839-7b1d-cdaf-6e78-9e6aa1b965f4`, which fails the shape assertion.
Why: as cause 2; the Location is unusable for addressing the policy.
Fix: the response-headers-policy id generator / Location derivation.
Traces: cfetag-response-headers-policy-etag-chain#task#1.

Causes 4-5 (S3CanonicalUserId is not per-identity).
AWS: each origin access identity gets its own S3CanonicalUserId of the documented canonical-user-id form (Smithy: CloudFrontOriginAccessIdentity.S3CanonicalUserId is required - 'The Amazon S3 canonical user ID for the origin access identity, used when giving the origin access identity read permission to an object in Amazon S3').
Emulator: emits the SAME fixed 64-hex value for every identity, in both the create and the get response, and it fails the shape assertion.
Why: the canonical user id is what an S3 bucket policy grants read to; one shared value means two identities are indistinguishable to S3, so per-identity access control cannot be modelled.
Fix: the origin-access-identity id/canonical-user-id generator in state.
Cause 4 of 7: GetCloudFrontOriginAccessIdentity - traces cfdeep-origin-access-inuse-conflicts#task#7.
Cause 5 of 7: CreateCloudFrontOriginAccessIdentity - traces cfetag-origin-access-identity-etag-chain#task#1.

Cause 6 of 7: ListAnycastIpLists includes a list AWS does not yet list.
AWS: immediately after CreateAnycastIpList the collection is still reported with Quantity 0 and an empty Items element - a list in the initial post-create state is not part of the collection yet.
Emulator: Quantity 1 with the just-created Deploying list present.
Why: a client that polls the list until the new entry appears cannot distinguish "created" from "visible", and inventory counts differ from AWS right after a create.
Fix: ListAnycastIpLists' state query (post-create visibility).
Traces: cloudfront-anycast-ip-list-lifecycle#task#3.

Cause 7 of 7: PublishConnectionFunction reports the wrong status.
AWS: 200 with ConnectionFunctionSummary.Status `PUBLISHING` - publishing is asynchronous, so the immediate status is the in-progress one (Smithy: ConnectionFunctionSummary.Status is a required string).
Emulator: Status `UNASSOCIATED`.
Why: a client that waits for PUBLISHING to settle never sees the transition, and UNASSOCIATED tells it the publish did not take effect.
Fix: PublishConnectionFunction's handler/state transition.
Traces: cloudfront-connection-function-publish-lifecycle#task#5.

## Evidence
1. CreateFunction 201->201: $headers.location expected 'https://cloudfront.amazonaws.com/2020-05-31/function/arn:aws:cloudfront::<account>:function/<name>', got 'https://cloudfront.amazonaws.com/2020-05-31/function/<name>' (7 affected steps).
2. CreateConnectionGroup 201->201: $.ConnectionGroup.Id and $.ConnectionGroup.Arn both fail the shape assertion; emitted 'cg_7IYCGPKTANSF3' / 'arn:aws:cloudfront::<account>:connection-group/cg_7IYCGPKTANSF3'.
3. CreateResponseHeadersPolicy 201->201: $headers.location shape_mismatch, emitted '.../response-headers-policy/e220a839-7b1d-cdaf-6e78-9e6aa1b965f4'.
4/5. Get/CreateCloudFrontOriginAccessIdentity 200/201: $.CloudFrontOriginAccessIdentity.S3CanonicalUserId fails the shape assertion, and BOTH clusters - different fixtures, different identities - emit the identical value '6e789e6aa1b965f406c45d188009454ff88bb8a8724c81ec1b39896a51a8749b', which is what shows it is a constant rather than per-identity.
6. ListAnycastIpLists 200->200: $.AnycastIpListCollection.Quantity expected '0' got '1'; Items expected empty, got one AnycastIpListSummary (Status 'Deploying').
7. PublishConnectionFunction 200->200: $.ConnectionFunctionSummary.Status expected 'PUBLISHING', got 'UNASSOCIATED'.

The account id, generated ids and timestamps quoted above are per-run; the requirements are the Location's ARN form, the id/ARN/canonical-user-id shapes, post-create list invisibility and the PUBLISHING status.

## Steps to Reproduce
1. Replay the wire corpus against the emulator.
2. Cause 1: CreateFunction and read the Location response header - it ends in the bare name, not the function ARN.
3. Causes 2-3: CreateConnectionGroup and CreateResponseHeadersPolicy and check the generated Id/Arn and Location id against CloudFront's id forms.
4. Causes 4-5: CreateCloudFrontOriginAccessIdentity twice and compare S3CanonicalUserId - the same value is returned for both identities.
5. Cause 6: CreateAnycastIpList then ListAnycastIpLists immediately; the new list is already counted.
6. Cause 7: CreateConnectionFunction then PublishConnectionFunction and read ConnectionFunctionSummary.Status.
7. Representative step: fixture cfdeep-test-function-derived-output, step task#1 (CreateFunction).
