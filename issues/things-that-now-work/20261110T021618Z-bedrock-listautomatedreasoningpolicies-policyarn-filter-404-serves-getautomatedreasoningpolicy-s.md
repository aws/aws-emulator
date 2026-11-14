Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.delete-automated-reasoning-policy-in-use-rejected
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.delete-automated-reasoning-policy-in-use-rejected|run#21|AssertionError|assert list_gone == list_gone_expected, (
Issue-Commit: 82a95c6eb540e273b0e8819fa03b8f3bfa792799

# bedrock ListAutomatedReasoningPolicies: policyArn-filter 404 serves GetAutomatedReasoningPolicy's message text, dropping the trailing period

> _Note: this issue was identified against the code base at commit `82a95c6eb540e273b0e8819fa03b8f3bfa792799`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAutomatedReasoningPolicies

## Description
Cause 1 of 1: ListAutomatedReasoningPolicies' ResourceNotFoundException message text is wrong — the emulator serves the sibling Get operation's wording for the policyArn-filter refusal.

NOTE ON AN EXISTING ISSUE: the open issue 20261106T091754Z-bedrock-listautomatedreasoningpolicies-the-policyarn-filter-404-reuses-getautomatedreasoningpolicy.md describes the same symptom, but the replay suite did not report this check as OWNED by it (its recorded Issue-Key pins a step location that no longer matches), so the failing check is still unowned and is filed here. If that issue is being worked, this one can be closed against the same fix.

AWS: ListAutomatedReasoningPolicies (Smithy: GET /automated-reasoning-policies, success 200; optional input member policyArn — target AutomatedReasoningPolicyArn, httpQuery "policyArn", @length 1..2048, pattern ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$; modeled errors include ResourceNotFoundException, errorKind client, httpError 404). When the policyArn filter names an ARN for which the account holds no revision — e.g. a policy already removed by DeleteAutomatedReasoningPolicy — AWS refuses the listing with 404 ResourceNotFoundException instead of returning an empty page. The model carries no message template for that error, so the text is recorded behavior only: the recording shows Error.Code "ResourceNotFoundException" and Error.Message "Policy does not exist." — the entire message is FIXED text ending in a period, with no variable part (the ARN is not interpolated). The recording also shows this wording is NOT shared with GetAutomatedReasoningPolicy: the same dead ARN on Get answers "Policy does not exist" with NO trailing period, and the check asserts the two spellings separately in one run.

Emulator: the dead-ARN filtered listing answers 404 ResourceNotFoundException with message "Policy does not exist" — the period is missing. The HTTP status and the error code are correct; only the message diverges. The Get leg of the same check passes, which shows the List path is emitting Get's spelling: the List handler builds its message from the shared store helper automated_reasoning_policy_revision_not_found_message(policy_arn) used by both operations.

Why: a client that keys on the error message — error-message assertions in tests, log/alert matchers, or wrappers that distinguish which Automated Reasoning API refused — sees a message AWS never sends for this operation. Because on real AWS the two sibling operations differ in exactly this one character, code written against real AWS and matching List's message silently stops matching against the emulator, while code-only handling still works, so the divergence stays invisible until the message path is exercised.

Fix: the ListAutomatedReasoningPolicies handler's refusal message for an unresolvable policyArn filter, in services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/list_automated_reasoning_policies.rs — the ResourceNotFoundException constructed when the store yields no revision for the ARN. The shared state helper serves two operations whose AWS wording differs by a trailing period, so List needs its own message text (or the helper must be parameterised per operation). Everything else about the refusal is unchanged: still 404, still code ResourceNotFoundException, still a refusal rather than an empty page, and Get's period-less spelling must stay as is.

Traces: bedrock.delete-automated-reasoning-policy-in-use-rejected — the check's earlier legs (the ValidationException "There are build workflow that have not been deleted." refusal while a build workflow exists, the policy surviving the rejected delete, and the Get 404 message) all pass; it fails only on the filtered-list message.

## Evidence
Failing check bedrock.delete-automated-reasoning-policy-in-use-rejected (verdict: mismatch).

Assertion from the replay:
  File "check_bedrock_delete_automated_reasoning_policy_in_use_rejected.py", line 93, in run
    assert list_gone == list_gone_expected, (
  AssertionError: expected a filtered list on a dead policy to 404 with ('ResourceNotFoundException', 'Policy does not exist.'), got ('ResourceNotFoundException', 'Policy does not exist')

Expected (AWS, recorded):
  ListAutomatedReasoningPolicies(policyArn=<deleted policy ARN>)
    -> HTTP 404, Error.Code = "ResourceNotFoundException",
       Error.Message = "Policy does not exist."  (fixed text, trailing period; no values interpolated from request or state)

Actual (emulator):
  ListAutomatedReasoningPolicies(policyArn=<deleted policy ARN>)
    -> HTTP 404, Error.Code = "ResourceNotFoundException",
       Error.Message = "Policy does not exist"   (no trailing period)

Passing control in the same run, showing the two messages are genuinely distinct on AWS and only List diverges:
  GetAutomatedReasoningPolicy(policyArn=<same deleted ARN>)
    -> HTTP 404, "ResourceNotFoundException", "Policy does not exist" (no period) — emulator matches.

Smithy (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json):
  ListAutomatedReasoningPolicies: GET /automated-reasoning-policies, http code 200; errors AccessDeniedException(403), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); input policyArn is an optional httpQuery filter. No message template is modeled, hence "recorded behavior only" for the text.

Handler: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/list_automated_reasoning_policies.rs builds the message via the store helper automated_reasoning_policy_revision_not_found_message(policy_arn), shared with the Get path.

## Steps to Reproduce
1. bedrock.create_automated_reasoning_policy(name=<unique>, policyDefinition={}) -> capture policyArn.
2. bedrock.start_automated_reasoning_policy_build_workflow(policyArn=..., buildWorkflowType="INGEST_CONTENT", sourceContent={"workflowContent": {"documents": [...]}}) and poll get_automated_reasoning_policy_build_workflow until status == COMPLETED.
3. bedrock.delete_automated_reasoning_policy_build_workflow(policyArn=..., buildWorkflowId=..., lastUpdatedAt=<workflow updatedAt>).
4. bedrock.delete_automated_reasoning_policy(policyArn=...) -> succeeds; the policy is gone.
5. bedrock.list_automated_reasoning_policies(policyArn=<the deleted ARN>) and read the ClientError's Error.Code / Error.Message.
   Observed: ("ResourceNotFoundException", "Policy does not exist").
   Expected: ("ResourceNotFoundException", "Policy does not exist.").
6. Contrast: bedrock.get_automated_reasoning_policy(policyArn=<same ARN>) -> ("ResourceNotFoundException", "Policy does not exist") with no period, which is correct and must not change.

Replay the whole check with: bedrock.delete-automated-reasoning-policy-in-use-rejected
