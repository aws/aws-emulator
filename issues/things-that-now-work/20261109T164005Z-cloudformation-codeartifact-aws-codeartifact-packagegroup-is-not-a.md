Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__codeartifact.deploy-codeartifact-domain-repo-packagegroup-round-trip cloudformation__codeartifact.deploy-codeartifact-packagegroup-invalid-pattern-rejected
Issue-Key: cloudformation__codeartifact|CreateStack|ValidationError
Issue-Commit: 0cee419aadfb4868f9ccd3aa23bc5045ac08a29b

# cloudformation__codeartifact: AWS::CodeArtifact::PackageGroup is not a registered resource type — CreateStack refuses the template with ValidationError "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `0cee419aadfb4868f9ccd3aa23bc5045ac08a29b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the emulator's CloudFormation resource-type registry has no AWS::CodeArtifact::PackageGroup, so CreateStack rejects any template containing one at template-format validation time. Both CloudFormation+CodeArtifact checks die on their very first CreateStack, so neither the successful PackageGroup deployment nor the handler-side rejection path is ever exercised.

AWS: Smithy (com.amazonaws.codeartifact#CodeArtifactControlPlaneService) models the resource this type provisions — CreatePackageGroup, POST /v1/package-group, success 200; required inputs `domain` (DomainName) and `packageGroup` (PackageGroupPattern, length 2–520, pattern ^[^\p{C}\p{IsWhitespace}]+$), optional contactInfo/description/tags; output packageGroup (PackageGroupDescription, carrying pattern, domainName and arn); modeled errors ValidationException 400, ConflictException 409, ResourceNotFoundException 404, AccessDeniedException 403, ServiceQuotaExceededException 402, ThrottlingException 429, InternalServerException 500. AWS::CodeArtifact::PackageGroup is a real, supported CloudFormation resource type (properties DomainName, Pattern, ContactInfo, Description, Tags; Ref returns the package group's ARN), so on AWS CreateStack accepts the template and answers 200 with a StackId. Recorded behavior for the two paths the checks assert: (a) a template with Domain + PackageGroup(Pattern="/pypi/*") reaches CREATE_COMPLETE, DescribePackageGroup(domain, packageGroup="/pypi/*") reads back pattern="/pypi/*", domainName=<the stack's domain> and arn equal to the stack output `{"Ref": "PackageGroup"}`; (b) a template whose Pattern lacks a leading '/' passes template validation (no early validation rows) and fails in the resource handler, so the stack ends ROLLBACK_COMPLETE, the PackageGroup resource's ResourceStatusReason is the registry-handler form `Resource handler returned message: "<message>" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)` whose message is CodeArtifact's own ValidationException text `1 validation error detected: Value '<the submitted pattern>' at 'packageGroup' failed to satisfy constraint: Pattern must start with '/'. (Service: Codeartifact, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)`, and the rolled-back domain is afterwards absent (DescribeDomain refused with `Domain not found. Domain '<domain>' owned by account '<account>' does not exist.`). In those messages the fixed text is everything outside the angle brackets; the submitted pattern, domain name, account, request token/id and attempt count are values from the request, the state and the run.

Emulator: CreateStack is refused outright with HTTP 400 `ValidationError` and message `Template format error: Unrecognized resource types: [AWS::CodeArtifact::PackageGroup]`. No stack is created, so no CREATE_COMPLETE, no Outputs, no rollback and no handler reason exist.

Why: a client deploying CodeArtifact package groups as infrastructure-as-code — the normal way they are managed — cannot deploy at all, and gets a template-format error that falsely says the type does not exist in CloudFormation. A client testing a bad Pattern gets the wrong failure class too: an early template-validation ValidationError on CreateStack instead of an accepted stack that rolls back with the handler's InvalidRequest reason, so error handling built around stack events and HandlerErrorCode is never reached.

Fix: the CloudFormation service crate's resource-type registry/provisioning table must register AWS::CodeArtifact::PackageGroup and provide its create/read/update/delete handler (delegating to the codeartifact crate's CreatePackageGroup/DescribePackageGroup/DeletePackageGroup, surfacing CodeArtifact's ValidationException through the standard `Resource handler returned message: "..." (RequestToken: ..., HandlerErrorCode: InvalidRequest)` reason), and `Ref` must resolve to the package group ARN. A sibling open issue records the same registry gap for other types and does not cover CodeArtifact: 20261109T115129Z-cloudformation-ec2-neptune-graph-cloudformation-aws-neptunegraph-graph-and-aws-neptunegraph-privategraphendpoint.md.

Traces: cloudformation__codeartifact.deploy-codeartifact-domain-repo-packagegroup-round-trip and cloudformation__codeartifact.deploy-codeartifact-packagegroup-invalid-pattern-rejected.

## Evidence
[error] cloudformation__codeartifact.deploy-codeartifact-domain-repo-packagegroup-round-trip
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CodeArtifact::PackageGroup]

[error] cloudformation__codeartifact.deploy-codeartifact-packagegroup-invalid-pattern-rejected
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CodeArtifact::PackageGroup]

Template submitted (both checks): Resources = { Domain: AWS::CodeArtifact::Domain {DomainName}, PackageGroup: AWS::CodeArtifact::PackageGroup {DomainName, Pattern} with DependsOn Domain }, Outputs = { PackageGroupRef: {"Ref": "PackageGroup"} }.

Expected (valid Pattern "/pypi/*"): CreateStack -> 200 with a StackId; stack reaches CREATE_COMPLETE; DescribePackageGroup returns pattern="/pypi/*", domainName=<stack domain>, arn == Outputs["PackageGroupRef"].
Expected (Pattern without a leading '/'): CreateStack accepted (no early template-validation rows); stack ends ROLLBACK_COMPLETE; the PackageGroup event's ResourceStatusReason == 'Resource handler returned message: "1 validation error detected: Value \'<submitted pattern>\' at \'packageGroup\' failed to satisfy constraint: Pattern must start with \'/\'. (Service: Codeartifact, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)'; afterwards DescribeDomain on the rolled-back domain is refused with 'Domain not found. Domain \'<domain>\' owned by account \'<account>\' does not exist.'
Actual (both): HTTP 400 ValidationError, 'Template format error: Unrecognized resource types: [AWS::CodeArtifact::PackageGroup]' — no stack, no events, no handler reason.

Stack names, domain names, request ids/tokens and the account id are per-run values, not requirements.

## Steps to Reproduce
1. Boot the emulator.
2. cloudformation.CreateStack(StackName=<name>, TemplateBody = {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Domain":{"Type":"AWS::CodeArtifact::Domain","Properties":{"DomainName":"<d>"}},"PackageGroup":{"Type":"AWS::CodeArtifact::PackageGroup","DependsOn":"Domain","Properties":{"DomainName":"<d>","Pattern":"/pypi/*"}}},"Outputs":{"PackageGroupRef":{"Value":{"Ref":"PackageGroup"}}}}).
3. Observe the call is refused with 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::CodeArtifact::PackageGroup]' instead of returning a StackId.
4. Repeat with Pattern="not-a-valid-pattern-format": the same early refusal occurs, so the stack never rolls back with the handler's InvalidRequest reason.
