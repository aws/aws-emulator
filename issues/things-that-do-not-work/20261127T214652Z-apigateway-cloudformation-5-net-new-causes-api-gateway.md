Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation.cfn-apigw-apikey-without-name-gets-generated-name apigateway__cloudformation.deploy-cfn-documentation-version-label-change-replaces-version apigateway__cloudformation.deploy-cfn-documentation-version-restapi-change-replaces-version apigateway__cloudformation.deploy-documentation-part-properties-update-keeps-id-location-change-replaces apigateway__cloudformation.deploy-documentation-part-template-setting-read-only-id-rejected apigateway__cloudformation.deploy-cfn-documentation-part-location-change-replaces-part apigateway__cloudformation.deploy-cfn-documentation-version-label-taken-direct-create-rejected apigateway__cloudformation.deploy-client-certificate-carries-stack-system-tags apigateway__cloudformation.deploy-untag-cloudformation-system-tag-from-client-certificate-rejected
Issue-Key: apigateway__cloudformation.cfn-apigw-apikey-without-name-gets-generated-name|run#9|AssertionError|assert isinstance(name, str) and name.startswith(want_prefix) and len(name) == len(want_prefix) + 12, (
Issue-Key: apigateway__cloudformation.deploy-cfn-documentation-version-label-change-replaces-version|run#13|AssertionError|assert before == f"{first}|{api_id}", (
Issue-Key: apigateway__cloudformation.deploy-cfn-documentation-version-restapi-change-replaces-version|run#10|AssertionError|assert _physical_id(cfn, stack_id, "Snapshot") == f"{_LABEL}|{origin}", (
Issue-Key: apigateway__cloudformation.deploy-documentation-part-properties-update-keeps-id-location-change-replaces|run#16|AssertionError|assert _PART_ID.match(part_id), f"GetAtt DocumentationPartId is not a part id: {part_id!r}"
Issue-Key: apigateway__cloudformation.deploy-documentation-part-template-setting-read-only-id-rejected|run#12|AssertionError|assert _DOC_PART_ID.match(outs["PartAttr"]), (
Issue-Key: api-gateway|GetDocumentationPart|NotFoundException
Issue-Key: apigateway__cloudformation.deploy-cfn-documentation-version-label-taken-direct-create-rejected|run#10|AssertionError|assert _versions(apigw, api_id) == {_STACK_LABEL: note}, (
Issue-Key: apigateway__cloudformation.deploy-client-certificate-carries-stack-system-tags|run#16|AssertionError|assert got == want, (
Issue-Key: apigateway__cloudformation.deploy-untag-cloudformation-system-tag-from-client-certificate-rejected|run#12|AssertionError|assert before.get(_SYSTEM_KEY) == stack_name, (
Issue-Commit: 9bfc3dcca28986b1e030db2b6d5b17f08a507141

# apigateway__cloudformation: 5 net-new causes — API Gateway resources get generated-name physical ids instead of service ids, DocumentationVersion/Part are never created in the service, and CFN system tags are missing

