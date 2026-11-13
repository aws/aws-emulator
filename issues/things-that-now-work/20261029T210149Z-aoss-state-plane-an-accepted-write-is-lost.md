Issue-Source: trace-sweep-divergence
Issue-Trace: opensearchserverless-create-access-policy-idempotent-clienttoken
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearchserverless-create-access-policy-idempotent-clienttoken opensearchserverless-vpc-endpoint-errors opensearchserverless-access-policy-json-document-byte-preservation opensearchserverless-collection-lifecycle
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: aoss|GetAccessPolicy|2xx->4xx|$.accessPolicyDetail
Issue-Key: aoss|GetAccessPolicy|2xx->4xx|status
Issue-Key: aoss|DeleteAccessPolicy|2xx->4xx|status
Issue-Key: aoss|CreateVpcEndpoint|4xx->2xx|$.__type
Issue-Key: aoss|CreateVpcEndpoint|4xx->2xx|$.message
Issue-Key: aoss|CreateVpcEndpoint|4xx->2xx|status
Issue-Key: aoss|ListVpcEndpoints|2xx->2xx|$.vpcEndpointSummaries[*]
Issue-Key: aoss|GetAccessPolicy|2xx->2xx|$.accessPolicyDetail.policyVersion
Issue-Key: aoss|CreateCollection|2xx->2xx|$.createCollectionDetail.id
Issue-Commit: a04a180e27a1be47457c995aec938403d1945997

# aoss state plane: an accepted write is lost, a refused create is persisted and listed, and minted identifiers have the wrong form (6 causes)

> _Note: this issue was identified against the code base at commit `a04a180e27a1be47457c995aec938403d1945997`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccessPolicy

## Description
SIX net-new root causes in what the aoss emulator STORES and MINTS, from replaying the wire corpus simparity-wire-aws-v1: an access policy created with an idempotency token is absent from state afterwards, a VPC-endpoint create AWS refuses is accepted and then listed, and the two server-minted opaque identifiers do not have the form the recorded AWS values have. Each cause is independent and must be closed on its own.

=== Cause 1 of 6: after an idempotent-clientToken CreateAccessPolicy sequence the policy is not readable ===
AWS: Smithy: CreateAccessPolicy is @idempotent with a clientToken (@idempotencyToken, @length 1..512) and takes type + name + policy; GetAccessPolicy(type, name) is @readonly and models ResourceNotFoundException only for a policy that does not exist. Recorded behavior: after the create (and a replay of it with the SAME clientToken, which AWS answers with the original result rather than a conflict), GetAccessPolicy answers HTTP 200 with accessPolicyDetail carrying the created type, name, description, the policy document and createdDate/lastModifiedDate/policyVersion.
Emulator: GetAccessPolicy answers HTTP 400 and no accessPolicyDetail at all — the policy is absent from state even though the preceding create steps were answered as expected.
Why: an idempotent retry is exactly what an SDK does after a timeout; here it leaves the account with no access policy while the caller believes one exists, and every later read of it fails. Data access policies gate the collection data plane, so a lost one silently removes access.
Fix: CreateAccessPolicy's clientToken replay path and the access-policy state write in the opensearchserverless crate (the replayed create must leave the SAME stored policy behind, not drop or supersede it).
Traces: opensearchserverless-create-access-policy-idempotent-clienttoken#task#4.

=== Cause 2 of 6: DeleteAccessPolicy then refuses the policy that was created ===
AWS: Smithy: DeleteAccessPolicy takes type + name + clientToken and answers an empty body; it models ResourceNotFoundException only for a policy that does not exist. Recorded behavior: the teardown delete of the policy created above answers HTTP 200.
Emulator: HTTP 400 — the delete cannot find the policy either, confirming the resource is missing from state rather than merely unreadable.
Why: a caller cannot clean up what it created; a teardown that fails leaves the apparent policy quota consumed and scripts aborting on the final step.
Fix: same component as Cause 1; this step is the second observable consequence and must pass too.
Traces: opensearchserverless-create-access-policy-idempotent-clienttoken#teardown#5.

