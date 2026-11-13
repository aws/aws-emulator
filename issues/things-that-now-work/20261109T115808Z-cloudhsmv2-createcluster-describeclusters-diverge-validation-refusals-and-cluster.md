Issue-Source: trace-sweep-divergence
Issue-Trace: cloudhsmv2-cluster-create-sdk-refused-inputs
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudhsmv2-cluster-create-sdk-refused-inputs cloudhsmv2-cluster-create-subnet-and-hsmtype-errors cloudhsmv2-cluster-create-validation-errors cloudhsmv2-cluster-lifecycle-tag-roundtrip-and-modify-rejected
Issue-Shape-Id: com.amazonaws.cloudhsmv2#BaldrApiService
Issue-Key: cloudhsmv2|CreateCluster|4xx->4xx|$.Message
Issue-Key: cloudhsmv2|CreateCluster|4xx->4xx|$.__type
Issue-Key: cloudhsmv2|CreateCluster|4xx->2xx|$.Message
Issue-Key: cloudhsmv2|CreateCluster|4xx->2xx|$.__type
Issue-Key: cloudhsmv2|CreateCluster|4xx->2xx|status
Issue-Key: cloudhsmv2|CreateCluster|2xx->2xx|$.Cluster.Certificates
Issue-Key: cloudhsmv2|CreateCluster|2xx->2xx|$.Cluster.VpcId
Issue-Key: cloudhsmv2|DescribeClusters|2xx->2xx|$.Clusters[*].Certificates
Issue-Key: cloudhsmv2|DescribeClusters|2xx->2xx|$.Clusters[*].VpcId
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# cloudhsmv2 CreateCluster/DescribeClusters diverge: validation refusals and Cluster body

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Replay of the cloudhsmv2 wire corpus (5 fixtures, 30 steps, 8 passed) clusters into 4 net-new root causes on the CreateCluster write path and the Cluster projection DescribeClusters reads back. All four belong to one fault: the CreateCluster handler neither refuses what AWS refuses nor projects the Cluster shape AWS projects. There are 4 causes in this issue and all 4 must be closed.

Cause 1 of 4 — request-shape validation refusals are answered with the wrong error type and no message (14 replayed steps, 9 in this cluster, spanning CreateCluster in three fixtures).
AWS: CloudHSM V2 (BaldrApiService, 2017-04-28, awsJson1_1) models CreateCluster's input with required members and member constraints — Smithy: CreateCluster.HsmType @required, @length(0,32), @pattern ^((p|)hsm[0-9][a-z.]*\.[a-zA-Z]+)$; CreateCluster.SubnetIds @required, @length(1,10). A request that violates the modeled input shape is refused HTTP 400 with __type CloudHsmInvalidRequestException (Smithy: CloudHsmInvalidRequestException, errorKind client, httpError 400, member Message) carrying the standard AWS input-validation message. Its fixed text is "<n> validation error(s) detected: Value <value> at '<memberName>' failed to satisfy constraint: <constraint>"; the variable parts are the error count, the offending value, the member name as the wire names it (lowerCamel, e.g. 'hsmType') and the violated constraint phrase ("Member must not be null" for a missing required member, "Member must satisfy regular expression pattern: <pattern>" / "Member must have length ..." for the others). The recorded step shows exactly that: 400, __type CloudHsmInvalidRequestException, Message "1 validation error detected: Value null at 'hsmType' failed to satisfy constraint: Member must not be null".
Emulator: answers 400 but with __type 'ValidationException' — an error this service does not model at all — and with no Message member in the body at all (replay reports $.Message missing, got None).
Why: a client cannot classify the refusal. An SDK matches __type against the operation's modeled errors, so CloudHsmInvalidRequestException is never raised and the call surfaces as an unmodeled/unknown error; retry and error-handling branches keyed on CloudHsmInvalidRequestException never run, and with no Message the caller is told nothing about which member was bad.
Fix: the shared request-shape validation stage for this service's awsJson1_1 operations (the deserialize→validate step ahead of crate::ops::create_cluster::CreateCluster::handle) must map a modeled-constraint violation onto CloudHsmInvalidRequestException with the AWS validation message built from the member and constraint, instead of emitting a generic ValidationException with an empty body.
Traces: cloudhsmv2-cluster-create-sdk-refused-inputs#task#1 (representative), plus task#2, task#3 of the same fixture; cloudhsmv2-cluster-create-subnet-and-hsmtype-errors#task#1; cloudhsmv2-cluster-create-validation-errors#task#1..task#5.

