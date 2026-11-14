Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearchserverless.deleting-network-policy-stops-the-live-collection-endpoint-serving
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: opensearchserverless.deleting-network-policy-stops-the-live-collection-endpoint-serving|run#28|AssertionError|assert stopped[1] == {"raw": ""}, (
Issue-Commit: e939f3162fad9baaa59a21c0fa7179928b05c50a

# opensearchserverless: the collection data plane's no-matching-network-policy refusal returns 401 with an OpenSearch security_exception JSON body instead of an empty body, so a network refusal is indistinguishable from the permission layer's

> _Note: this issue was identified against the code base at commit `e939f3162fad9baaa59a21c0fa7179928b05c50a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSecurityPolicy

## Description
ONE net-new root cause on the OpenSearch Serverless collection DATA plane, grounded in one failing behavioral check of the opensearchserverless service token.

Progress note (not a duplicate): open issue 20261108T223024Z-opensearchserverless-3-net-new-causes-on-the-collection.md, 'Cause 2 of 3', describes an EARLIER wall — network policies were not evaluated at all, so deleting the only matching network policy left the endpoint serving and the check died on its 180s budget with no refusal ever produced. That wall is gone in this replay: the control-plane delete is accepted, GetSecurityPolicy answers ResourceNotFoundException with the exact AWS message, the endpoint DOES start refusing both the read and the write, it refuses with HTTP 401 for three consecutive stabilisation probes, the collection stays ACTIVE and the published collectionEndpoint is unchanged. What fails now is the SHAPE of that 401's body, which is new behavior behind that wall; the suite marked this check neither ALREADY FILED nor OWNED by that issue (it owns the sibling check opensearchserverless.server-side-network-policy-enforces-vpc-only-access, whose refusal still never arrives). If that issue is reworked to also fix the refusal body, this closes with it.

=== Cause 1 of 1: the 401 a collection endpoint returns when NO network policy matches carries a JSON error document instead of an empty body ===
Failing check: opensearchserverless.deleting-network-policy-stops-the-live-collection-endpoint-serving

AWS: recorded behavior only — the aoss collection data plane is not Smithy-modeled. The control-plane writer that produces the state is Smithy-modeled: DeleteSecurityPolicy(type: SecurityPolicyType required, name: PolicyName required pattern ^[a-z][a-z0-9-]+$ length 3-32, clientToken), errors ConflictException/409, ResourceNotFoundException/404, ValidationException/400, InternalServerException/500; Smithy documents CollectionDetail.collectionEndpoint as the 'Collection-specific endpoint used to submit index, search, and data upload requests'. Recorded: deleting the only network policy an ACTIVE collection matches is permitted by the control plane, and the collection endpoint then refuses even a fully authorised, correctly aoss-scoped, data-access-granted request with HTTP 401 and an EMPTY response body — zero bytes, no JSON object at all. The empty body is the discriminator for this refusal class: with no matching network policy the request is rejected at the network front door, BEFORE aoss builds any error document, which is what distinguishes it from the two refusals that DO carry a body — the front-door JSON 403 whose error.type is the fixed text 'forbidden' (wrong SigV4 signing scope, or missing aoss:APIAccessAll) and OpenSearch's own permission-layer refusal, whose error.type is the fixed text 'security_exception'. The HTTP status 401 and the empty body are the requirement; the collection name, index name, document id and vpce id in the request are request/state values and appear nowhere in the response.

Emulator: the status is right (401, observed stable across three consecutive probes of both a read and a write) but the response carries a body, and that body is an OpenSearch security_exception document: {'error': {'root_cause': [{'type': 'security_exception', 'reason': 'Unauthorized'}], 'type': 'security_exception', 'reason': 'Unauthorized'}, 'status': 401}. So the emulator answers a network-boundary rejection with the body of the permission layer, and with a 401 carrying a body at all, which on AWS never happens for this class.

Why: every OpenSearch client and every operator runbook classifies a data-plane failure by the error document. A client that branches on the refusal class takes the wrong branch: an empty 401 means 'no network policy admits you — fix the network policy / reach the collection from an admitted VPC endpoint', while error.type == 'security_exception' means 'you reached the collection and OpenSearch's permission layer denied you — fix the data access policy'. Code that retries after re-signing or after editing a data access policy will loop forever against a collection that has no network policy at all, and code that asserts the body is empty (or that no 'error' key is present) to confirm the endpoint has been taken out of service by removing its network policy sees the opposite signal. It also makes the emulator's three refusal paths (network 401 empty, front-door 403 'forbidden', permission 'security_exception') collapse into one indistinguishable shape.

Fix: the collection data-plane front door in the opensearchserverless service crate — specifically the network-policy decision's refusal path and the data-plane error serializer it hands off to. When no network security policy matches the target collection (or the caller arrives from outside every admitted SourceVPCEs entry), the response must be HTTP 401 with a zero-length body, not a serialized OpenSearch error document; only the authorization refusals below it may emit a body, and those must use their own fixed error.type ('forbidden' for the front door, 'security_exception' for the permission layer).

Traces: opensearchserverless.deleting-network-policy-stops-the-live-collection-endpoint-serving.

## Evidence
Cause 1 — opensearchserverless.deleting-network-policy-stops-the-live-collection-endpoint-serving [mismatch]:
  Expected (AWS): after DeleteSecurityPolicy(type='network', name=<n>) removes the only network policy the ACTIVE collection matches, the identical previously-served, aoss-signed, data-access-authorised GET /<index>/_doc/<id> and PUT /<index>/_doc/<id> are refused with HTTP 401 and an EMPTY body (the check models this as body {'raw': ''}), while the collection stays ACTIVE and its collectionEndpoint is unchanged.
  Actual (emulator): HTTP 401 (correct, and stable for the required three consecutive probes of both read and write) but the body is a JSON OpenSearch error document. Suite detail: `AssertionError: expected the 401 for a request with no matching network policy to carry an empty body (a 401 WITH a JSON error body would mean a different refusal path), got {'error': {'root_cause': [{'type': 'security_exception', 'reason': 'Unauthorized'}], 'type': 'security_exception', 'reason': 'Unauthorized'}, 'status': 401}`.
  Everything the check asserts before that point now passes: the delete is accepted, GetSecurityPolicy answers ResourceNotFoundException with the exact message `Policy with name <name> and type network is not found`, and the endpoint stops serving within the stabilisation window — which is why this body-shape divergence is newly visible.
  Note on fixed text vs values: 'security_exception' / 'Unauthorized' are the emulator's fixed text (no part of them derives from the request); the collection name, index name and document ids in the reproduction are this run's values and are not part of any requirement.

## Steps to Reproduce
1. create_security_policy(type='encryption', name=<e>, policy matching collection/<name>, aWSOwnedKey=true).
2. Create an OpenSearch Serverless VPC endpoint <vpce> in the runner's VPC (create_vpc_endpoint + poll batch_get_vpc_endpoint until ACTIVE), its security group admitting the runner on 443.
3. create_security_policy(type='network', name=<n>, policy=[{Rules:[{ResourceType:'collection', Resource:['collection/<name>']}], AllowFromPublic:false, SourceVPCEs:['<vpce>']}]).
4. Create an IAM role holding aoss:APIAccessAll scoped to the collection, assume it, and sign every data-plane request SigV4 with signing service name 'aoss', adding x-amz-content-sha256 on body-bearing requests. create_access_policy(type='data', name=<d>) granting that role the collection and index permissions.
5. create_collection(name=<name>, type='SEARCH'); poll batch_get_collection(ids=[<id>]) until ACTIVE and read collectionDetails[0].collectionEndpoint as <base>.
6. Positive control: PUT <base>/<index>/_doc/1 {'field':'before-outage'} -> accepted; GET <base>/<index>/_doc/1 -> 200 with the stored _source.
7. delete_security_policy(type='network', name=<n>); confirm get_security_policy(type='network', name=<n>) -> ResourceNotFoundException.
8. Re-issue the identical authorised GET <base>/<index>/_doc/1 and PUT <base>/<index>/_doc/stop-probe every 10s until both are refused on three consecutive probes, and inspect the refusal.
   -> EXPECTED: HTTP 401 with a zero-length body (no JSON object).
   -> ACTUAL: HTTP 401 with {'error': {'root_cause': [{'type': 'security_exception', 'reason': 'Unauthorized'}], 'type': 'security_exception', 'reason': 'Unauthorized'}, 'status': 401}.
9. (Unaffected controls, both correct today) batch_get_collection still reports status ACTIVE and the same collectionEndpoint; re-creating the byte-identical network policy makes the endpoint serve again and the pre-outage document reads back unchanged.
