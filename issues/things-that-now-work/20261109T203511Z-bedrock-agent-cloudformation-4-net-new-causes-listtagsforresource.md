Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__cloudformation.kbcfn-cfn-created-kb-round-trips-config-and-stack-tags bedrock-agent__cloudformation.kbcfn-kb-drift-detection-rejected-as-unsupported-type bedrock-agent__cloudformation.kbcfn-same-name-kb-replacement-rejected-already-exists bedrock-agent__cloudformation__iam__s3__s3vectors.deploy-knowledge-base-data-source-cfn-lifecycle
Issue-Key: bedrock-agent__cloudformation|ListTagsForResource|ValidationException
Issue-Key: bedrock-agent__cloudformation.kbcfn-kb-drift-detection-rejected-as-unsupported-type|_err#4|AssertionError|raise AssertionError(f"expected a rejection, but it succeeded: {result!r}")
Issue-Key: bedrock-agent__cloudformation.kbcfn-same-name-kb-replacement-rejected-already-exists|run#5|AssertionError|assert "HandlerErrorCode: AlreadyExists" in failed[0], (
Issue-Key: bedrock-agent__cloudformation|ListDataSources|NotImplemented
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# bedrock-agent__cloudformation: 4 net-new causes — ListTagsForResource refuses a knowledge-base ARN with a foreign pattern, KB drift detection is not refused, an embedding-model change is applied in place instead of replaced, and ListDataSources is a 501 stub

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Four distinct net-new root causes on CloudFormation-provisioned AWS::Bedrock::KnowledgeBase / AWS::Bedrock::DataSource estates. Open issue 20261102T101202Z-bedrock-agent-cloudformation-2-net-new-root-causes.md covers neighbouring behaviour of the same two checks (the non-conforming knowledge-base id, which now resolves, and the accepted same-name replacement) but carries neither failure below as its replay key; none of the four is covered by it.

Cause 1 of 4: bedrock-agent ListTagsForResource refuses a knowledge-base ARN, validating the request against a pattern from a different bedrock-* service's model.
AWS: Smithy (com.amazonaws.bedrockagent#ListTagsForResource): resourceArn targets TaggableResourcesArn, httpLabel, required, length 20..1011, pattern (^arn:aws:bedrock:[a-zA-Z0-9-]+:/d{12}:(agent|agent-alias|knowledge-base|flow|prompt)/[A-Z0-9]{10}(?:/[A-Z0-9]{10})?$|...) — a knowledge-base ARN is explicitly a taggable resource; GET /tags/{resourceArn} answers 200 with the tags map (for a CFN-created knowledge base, including the aws:cloudformation:stack-name / stack-id / logical-id tags).
Emulator: the call is refused 400 ValidationException, message "1 validation error detected: Value '<resourceArn>' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[^:]+)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}:(session|advanced-optimize-prompt-job)/[a-zA-Z0-9-]+". That pattern appears nowhere in bedrock-agent's model; it is the session-scoped TaggableResourcesArn of a sibling bedrock-* service that binds the same GET /tags/{resourceArn} URI (bedrock-agent-runtime's ListTagsForResource). Hypothesis: the request is dispatched to that sibling service's ListTagsForResource instead of bedrock-agent's. Fixed text of the message is "1 validation error detected: Value '" ... "' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>"; the ARN is from the request and the pattern must be bedrock-agent's. Note also that services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/list_tags_for_resource.rs is still a SCAFFOLDED stub, so once routing/validation accept the ARN the operation answers 501 NotImplemented; both have to be closed for a knowledge base's tags to be readable.
Why: no bedrock-agent resource's tags can ever be read — a client cannot verify the tags CloudFormation propagated to a knowledge base, nor read tags it set itself, and it gets a validation error naming a resource vocabulary (session) that has nothing to do with its request.
Fix: bedrock-agent's ListTagsForResource handler (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/list_tags_for_resource.rs) plus whatever resolves GET /tags/{resourceArn} to a service — the request must be validated with bedrock-agent's TaggableResourcesArn and served from bedrock-agent state.
Traces: bedrock-agent__cloudformation.kbcfn-cfn-created-kb-round-trips-config-and-stack-tags.

