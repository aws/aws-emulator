Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rekognition.put-project-policy-malformed-document-rejected
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition.put-project-policy-malformed-document-rejected|run#10|AssertionError|assert code_bad_json == "MalformedPolicyDocumentException", (
Issue-Commit: 83b3595983c9b1b55d7eff59be20aa7240ef9e4c

# Rekognition PutProjectPolicy answers 501 NotImplemented instead of validating the policy document (MalformedPolicyDocumentException)

> _Note: this issue was identified against the code base at commit `83b3595983c9b1b55d7eff59be20aa7240ef9e4c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutProjectPolicy

## Description
Cause 1 of 1: `PutProjectPolicy` is an unimplemented scaffold stub, so every call — valid or invalid — is refused with a 501 `NotImplemented` and the operation's whole policy-document validation surface is absent. (Failing check: rekognition.put-project-policy-malformed-document-rejected.)

AWS: Rekognition models `PutProjectPolicy` with required `ProjectArn` (ProjectArn, @length 20..2048, @pattern ^(^arn:[a-z\d-]+:rekognition:[a-z\d-]+:\d{12}:project\/[a-zA-Z0-9_.\-]{1,255}\/[0-9]+$)$), required `PolicyName` (ProjectPolicyName, @length 1..128, @pattern ^[a-zA-Z0-9_.\-]+$), required `PolicyDocument` (ProjectPolicyDocument, @length 1..2000, @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$) and optional `PolicyRevisionId` (ProjectPolicyRevisionId, @length 0..64, @pattern ^[0-9A-Fa-f]+$); the success response carries `PolicyRevisionId`. Smithy: PutProjectPolicy declares `MalformedPolicyDocumentException` (client error, HTTP 400, doc: "The format of the project policy document that you supplied to PutProjectPolicy is incorrect.") alongside AccessDeniedException, InvalidParameterException, InvalidPolicyRevisionIdException, LimitExceededException, ProvisionedThroughputExceededException, ResourceAlreadyExistsException, ResourceNotFoundException, ServiceQuotaExceededException (400) and InternalServerError, ThrottlingException (500). Recorded behavior (the check's replay of real AWS) shows two distinct rejection classes, both HTTP 400 `MalformedPolicyDocumentException`:
  (a) a `PolicyDocument` that is not parseable JSON -> message exactly "Request contains a malformed policy." — this message is entirely fixed text, with no variable part;
  (b) a `PolicyDocument` that parses as IAM JSON but whose `Resource` is not a valid, EXISTING project-VERSION-shaped ARN (`...:project/<name>/version/<version>/<ts>`) — a bare project ARN, or a version-shaped ARN naming a version that does not exist — -> message exactly "Request contains an invalid resource in the provided policy.", also entirely fixed text with no variable part.
After either refusal nothing is attached: `ListProjectPolicies` on that project returns an empty `ProjectPolicies` list.

Emulator: the call is refused with error code `NotImplemented` (HTTP 501) for the malformed-JSON document, so the check fails at `assert code_bad_json == "MalformedPolicyDocumentException"` with `AssertionError: expected MalformedPolicyDocumentException for invalid JSON, got 'NotImplemented'`. The handler file still carries the scaffold marker and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("PutProjectPolicy"))`, so the invalid-resource class and the success path (returning a `PolicyRevisionId`, and the policy becoming visible to `ListProjectPolicies`) are equally unimplemented.

Why: a client that attaches a Custom Labels project policy to enable `CopyProjectVersion` cannot do so at all, and a client that relies on AWS refusing a bad policy sees `NotImplemented` (501, a server-side "not supported" signal) rather than the modeled 400 client error it catches. Error-handling branches keyed on `MalformedPolicyDocumentException` never run, SDK retry logic treats the 501 differently from a 400, and there is no way to assert that a rejected policy left no attachment behind.

Fix: implement the operation's handler and its validation — `PutProjectPolicy::handle` in services/service-com-amazonaws-rekognition-rekognitionservice/src/ops/put_project_policy.rs — so it parses `PolicyDocument` as JSON and refuses an unparseable document with `MalformedPolicyDocumentException` ("Request contains a malformed policy."), refuses a document whose `Resource` is not an existing project-version ARN with `MalformedPolicyDocumentException` ("Request contains an invalid resource in the provided policy."), persists the accepted policy in this service's state keyed by project ARN + policy name so `ListProjectPolicies`/`DeleteProjectPolicy` observe it, and returns a generated hex `PolicyRevisionId`. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line once done.

Traces: rekognition.put-project-policy-malformed-document-rejected.

Note: this check also exercises `ListProjectPolicies` and `DeleteProject`, each already covered by its own stub issue; this issue is only about `PutProjectPolicy`.

## Evidence
Check rekognition.put-project-policy-malformed-document-rejected, verdict mismatch:

  File "check_rekognition_put_project_policy_malformed_document_rejected.py", line 63, in run
    assert code_bad_json == "MalformedPolicyDocumentException", (
  AssertionError: expected MalformedPolicyDocumentException for invalid JSON, got 'NotImplemented'

Expected (recorded AWS), boundary 1 — PolicyDocument="{this is not valid json":
  HTTP 400, Error.Code = "MalformedPolicyDocumentException",
  Error.Message = "Request contains a malformed policy."   (fixed text, no variable parts)

Expected (recorded AWS), boundary 2 — syntactically valid IAM JSON whose Resource is a bare project ARN (not a project-version ARN):
  HTTP 400, Error.Code = "MalformedPolicyDocumentException",
  Error.Message = "Request contains an invalid resource in the provided policy."   (fixed text, no variable parts)

Expected effect-absent assertion: ListProjectPolicies(ProjectArn=<the project>) -> {"ProjectPolicies": []}

Actual (emulator): HTTP 501, Error.Code = "NotImplemented" for both boundaries.

Handler source confirming the cause — services/service-com-amazonaws-rekognition-rekognitionservice/src/ops/put_project_policy.rs:4 ("//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`") and line 211 (`Err(crate::scaffolded::Scaffolded::not_implemented("PutProjectPolicy"))`).

Model evidence: smithy_query(operation="PutProjectPolicy") lists MalformedPolicyDocumentException as a client error with httpError 400; smithy_query(shape="MalformedPolicyDocumentException") documents "The format of the project policy document that you supplied to PutProjectPolicy is incorrect."

## Steps to Reproduce
1. CreateProject(ProjectName=<unique name>) and keep the returned ProjectArn.
2. Call PutProjectPolicy(ProjectArn=<that ARN>, PolicyName="sim2realtestpolicy", PolicyDocument="{this is not valid json").
   Expect HTTP 400 MalformedPolicyDocumentException with message "Request contains a malformed policy."; the emulator answers HTTP 501 NotImplemented.
3. Call PutProjectPolicy with the same ProjectArn/PolicyName and a syntactically valid IAM policy document whose single statement has Action "rekognition:CopyProjectVersion" and Resource set to the bare project ARN (not a project-version ARN).
   Expect HTTP 400 MalformedPolicyDocumentException with message "Request contains an invalid resource in the provided policy."; the emulator answers HTTP 501 NotImplemented.
4. Call ListProjectPolicies(ProjectArn=<that ARN>) and expect ProjectPolicies == [] (no attachment from either refused call).
5. Replay: rekognition.put-project-policy-malformed-document-rejected.