Cause 2 of 4 — CreateCluster never resolves the requested SubnetIds, so a request naming a subnet that does not exist succeeds instead of being refused.
AWS: CreateCluster.SubnetIds is the set of subnets the cluster is placed in ("The identifiers (IDs) of the subnets where you are creating the cluster... All subnets must be in the same virtual private cloud (VPC)") and the operation models CloudHsmResourceNotFoundException (Smithy: errorKind client, httpError 400, member Message) for "a resource that cannot be found". A subnet id that does not exist in the account is refused HTTP 400, __type CloudHsmResourceNotFoundException, Message "The subnet ID '<subnetId>' does not exist" — the fixed text is "The subnet ID '" ... "' does not exist" and the only variable part is the subnet id taken from the request. The second member of this cluster is the same 400→200 shape for the hsmType leg of the same fixture.
Emulator: answers HTTP 200 and mints a cluster. The handler validates HsmType against its own catalog and rejects duplicate subnet ids, but never looks a subnet id up (services/service-com-amazonaws-cloudhsmv2-baldrapiservice/src/ops/create_cluster.rs: the SubnetIds loop at lines 194-201 only checks for duplicates, and line 248 feeds the ids straight into crate::state::subnet_mapping). Replay: status expected 400, got 200; $.__type expected CloudHsmResourceNotFoundException, got None; $.Message expected the subnet text, got None.
Why: a client that relies on CreateCluster to reject a bad subnet gets back a cluster id for a cluster that could never exist, so provisioning code (and any negative test asserting the refusal) silently passes and the error is only discovered much later.
Fix: CreateCluster's validation must resolve each requested subnet id against the subnets the emulator's state knows and refuse the unknown one with CloudHsmResourceNotFoundException and AWS's message before any cluster is minted.
Traces: cloudhsmv2-cluster-create-subnet-and-hsmtype-errors#task#2 (representative), plus #task#3.