Cause 2 of 4: DetectStackResourceDrift on an AWS::Bedrock::KnowledgeBase succeeds instead of being refused as an unsupported resource type.
AWS: recorded behavior only — DetectStackResourceDrift for a logical resource of type AWS::Bedrock::KnowledgeBase is refused 400 ValidationError with the message "Drift detection is not supported for ResourceType AWS::Bedrock::KnowledgeBase" (fixed text apart from the resource type, which comes from the stack's resource). Drift detection on the same stack's AWS::IAM::Role succeeds and reports IN_SYNC.
Emulator: the call answers 200 with a StackResourceDrift whose StackResourceDriftStatus is IN_SYNC, and whose ActualProperties is a strict subset of ExpectedProperties (it omits KnowledgeBaseConfiguration and StorageConfiguration entirely) — so an unsupported type is reported as in sync on a comparison that never looked at most of the declared properties.
Why: a client gets a fabricated clean drift verdict for a resource type AWS cannot assess, so real configuration drift is reported as IN_SYNC and the documented refusal that tells the client to assess the resource another way never arrives.
Fix: the DetectStackResourceDrift handler's supported-resource-type gate in the cloudformation service (refuse unsupported types with ValidationError before any comparison).
Traces: bedrock-agent__cloudformation.kbcfn-kb-drift-detection-rejected-as-unsupported-type.

Cause 3 of 4: an embedding-model change on AWS::Bedrock::KnowledgeBase is attempted as an in-place update instead of a replacement, so the stack fails with the wrong HandlerErrorCode and message.
AWS: recorded behavior only — EmbeddingModelArn inside KnowledgeBaseConfiguration.VectorKnowledgeBaseConfiguration is a replacement-triggering property: CloudFormation creates the replacement knowledge base (same Name, because the template's Name did not change) before deleting the old one, Bedrock refuses the create because the name is taken, and the resource's UPDATE_FAILED row carries a resource-handler reason of the form 'Resource handler returned message: "<name-already-exists sentence naming the knowledge base>" (RequestToken: <token>, HandlerErrorCode: AlreadyExists)'. The stack ends UPDATE_ROLLBACK_COMPLETE and both knowledge bases keep their ids and their original model.
Emulator: the stack does roll back, but each knowledge base's single UPDATE_FAILED row reads 'Resource handler returned message: "You cannot modify the Vector knowledge base configuration once it is created." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)' — the handler called UpdateKnowledgeBase on the existing knowledge base instead of creating a replacement, so the failure is an immutability refusal with HandlerErrorCode InvalidRequest, not the name-collision AlreadyExists.
Why: the failure a client sees misattributes the cause — it reads as "this property can never be changed" where AWS says "the replacement collided with the existing name", which is what tells the client to rename or stage the change. Automation that branches on HandlerErrorCode (AlreadyExists vs InvalidRequest) takes the wrong path.
Fix: the AWS::Bedrock::KnowledgeBase resource handler's update path in cloudformation — classify the embedding-model/VectorKnowledgeBaseConfiguration change as requiring replacement, run create-then-delete, and surface the create's name-collision as HandlerErrorCode: AlreadyExists.
Traces: bedrock-agent__cloudformation.kbcfn-same-name-kb-replacement-rejected-already-exists.

Cause 4 of 4: bedrock-agent ListDataSources is an unimplemented scaffold stub, so a CFN-created data source can never be listed.
AWS: Smithy (com.amazonaws.bedrockagent#ListDataSources): POST /knowledgebases/{knowledgeBaseId}/datasources/ answers 200 with the required dataSourceSummaries list, paginated by nextToken/maxResults; errors are AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400 — 501 is not among them.
Emulator: the call answers HTTP 501 with code NotImplemented and message "operation ListDataSources is scaffolded but not implemented in this emulator"; services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/list_data_sources.rs still carries the SCAFFOLDED, NOT IMPLEMENTED marker and returns Scaffolded::not_implemented("ListDataSources").
Why: the data sources of a knowledge base cannot be enumerated at all, so a client cannot discover what a stack created, cannot page them, and gets an unmodeled 501 where the SDK expects a modeled error or a result.
Fix: implement the handler in services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/list_data_sources.rs against the knowledge base's data sources in service state (including nextToken paging and the ResourceNotFoundException for an unknown knowledgeBaseId).
Traces: bedrock-agent__cloudformation__iam__s3__s3vectors.deploy-knowledge-base-data-source-cfn-lifecycle.

## Evidence
C1 (kbcfn-cfn-created-kb-round-trips-config-and-stack-tags): botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the ListTagsForResource operation: 1 validation error detected: Value 'arn:aws:bedrock:us-west-2:000000000000:knowledge-base/8EUEg85plA' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws(-[^:]+)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}:(session|advanced-optimize-prompt-job)/[a-zA-Z0-9-]+ — expected 200 and the tags map. The ARN/region/account are per-run; bedrock-agent's own model admits knowledge-base ARNs.

C2 (kbcfn-kb-drift-detection-rejected-as-unsupported-type): AssertionError: expected a rejection, but it succeeded: {'StackResourceDrift': {... 'ResourceType': 'AWS::Bedrock::KnowledgeBase', 'StackResourceDriftStatus': 'IN_SYNC', 'ExpectedProperties': '{"Name":...,"KnowledgeBaseConfiguration":{...},"StorageConfiguration":{...}}', 'ActualProperties': '{"Name":...,"Description":...,"RoleArn":...}'}, 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}. Expected (ValidationError, "Drift detection is not supported for ResourceType AWS::Bedrock::KnowledgeBase").

C3 (kbcfn-same-name-kb-replacement-rejected-already-exists): AssertionError: MetadataKB failed for a different reason: 'Resource handler returned message: "You cannot modify the Vector knowledge base configuration once it is created." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)' — expected the UPDATE_FAILED reason to carry HandlerErrorCode: AlreadyExists.

C4 (deploy-knowledge-base-data-source-cfn-lifecycle): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListDataSources operation: operation ListDataSources is scaffolded but not implemented in this emulator — expected 200 with dataSourceSummaries.

## Steps to Reproduce
C1: deploy a stack with an AWS::Bedrock::KnowledgeBase (S3_VECTORS), read the knowledge base with GetKnowledgeBase, then call bedrock-agent ListTagsForResource(resourceArn=<knowledgeBaseArn>).
C2: on the same stack, call cloudformation DetectStackResourceDrift(StackName=<stack>, LogicalResourceId=<the KB logical id>); then repeat for the stack's AWS::IAM::Role (which must succeed and report IN_SYNC).
C3: UpdateStack with the same template but a different EmbeddingModelId (e.g. titan-embed-text-v2:0 -> cohere.embed-english-v3) and an unchanged knowledge-base Name; poll DescribeStacks to UPDATE_ROLLBACK_COMPLETE and read DescribeStackEvents for each KB's UPDATE_FAILED ResourceStatusReason.
C4: deploy a stack with an AWS::Bedrock::KnowledgeBase plus an AWS::Bedrock::DataSource, then call bedrock-agent ListDataSources(knowledgeBaseId=<kb id>) (or paginate it).