> _Note: this issue was identified against the code base at commit `9bfc3dcca28986b1e030db2b6d5b17f08a507141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Five independent causes in how CloudFormation provisions AWS::ApiGateway::* resources. All concern resource identity (the PhysicalResourceId / Ref / GetAtt a stack publishes) and the stack tags written onto the created resource. These are net-new and distinct from the already-open 20261109T123922Z-apigateway-cloudformation-6-net-new-causes-a-not (which owns the documentation/gateway-response round trip and the usage-plan/usage-plan-key causes).

Cause 1 of 5: AWS::ApiGateway::ApiKey with no Name gets a physical name built from the full stack name.
AWS: recorded behavior only. CloudFormation's generated physical name is `<truncated stack name>-<logical id>-<12-character random suffix>`; the stack-name part is truncated so the whole name stays within the resource's limit — for a stack named `sim2real-behav-stk-<id>` with logical id `Key` the generated name has the form `sim2real-Key-<12-character suffix>` (the suffix is random per run and is not part of the requirement; the structure and the truncation are).
Emulator: the key is named `sim2real-behav-stk-c833f7805c9a-Key-6DA067E93996` — the untruncated stack name is used.
Why: a client that derives or matches the generated name, or whose name length is bounded, gets a name AWS would not have minted.
Fix: the CloudFormation physical-name generator for AWS::ApiGateway::ApiKey (stack-name truncation).
Traces: apigateway__cloudformation.cfn-apigw-apikey-without-name-gets-generated-name

Cause 2 of 5: AWS::ApiGateway::DocumentationVersion's physical id is a generated name, not `<label>|<api id>`.
AWS: recorded behavior only. The resource's PhysicalResourceId is the composite `<documentation version label>|<rest api id>`, which is what a replacement (on a label change or a RestApiId change) must swap.
Emulator: the physical id is `sim2real-xsvc-docver-a200133eff06-Snapshot-D59D04DB38AB`, a CFN-generated name, so neither the label-change nor the RestApiId-change replacement can be observed.
Why: a client that reads the stack resource to find the documentation version it must call GetDocumentationVersion with gets a value API Gateway does not know.
Fix: the AWS::ApiGateway::DocumentationVersion resource provider's physical-id composition.
Traces: apigateway__cloudformation.deploy-cfn-documentation-version-label-change-replaces-version, apigateway__cloudformation.deploy-cfn-documentation-version-restapi-change-replaces-version

Cause 3 of 5: AWS::ApiGateway::DocumentationPart's Ref/GetAtt DocumentationPartId is a generated name, not the service-assigned part id.
AWS: recorded behavior only. Ref and `GetAtt <logical id>.DocumentationPartId` both return the part id API Gateway assigned, so GetDocumentationPart(documentationPartId=<that value>) resolves; setting the read-only DocumentationPartId in the template is refused.
Emulator: the value is a CFN-generated name — `sim2real-docupd-2b84235ea01f-Part-4CB4B74ADE33` / `sim2real-docid-ok-d4eccbc87a08-Doc-D100EC8F653E` — and GetDocumentationPart with it fails `NotFoundException: Invalid Documentation part identifier specified`.
Why: every stack output or cross-resource reference that carries a documentation part id is unusable against the API Gateway control plane.
Fix: the AWS::ApiGateway::DocumentationPart resource provider — return the service-assigned part id as the physical id / DocumentationPartId attribute.
Traces: apigateway__cloudformation.deploy-documentation-part-properties-update-keeps-id-location-change-replaces, apigateway__cloudformation.deploy-documentation-part-template-setting-read-only-id-rejected, apigateway__cloudformation.deploy-cfn-documentation-part-location-change-replaces-part

Cause 4 of 5: a stack's AWS::ApiGateway::DocumentationVersion does not exist in API Gateway after CREATE_COMPLETE.
AWS: recorded behavior only. After the stack creates a documentation version, GetDocumentationVersions for the API lists that label, and a direct CreateDocumentationVersion reusing the same label is then refused as already taken.
Emulator: GetDocumentationVersions returns `[]` after create, so the stack reported success without creating the version and the duplicate-label refusal can never occur.
Why: the stack claims a resource exists that the control plane does not have, so any consumer (an export, a later stack, a direct API call) fails.
Fix: the AWS::ApiGateway::DocumentationVersion resource provider's create action — actually call CreateDocumentationVersion.
Traces: apigateway__cloudformation.deploy-cfn-documentation-version-label-taken-direct-create-rejected

Cause 5 of 5: CloudFormation system tags are not written onto AWS::ApiGateway::ClientCertificate.
AWS: recorded behavior only. A stack-created client certificate carries, in addition to the template's tags, the three system tags `aws:cloudformation:logical-id` (the resource's logical id), `aws:cloudformation:stack-id` (the stack ARN) and `aws:cloudformation:stack-name` (the stack name); an UntagResource removing a system tag is refused.
Emulator: GetClientCertificate reports only the template tags `['sim2real', 'team']` — all three `aws:cloudformation:*` tags are absent (and so the stack-name tag reads None), and therefore the untag-refusal cannot occur either.
Why: tag-based cost allocation, discovery and drift tooling that keys on the aws:cloudformation:* tags finds nothing on stack-managed certificates.
Fix: the CloudFormation resource-creation path that applies stack system tags for AWS::ApiGateway::ClientCertificate (and the tag-mutation guard that refuses removing them).
Traces: apigateway__cloudformation.deploy-client-certificate-carries-stack-system-tags, apigateway__cloudformation.deploy-untag-cloudformation-system-tag-from-client-certificate-rejected

## Evidence
Cause 1 — expected a name of the form 'sim2real-Key-<12-character suffix>', got 'sim2real-behav-stk-c833f7805c9a-Key-6DA067E93996'.
Cause 2 — "the version's physical id is 'sim2real-xsvc-docver-a200133eff06-Snapshot-D59D04DB38AB', expected '<label>|<api id>' for 'rev-a'"; and "the version's physical id must be '<label>|<api id>' of the first API".
Cause 3 — "GetAtt DocumentationPartId is not a part id: 'sim2real-docupd-2b84235ea01f-Part-4CB4B74ADE33'"; "control GetAtt DocumentationPartId is not a part id: 'sim2real-docid-ok-d4eccbc87a08-Doc-D100EC8F653E'"; botocore NotFoundException on GetDocumentationPart: 'Invalid Documentation part identifier specified'.
Cause 4 — 'versions after create: [], expected the stack's label'.
Cause 5 — "GetClientCertificate tags have keys ['sim2real', 'team'], expected ['aws:cloudformation:logical-id', 'aws:cloudformation:stack-id', 'aws:cloudformation:stack-name', 'sim2real', 'team']; system values match: [False, False, False]"; "the certificate's aws:cloudformation:stack-name tag is None, expected the stack name".

## Steps to Reproduce
1. CreateStack with an AWS::ApiGateway::ApiKey that sets no Name; read DescribeStackResource PhysicalResourceId (Cause 1).
2. CreateStack with AWS::ApiGateway::RestApi + DocumentationPart + DocumentationVersion; read DescribeStackResource PhysicalResourceId for the version and Ref/GetAtt DocumentationPartId for the part, then call GetDocumentationPart / GetDocumentationVersions with those values (Causes 2, 3, 4).
3. CreateStack with an AWS::ApiGateway::ClientCertificate carrying template tags; after CREATE_COMPLETE call GetClientCertificate and inspect its tags, then UntagResource an aws:cloudformation:* key (Cause 5).