Cause 3 of 4 — the Cluster CreateCluster returns omits Certificates and carries a VpcId unrelated to the request's subnets.
AWS: Smithy: Cluster.Certificates targets the Certificates structure ("Contains one or more certificates or a certificate signing request (CSR)"); for a cluster that has not been initialized the member is still present, serialized as an empty object, and the recording confirms the expected value is {}. Smithy: Cluster.VpcId ("The identifier (ID) of the virtual private cloud (VPC) that contains the cluster") is the VPC that owns the subnets named in SubnetIds — AWS does not invent it, it reports the VPC the requested subnets belong to.
Emulator: omits Cluster.Certificates entirely (replay: expected {}, got None) because the record is built with certificates: None (create_cluster.rs line 262) and the None is dropped by the serializer; and it mints a fresh unrelated VpcId (create_cluster.rs line 239, store.mint_vpc_id()) rather than deriving it from the requested subnets, so the returned id names no VPC in state. (The literal id in the recording is environment-specific and is not the requirement; the requirement is that the member is present and that VpcId is the VPC owning the request's subnets.)
Why: a client reading cluster.Certificates on an UNINITIALIZED cluster gets a null where AWS gives an empty structure, so the normal "is ClusterCsr available yet?" poll NPEs or is skipped instead of simply finding no CSR; and the VpcId it records cannot be cross-referenced to the VPC/subnets it just used, breaking any flow that pairs the cluster with its network.
Fix: the Cluster projection (crate::state::ClusterRecord::to_shape in services/service-com-amazonaws-cloudhsmv2-baldrapiservice/src/state.rs) must always emit Certificates — empty before initialization — and the VpcId stored at creation must come from resolving SubnetIds, not from mint_vpc_id().
Traces: cloudhsmv2-cluster-lifecycle-tag-roundtrip-and-modify-rejected#task#1.

Cause 4 of 4 — DescribeClusters reads the same two defects back out of state.
AWS: Smithy: DescribeClusters output Clusters is a list of the same Cluster shape, so each element carries Certificates (an empty object for a cluster that is not yet initialized) and the VpcId that owns the cluster's subnets.
Emulator: every element omits Certificates ($.Clusters[0].Certificates expected {}, got None) and reports the invented VpcId ($.Clusters[0].VpcId mismatch), i.e. the create-time record is served unchanged on the read path, both at the first read and again after the later lifecycle steps of the fixture.
Why: the read path is where clients poll for the cluster's CSR and confirm its placement, so the gap in cause 3 is permanent rather than only visible on the create response.
Fix: same projection as cause 3 — once ClusterRecord::to_shape emits Certificates and stores a resolved VpcId, DescribeClusters' serializer needs no separate change, but it must be re-replayed to confirm.
Traces: cloudhsmv2-cluster-lifecycle-tag-roundtrip-and-modify-rejected#task#2, #task#9.

No open issue mentions cloudhsmv2 (list_existing_issues query 'cloudhsmv2' returns none), so all four causes are net-new. 8 further steps are unimplemented-operation stubs whose issues are filed separately.

## Evidence
Replay summary: trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false, 30 steps / 8 passed / 22 mismatched, score 26.7%, divergence_count 4 (all service=cloudhsmv2).

Cause 1 — cloudhsmv2-cluster-create-sdk-refused-inputs#task#1, CreateCluster, verdict mismatch, status 400 -> 400 (9 steps affected):
  expected $.__type = 'CloudHsmInvalidRequestException', actual 'ValidationException'
  expected $.Message = "1 validation error detected: Value null at 'hsmType' failed to satisfy constraint: Member must not be null", actual absent (None)

Cause 2 — cloudhsmv2-cluster-create-subnet-and-hsmtype-errors#task#2, CreateCluster, verdict mismatch (2 steps affected):
  expected status 400, actual 200
  expected $.__type = 'CloudHsmResourceNotFoundException', actual absent
  expected $.Message = "The subnet ID 'subnet-aaaaaaaaaaaaaaaa' does not exist", actual absent

Cause 3 — cloudhsmv2-cluster-lifecycle-tag-roundtrip-and-modify-rejected#task#1, CreateCluster, status 200 -> 200:
  expected $.Cluster.Certificates = {}, actual absent (None)
  expected $.Cluster.VpcId to be the VPC owning the requested subnets ('vpc-00000000000000080' as recorded), actual a freshly minted id ('vpc-149a33b9f0ea50082')

Cause 4 — cloudhsmv2-cluster-lifecycle-tag-roundtrip-and-modify-rejected#task#2 (and #task#9), DescribeClusters, status 200 -> 200:
  expected $.Clusters[0].Certificates = {}, actual absent (None)
  expected $.Clusters[0].VpcId the owning VPC, actual the minted id

Source corroboration: services/service-com-amazonaws-cloudhsmv2-baldrapiservice/src/ops/create_cluster.rs lines 194-201 (SubnetIds checked only for duplicates), line 239 (store.mint_vpc_id()), line 262 (certificates: None).

## Steps to Reproduce
1. git_clone, then replay_wire_corpus() against the built emulator.
2. Cause 1: replay fixture cloudhsmv2-cluster-create-sdk-refused-inputs step task#1 — POST / with X-Amz-Target BaldrApiService.CreateCluster and a body omitting HsmType. Observe 400 with {"__type":"ValidationException"} and no Message, instead of 400 CloudHsmInvalidRequestException with "1 validation error detected: Value null at 'hsmType' failed to satisfy constraint: Member must not be null". Same for cloudhsmv2-cluster-create-validation-errors task#1..task#5 and cloudhsmv2-cluster-create-subnet-and-hsmtype-errors task#1.
3. Cause 2: replay cloudhsmv2-cluster-create-subnet-and-hsmtype-errors step task#2 — CreateCluster with a valid HsmType and SubnetIds naming a subnet that does not exist. Observe 200 and a minted cluster instead of 400 CloudHsmResourceNotFoundException "The subnet ID '<subnetId>' does not exist". Step task#3 of the same fixture is the hsmType leg.
4. Cause 3: replay cloudhsmv2-cluster-lifecycle-tag-roundtrip-and-modify-rejected step task#1 — CreateCluster success. Inspect the response Cluster: Certificates is absent (AWS sends {}) and VpcId is an id unrelated to the requested SubnetIds.
5. Cause 4: continue the same fixture to step task#2 (and task#9) — DescribeClusters. Inspect Clusters[0]: Certificates absent and the same unrelated VpcId.
