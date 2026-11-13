Issue-Source: trace-sweep-divergence
Issue-Trace: mediaconnect-flow-source-metadata-and-thumbnail
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediaconnect-flow-source-metadata-and-thumbnail mediaconnect-flow-vpc-interfaces
Issue-Shape-Id: com.amazonaws.mediaconnect#MediaConnect
Issue-Key: mediaconnect|DescribeFlowSourceMetadata|4xx->2xx|$.message
Issue-Key: mediaconnect|DescribeFlowSourceMetadata|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mediaconnect|DescribeFlowSourceMetadata|4xx->2xx|status
Issue-Key: mediaconnect|DescribeFlowSourceThumbnail|4xx->2xx|$.message
Issue-Key: mediaconnect|DescribeFlowSourceThumbnail|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mediaconnect|DescribeFlowSourceThumbnail|4xx->2xx|status
Issue-Key: mediaconnect|AddFlowVpcInterfaces|4xx->2xx|$.message
Issue-Key: mediaconnect|AddFlowVpcInterfaces|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: mediaconnect|AddFlowVpcInterfaces|4xx->2xx|status
Issue-Commit: bcbe4ab72d6cacb413c4017b750730a99b5d228f

# MediaConnect flow operations accept requests AWS refuses with BadRequestException (source-metadata, source-thumbnail, VPC-interface role)