=== Cause 3 of 6: CreateVpcEndpoint accepts a request AWS refuses with ValidationException ===
AWS: Smithy: CreateVpcEndpoint requires name (@length 3..32, @pattern ^[a-z][a-z0-9-]+$), vpcId (@pattern ^vpc-[0-9a-z]*$) and subnetIds (@length min 1), and models ValidationException (400) alongside ConflictException and ServiceQuotaExceededException. Recorded behavior: the corpus' invalid create is refused HTTP 400 with __type `com.amazon.aoss#ValidationException` and a message (its text is not asserted by the fixture), and nothing is created. Hypothesis: the request names a VPC and/or subnets that do not exist in the account, which AWS validates against EC2 before accepting the endpoint.
Emulator: HTTP 200 with a createVpcEndpointDetail — no __type, no message — and the endpoint is persisted in PENDING.
Why: a caller gets a VPC endpoint that cannot work (its VPC/subnets are not real) instead of the 400 its error handling is written against, and the invalid endpoint counts against the account's endpoint quota.
Fix: the CreateVpcEndpoint handler's request validation — refuse the condition the recorded run refuses with ValidationException (400) before any state write.
Traces: opensearchserverless-vpc-endpoint-errors#task#1.

=== Cause 4 of 6: ListVpcEndpoints then reports an endpoint that should never have existed ===
AWS: Smithy: ListVpcEndpoints is @readonly and @paginated and answers vpcEndpointSummaries{id,name,status}. Recorded behavior: after the refused create, the listing answers HTTP 200 with NO vpcEndpointSummaries entry for that name.
Emulator: HTTP 200 with one summary — {id: 'vpce-<17 hex>', name: '<the refused create's name>', status: 'PENDING'}.
Why: the state the listing reports is a resource AWS never created, so any inventory, reconciliation or cleanup loop operates on a phantom endpoint; this is the stored-state half of Cause 3 and must be re-replayed after it is fixed.
Fix: the same CreateVpcEndpoint validation — nothing must be written to the VPC-endpoint store when the create is refused.
Traces: opensearchserverless-vpc-endpoint-errors#task#5.

=== Cause 5 of 6: the minted policyVersion is standard-alphabet base64, so a token can contain '/' or '+' where AWS's are alphanumeric ===
AWS: Smithy: PolicyVersion is @length 20..36 with @pattern ^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$. Recorded behavior: the AWS values this corpus records are 24-character tokens drawn from [0-9a-zA-Z] only, with no '+', '/' or padding, and the fixture asserts the member's SHAPE (not a literal, since the value differs per run). The emulator's token fails that shape assertion.
Emulator: services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/state.rs mint_policy_version() base64-encodes 18 random bytes with aws_smithy_types::base64 (the STANDARD alphabet), so a substantial fraction of tokens contain '/' or '+' — the replayed GetAccessPolicy served '<24 chars including a slash>'.
Why: policyVersion is an opaque token clients round-trip into Update*Policy/UpdateSecurityConfig, log, store in config and sometimes place in URLs and paths; a '/' inside it breaks path-safety assumptions no AWS-issued token violates, and any client or test validating the identifier's form against AWS's rejects it.
Fix: the policy-version minting in state.rs (mint_policy_version / POLICY_VERSION_BYTES) — draw the 24 characters from the alphanumeric alphabet AWS's recorded tokens use (or a URL-safe, unpadded encoding).
Traces: opensearchserverless-access-policy-json-document-byte-preservation#task#2. Corroboration (not part of this cause, and not fixable — those steps assert the recorded run's literal token, a per-run value): opensearchserverless-encryption-security-policy-lifecycle#task#4 and #task#5 show the emulator's token for that run also carrying a '/', while the recorded AWS token is 24 alphanumeric characters. The earlier wire issue 20261023T031626Z-aoss-generated-collection-ids-and-policy-versions-are.md is RESOLVED and does not cover this: it removed a hex counter, and the step still diverges.

=== Cause 6 of 6: the minted collection id does not have the form of a recorded AWS collection id ===
AWS: Smithy: CollectionId is @length 3..40 with @pattern ^[a-z0-9]{3,40}$; the recorded CreateCollection response's id is asserted by SHAPE rather than by value. The emulator's id satisfies the model's pattern yet still fails the recorded shape, so AWS mints an id of a different form inside that pattern. Hypothesis: AWS's collection id is a different fixed width than 24 characters (the id is also the host label of the collection endpoint, where AWS's recorded ids are shorter).
Emulator: state.rs mints ID_LEN = 24 characters of ID_ALPHABET = [a-z0-9] (render_id/mint_id); the replayed CreateCollection returned a 24-character lowercase-alphanumeric id and the shape assertion rejected it. Note the earlier, now-resolved issue 20261023T031626Z-aoss-generated-collection-ids-and-policy-versions-are.md replaced a constant placeholder with a random draw but kept the 24-character width, and this very step still diverges — so what remains is the token's WIDTH/form, not its entropy.
Why: the id is the primary key of DeleteCollection/BatchGetCollection, is embedded in the collection ARN and is the host label of the collection endpoint; a client, CloudFormation provider or test that validates the id's form (or an endpoint built from it) against AWS's rejects the emulator's value.
Fix: the collection/collection-group id minting in state.rs (ID_LEN and render_id/mint_id) — mint ids of the width the recorded AWS responses carry.
Traces: opensearchserverless-collection-lifecycle#setup#2.

## Evidence
All six clusters come from the simparity-wire-aws-v1 replay.
Cause 1 — opensearchserverless-create-access-policy-idempotent-clienttoken#task#4 GetAccessPolicy: expected status 200 with $.accessPolicyDetail {'type': 'data', 'name': <the created name>, 'description': 'idempotent clientToken probe', 'policy': [{'Rules': [{'ResourceType': 'collection', 'Resource': ['collection/*'], 'Permission': ['aoss:DescribeCollectionItems']}], 'Principal': ['arn:aws:iam::<account>:root']}], 'createdDate'/'lastModifiedDate'/'policyVersion' shaped}; got status 400 and $.accessPolicyDetail missing (None).
Cause 2 — opensearchserverless-create-access-policy-idempotent-clienttoken#teardown#5 DeleteAccessPolicy: expected status 200; got status 400.
Cause 3 — opensearchserverless-vpc-endpoint-errors#task#1 CreateVpcEndpoint: expected status 400 with $.__type 'com.amazon.aoss#ValidationException' and a $.message (shape-asserted); got status 200, no __type and no message.
Cause 4 — opensearchserverless-vpc-endpoint-errors#task#5 ListVpcEndpoints (200/200): expected no $.vpcEndpointSummaries[0]; got {'id': 'vpce-<17 hex>', 'name': '<the refused create's name>', 'status': 'PENDING'} (the id and the probe name are per-run values; the defect is that any entry exists).
Cause 5 — opensearchserverless-access-policy-json-document-byte-preservation#task#2 GetAccessPolicy (200/200): $.accessPolicyDetail.policyVersion expected '<shaped-not-asserted>' (a shape assertion on the identifier) and REJECTED the served value, a 24-character standard-base64 token containing '/'. Source: state.rs `mint_policy_version()` -> `::aws_smithy_types::base64::encode(&bytes[..18])` with POLICY_VERSION_BYTES = 18.
Cause 6 — opensearchserverless-collection-lifecycle#setup#2 CreateCollection (200/200): $.createCollectionDetail.id expected '<shaped-not-asserted>' and REJECTED the served 24-character [a-z0-9] id. Source: state.rs ID_LEN = 24, ID_ALPHABET = b"abcdefghijklmnopqrstuvwxyz0123456789", render_id()/mint_id().

## Steps to Reproduce
Boot the emulator from this checkout and replay the aoss wire fixtures, or with an SDK:
Causes 1-2: create_access_policy(type='data', name=<n>, description='idempotent clientToken probe', policy=<json>, clientToken=<t>); repeat the identical call with the SAME clientToken; then get_access_policy(type='data', name=<n>) -> expect 200 with the accessPolicyDetail; then delete_access_policy(type='data', name=<n>) -> expect 200. Observe 400 on both the get and the delete.
Causes 3-4: create_vpc_endpoint(name=<n>, vpcId='vpc-<id not present in the account>', subnetIds=['subnet-<id not present>']) -> expect 400 ValidationException; observe 200. Then list_vpc_endpoints() -> expect no summary for <n>; observe one with status PENDING.
Cause 5: create_access_policy(...) then get_access_policy(...) and inspect accessPolicyDetail.policyVersion over several runs -> tokens contain '/' or '+'; AWS's recorded tokens are 24 characters of [0-9a-zA-Z].
Cause 6: create_collection(name=<n>, encryptionConfig={'AWSOwnedKey': True}) and inspect createCollectionDetail.id -> a 24-character [a-z0-9] draw, which the recorded id's shape rejects.
