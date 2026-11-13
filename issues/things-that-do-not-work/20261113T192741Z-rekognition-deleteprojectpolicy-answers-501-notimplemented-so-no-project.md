Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rekognition.delete-project-policy-wrong-revision-rejected
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition.delete-project-policy-wrong-revision-rejected|run#12|AssertionError|assert code_no_revision == "ResourceNotFoundException", (
Issue-Commit: b037d5392f1efc291497c72edeaad9a6f7e9925d

# rekognition DeleteProjectPolicy answers 501 NotImplemented, so no project policy can ever be deleted and neither refusal class (ResourceNotFoundException / InvalidPolicyRevisionIdException) is reachable

> _Note: this issue was identified against the code base at commit `b037d5392f1efc291497c72edeaad9a6f7e9925d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteProjectPolicy

## Description
1 root cause in the rekognition service.

Cause 1 of 1: DeleteProjectPolicy is still an unimplemented scaffold stub while the rest of the project-policy surface around it (CreateProject, PutProjectPolicy, ListProjectPolicies) answers normally, so a client can attach and list policies but can never delete one, and both documented refusal classes of the operation are unreachable.
Failing check ids: rekognition.delete-project-policy-wrong-revision-rejected

AWS: Smithy (services/service-com-amazonaws-rekognition-rekognitionservice/smithy-model/com-amazonaws-rekognition-rekognitionservice.json): DeleteProjectPolicy takes required ProjectArn (ProjectArn, len 20..2048, pattern ^(^arn:[a-z\d-]+:rekognition:[a-z\d-]+:\d{12}:project\/[a-zA-Z0-9_.\-]{1,255}\/[0-9]+$)$) and required PolicyName (ProjectPolicyName, len 1..128, pattern ^[a-zA-Z0-9_.\-]+$), plus optional PolicyRevisionId (ProjectPolicyRevisionId, len 0..64, pattern ^[0-9A-Fa-f]+$); its output shape DeleteProjectPolicyResponse has no members, so success is HTTP 200 with an empty body. Modeled errors include ResourceNotFoundException (HTTP 400, "The resource specified in the request cannot be found") and InvalidPolicyRevisionIdException (HTTP 400, "The supplied revision id for the project policy is invalid"). Recorded behavior supplies the wording and the ordering the model does not state:
  * No policy of that name is attached to the project -> HTTP 400 ResourceNotFoundException with message "The specified policy <PolicyName> does not exist for project <ProjectName>." The fixed text is "The specified policy ", " does not exist for project " and the trailing "."; <PolicyName> is the request's PolicyName and <ProjectName> is the project-name segment of the request's ProjectArn (the part after "project/" and before the trailing "/<timestamp>"). Both values come from the request/state, never from one recording.
  * Existence is checked BEFORE the revision id: the same call with a syntactically valid all-hex PolicyRevisionId for a name that was never attached still returns that same ResourceNotFoundException and message, not a revision-mismatch error.
  * Policy attached but PolicyRevisionId does not equal the latest revision id -> HTTP 400 InvalidPolicyRevisionIdException with message "The policy cannot be deleted. Please enter the latest policy revision ID associated with the policy <PolicyName> and try again." Fixed text is everything except <PolicyName>, which is the request's PolicyName.
  * Both refusals leave state untouched: ListProjectPolicies still reports exactly the policies attached before the attempt.
  * The same call with the correct PolicyRevisionId (the value PutProjectPolicy returned) succeeds with HTTP 200 and removes the policy, after which ListProjectPolicies returns an empty ProjectPolicies list.

Emulator: every DeleteProjectPolicy call is refused with HTTP 501 and error code 'NotImplemented' regardless of input. Replay output: "AssertionError: expected ResourceNotFoundException, got 'NotImplemented'" on the first delete of a never-attached policy name. The handler's body is the scaffold's Err(crate::scaffolded::Scaffolded::not_implemented("DeleteProjectPolicy")), and the file still carries the "SCAFFOLDED, NOT IMPLEMENTED" marker. Because the call never gets past 501, the revision-mismatch branch, the no-op-on-refusal guarantee and the successful delete are all unverifiable.

Why: a client that attaches a project policy can never remove it, and error handling built on the real API breaks twice over — code that catches ResourceNotFoundException to treat a delete as idempotent instead sees an unmodeled 501/NotImplemented (botocore raises it as an unexpected ClientError, and SDK retry logic does not treat 501 as terminal the way a modeled 400 is), and code that catches InvalidPolicyRevisionIdException to re-read the latest revision and retry never reaches that path. Project-policy cleanup leaks policies, so any later PutProjectPolicy/CopyProjectVersion flow that assumes a clean project misbehaves.

Fix: the operation's handler — services/service-com-amazonaws-rekognition-rekognitionservice/src/ops/delete_project_policy.rs — implement handle() against the same project-policy state that put_project_policy.rs and the ListProjectPolicies handler already read and write (look the project up by ProjectArn, then the policy by PolicyName; refuse with ResourceNotFoundException before consulting PolicyRevisionId, refuse with InvalidPolicyRevisionIdException when a supplied revision id is not the stored latest, otherwise remove the policy and return the empty 200 output), and delete the SCAFFOLDED marker line once done. Message templates belong with the error construction in that handler.

Traces: rekognition.delete-project-policy-wrong-revision-rejected (fails at the first boundary, the never-attached policy name, with no revision id supplied).

Note: the open issues '20261101T060719Z-rekognition-listprojectpolicies-stub.md' and '20261101T060719Z-rekognition-deleteproject-stub.md' cover neighbouring operations and do NOT cover DeleteProjectPolicy; no open issue names this operation.

## Evidence
Expected (real AWS, per the check's assertions and the Smithy model):
  - DeleteProjectPolicy(ProjectArn=<project arn>, PolicyName="sim2realtestpolicy") on a brand-new project -> HTTP 400, Error.Code "ResourceNotFoundException", Error.Message "The specified policy sim2realtestpolicy does not exist for project <project name>." (fixed text with PolicyName and the ProjectArn's project-name segment substituted).
  - Same call with PolicyRevisionId="deadbeef" -> identical HTTP 400 ResourceNotFoundException and identical message (existence checked before revision).
  - ListProjectPolicies after both refusals -> ProjectPolicies == [].
  - After PutProjectPolicy returns PolicyRevisionId R: DeleteProjectPolicy with a wrong-but-well-formed revision -> HTTP 400 "InvalidPolicyRevisionIdException", message "The policy cannot be deleted. Please enter the latest policy revision ID associated with the policy sim2realtestpolicy and try again."; the policy survives.
  - DeleteProjectPolicy with PolicyRevisionId=R -> HTTP 200, empty body; ListProjectPolicies -> ProjectPolicies == [].

Actual (emulator):
  - HTTP 501 with Error.Code "NotImplemented" on the very first DeleteProjectPolicy call.
  - Replay: check_rekognition_delete_project_policy_wrong_revision_rejected.py line 38 -> "AssertionError: expected ResourceNotFoundException, got 'NotImplemented'".
  - Source: src/ops/delete_project_policy.rs still returns Scaffolded::not_implemented("DeleteProjectPolicy") and retains the "//! SCAFFOLDED, NOT IMPLEMENTED" marker; CreateProject, PutProjectPolicy and ListProjectPolicies in the same service answered this check successfully, so only the delete is missing.

## Steps to Reproduce
1. CreateProject(ProjectName="sim2real-rek-polrev-<suffix>") and keep the returned ProjectArn.
2. ListProjectPolicies(ProjectArn=...) -> ProjectPolicies == [] (passes today).
3. DeleteProjectPolicy(ProjectArn=..., PolicyName="sim2realtestpolicy") -> AWS returns HTTP 400 ResourceNotFoundException "The specified policy sim2realtestpolicy does not exist for project <project name>."; the emulator returns HTTP 501 NotImplemented.
4. Repeat step 3 with PolicyRevisionId="deadbeef" -> AWS returns the same ResourceNotFoundException and message; emulator returns 501.
5. PutProjectPolicy(ProjectArn=..., PolicyName="sim2realtestpolicy", PolicyDocument=<policy whose Resource is a project-version-shaped ARN>) and keep PolicyRevisionId R.
6. DeleteProjectPolicy with a wrong all-hex revision -> AWS: HTTP 400 InvalidPolicyRevisionIdException "The policy cannot be deleted. Please enter the latest policy revision ID associated with the policy sim2realtestpolicy and try again.", policy still listed.
7. DeleteProjectPolicy with PolicyRevisionId=R -> AWS: HTTP 200 and ListProjectPolicies returns [].
Replay: rekognition.delete-project-policy-wrong-revision-rejected.