> _Note: this issue was identified against the code base at commit `bcbe4ab72d6cacb413c4017b750730a99b5d228f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFlowSourceMetadata

## Description
Three net-new wire divergences from the replay of trace set simparity-wire-aws-v1 against the MediaConnect emulator, all on service `mediaconnect`. They share one root fault: a precondition AWS enforces before doing the work is not enforced at all, so the emulator answers a 2xx success (and, for AddFlowVpcInterfaces, mutates the flow) where AWS answers HTTP 400 BadRequestException. Each cause must be closed separately before this issue is submitted.

=== Cause 1 of 3: DescribeFlowSourceMetadata answers 200 on a flow that is not ACTIVE instead of refusing it ===
AWS: Smithy: DescribeFlowSourceMetadata, GET /v1/flows/{FlowArn}/source-metadata, success code 200, output {flowArn, messages, timestamp, transportMediaInfo, ndiInfo}; modelled client errors include BadRequestException (httpError 400). Recorded behavior: the operation is gated on the flow's own Status. While the flow is not ACTIVE the call is refused with HTTP 400, error type BadRequestException, message `Bad request: The flow must be in ACTIVE state to perform this action`. Every word of that message is fixed text for this error class: it interpolates nothing from the request or from state (no ARN, no flow name, no status value). The same call succeeds with 200 once the flow is ACTIVE.
Emulator: the step answered HTTP 200 with no `message` member and no `x-amzn-errortype` header at all (replay: `status: expected 400, got 200`; `$.message: missing`; `$headers.x-amzn-errortype: missing`), i.e. the non-ACTIVE flow is treated as a readable source and metadata is projected for it.
Why: a client cannot branch on BadRequestException to learn that it must start the flow first. It receives a success it must then re-interpret from free-form response content, and any client that treats a 200 as "source metadata is available" reads metadata for a flow that is not ingesting.
Fix: the DescribeFlowSourceMetadata handler's validation — refuse with the modelled BadRequestException, carrying AWS's fixed message for this class, whenever the stored flow's status is not ACTIVE, before projecting any metadata. Related open issue 20261029T202226Z-mediaconnect-deletebridge-erases-the-bridge-instead-of-retiring.md describes the same emulator behavior as its Cause 2 from the behavioral corpus; the replay report did not mark this wire step as owned by it, so the wire binding is recorded here.
Traces: mediaconnect-flow-source-metadata-and-thumbnail#task#2.

=== Cause 2 of 3: DescribeFlowSourceThumbnail answers 200 instead of refusing a flow whose state or ThumbnailState does not allow a thumbnail read ===
AWS: Smithy: DescribeFlowSourceThumbnail, GET /v1/flows/{FlowArn}/source-thumbnail, success code 200, output {thumbnailDetails}; modelled client errors include BadRequestException (httpError 400). Recorded behavior: the operation enforces two preconditions together and reports both in one refusal — HTTP 400, error type BadRequestException, message `Validation failed:\nTo perform this action, the flow must be in ACTIVE or UPDATING state.\nThis action can only be performed on a flow with ThumbnailState in SourceMonitoringConfiguration set to ENABLED.` The `Validation failed:` envelope line and both following sentences are fixed text for this error class; nothing in the message is taken from the request or from state.
Emulator: the step answered HTTP 200 with no `message` member and no `x-amzn-errortype` header (replay: `status: expected 400, got 200`; `$.message: missing`; `$headers.x-amzn-errortype: missing`), so a flow that is neither ACTIVE nor UPDATING, and whose SourceMonitoringConfiguration.ThumbnailState is not ENABLED, is served as if thumbnails were available.
Why: thumbnail polling loops rely on the refusal to know that monitoring must be enabled (or the flow started) first; against the emulator they instead get a 200 whose ThumbnailDetails cannot be meaningful, and the configuration mistake is never surfaced.
Fix: the DescribeFlowSourceThumbnail handler's validation — check the stored flow's status against {ACTIVE, UPDATING} and its SourceMonitoringConfiguration.ThumbnailState against ENABLED, and emit the modelled BadRequestException with AWS's two-sentence fixed text under the `Validation failed:` envelope when either precondition fails.
Traces: mediaconnect-flow-source-metadata-and-thumbnail#task#3.

=== Cause 3 of 3: AddFlowVpcInterfaces attaches a VPC interface whose RoleArn names a role that cannot be assumed, answering 201 instead of refusing it ===
AWS: Smithy: AddFlowVpcInterfaces, POST /v1/flows/{FlowArn}/vpcInterfaces, success code 201, input {FlowArn @required @httpLabel, VpcInterfaces @required}, output {flowArn, vpcInterfaces}; modelled client errors include BadRequestException (httpError 400). Recorded behavior: before creating the interface MediaConnect tries to assume the supplied VpcInterfaceRequest.RoleArn, and when that fails the whole request is refused with HTTP 400, error type BadRequestException, and the message `Bad request: Unable to assume role: <RoleArn> Add mediaconnect.amazonaws.com as a trusted entity and try again.` Here `Bad request: Unable to assume role: ` and ` Add mediaconnect.amazonaws.com as a trusted entity and try again.` are the fixed text of this error class, and the only variable part is the role ARN echoed verbatim from the request's VpcInterfaces[].RoleArn (in the recording that happened to be a probe role ARN; the ARN itself is a request value, not part of the requirement). No VPC interface is added and the flow is unchanged.
Emulator: the step answered HTTP 201 with no `message` member and no `x-amzn-errortype` header (replay: `status: expected 400, got 201`; `$.message: missing`; `$headers.x-amzn-errortype: missing`), i.e. the interface was created against an unassumable role.
Why: the flow is left holding a VPC interface backed by a role MediaConnect could never use, so the misconfiguration surfaces later (or never) instead of at the call that caused it, and a client that relies on the 400 to validate its IAM wiring concludes the trust policy is correct. It also makes the emulator's stored flow diverge from any real account's.
Fix: the AddFlowVpcInterfaces handler's validation — resolve each requested VpcInterfaces[].RoleArn against IAM state and refuse the request with the modelled BadRequestException and AWS's fixed assume-role text (role ARN interpolated from the request) before any interface is stored. The same unchecked-reference pattern on a sibling operation is open as 20261024T132820Z-mediaconnect-createrouternetworkinterface-validation.md, which does not cover this operation or the RoleArn check.
Traces: mediaconnect-flow-vpc-interfaces#task#2.

## Evidence
Replay of simparity-wire-aws-v1: 8 fixtures, 50 steps, 45 passed, 5 mismatched (score 90%), emulator_up=true, bootstrap_signal=false. All three causes below are `verdict: mismatch` clusters with empty `stub_path` and empty `owned_by`.

Cause 1 — fixture mediaconnect-flow-source-metadata-and-thumbnail, step task#2, operation DescribeFlowSourceMetadata:
  expected_status 400, actual_status 200
  detail: status: expected 400, got 200 | $.message: missing (expected 'Bad request: The flow must be in ACTIVE state to perform this action', got None) | $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  divergence_key: service=mediaconnect|DescribeFlowSourceMetadata|status=400->200|keys=$.message,$headers.x-amzn-errortype,status

Cause 2 — fixture mediaconnect-flow-source-metadata-and-thumbnail, step task#3, operation DescribeFlowSourceThumbnail:
  expected_status 400, actual_status 200
  detail: status: expected 400, got 200 | $.message: missing (expected 'Validation failed:\nTo perform this action, the flow must be in ACTIVE or UPDATING state.\nThis action can only be performed on a flow with ThumbnailState in SourceMonitoringConfiguration set to ENABLED.', got None) | $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  divergence_key: service=mediaconnect|DescribeFlowSourceThumbnail|status=400->200|keys=$.message,$headers.x-amzn-errortype,status

Cause 3 — fixture mediaconnect-flow-vpc-interfaces, step task#2, operation AddFlowVpcInterfaces:
  expected_status 400, actual_status 201
  detail: status: expected 400, got 201 | $.message: missing (expected 'Bad request: Unable to assume role: arn:aws:iam::000000000000:role/sim2real-probe-nonexistent-role Add mediaconnect.amazonaws.com as a trusted entity and try again.', got None) | $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  divergence_key: service=mediaconnect|AddFlowVpcInterfaces|status=400->201|keys=$.message,$headers.x-amzn-errortype,status
  (the role ARN in that message is the request's own RoleArn value; only the surrounding text is the requirement)

Model facts used (services/service-com-amazonaws-mediaconnect-mediaconnect/smithy-model/com-amazonaws-mediaconnect-mediaconnect.json): all three operations declare BadRequestException with httpError 400; success codes are 200, 200 and 201 respectively.

## Steps to Reproduce
Boot the emulator and replay the wire corpus (replay_wire_corpus), or drive a botocore `mediaconnect` client at the emulator endpoint:

Cause 1: f = mc.create_flow(Name='f1', Source={'Name':'source1','Protocol':'rtp','WhitelistCidr':'203.0.113.0/24'})['Flow']['FlowArn'] (flow is not ACTIVE); mc.describe_flow_source_metadata(FlowArn=f) -> observe HTTP 200 instead of HTTP 400 BadRequestException with message 'Bad request: The flow must be in ACTIVE state to perform this action'. Fixture mediaconnect-flow-source-metadata-and-thumbnail step task#2.

Cause 2: with the same non-ACTIVE flow (and SourceMonitoringConfiguration.ThumbnailState not ENABLED), mc.describe_flow_source_thumbnail(FlowArn=f) -> observe HTTP 200 instead of HTTP 400 BadRequestException with message 'Validation failed:\nTo perform this action, the flow must be in ACTIVE or UPDATING state.\nThis action can only be performed on a flow with ThumbnailState in SourceMonitoringConfiguration set to ENABLED.'. Fixture mediaconnect-flow-source-metadata-and-thumbnail step task#3.

Cause 3: mc.add_flow_vpc_interfaces(FlowArn=f, VpcInterfaces=[{'Name':'vpc1','RoleArn':'arn:aws:iam::<account>:role/<a role that does not exist>','SecurityGroupIds':['sg-0123456789abcdef0'],'SubnetId':'subnet-0123456789abcdef0'}]) -> observe HTTP 201 with the interface created, instead of HTTP 400 BadRequestException with message 'Bad request: Unable to assume role: <that RoleArn> Add mediaconnect.amazonaws.com as a trusted entity and try again.'; then mc.describe_flow(FlowArn=f) and observe the interface present where AWS leaves the flow unchanged. Fixture mediaconnect-flow-vpc-interfaces step task#2.
