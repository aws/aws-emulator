Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.webhook-targeting-absent-pipeline-lifecycle
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|DeregisterWebhookWithThirdParty|ValidationException
Issue-Commit: eb149afeb17c64df33e2fc28f71e5890f0d7e7b3

# codepipeline: DeregisterWebhookWithThirdParty refuses an UNAUTHENTICATED webhook with a dangling-target-pipeline sentence instead of AWS's GITHUB_HMAC authentication-type ValidationException message

> _Note: this issue was identified against the code base at commit `eb149afeb17c64df33e2fc28f71e5890f0d7e7b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeregisterWebhookWithThirdParty

## Description
Cause 1 of 1: DeregisterWebhookWithThirdParty evaluates the stored webhook's targetPipeline before (or instead of) the stored webhook's authentication type, so a webhook created with authentication UNAUTHENTICATED is refused with an emulator-authored pipeline-existence sentence and AWS's authentication-type refusal is never produced. Failing check: codepipeline.webhook-targeting-absent-pipeline-lifecycle.

AWS: Smithy (codepipeline-2015-07-09) models DeregisterWebhookWithThirdParty with a single input member webhookName (WebhookName, @length 1..100, pattern ^[A-Za-z0-9.@\-_]+$), an empty output, and exactly two errors, both client / HTTP 400: ValidationException and WebhookNotFoundException. The model does not state the ordering of the checks, so the refusal itself is recorded behavior only: third-party registration is meaningful only for a GITHUB_HMAC webhook, and the recording shows AWS refusing a webhook with any other authentication type with HTTP 400, Code "ValidationException" and the message
    Webhook <webhookName> must be configured with GITHUB_HMAC authentication type. Type is <authentication>.
Fixed text of that message: "Webhook ", " must be configured with GITHUB_HMAC authentication type. Type is ", ".". Its two variable parts come from the request and from state: <webhookName> is the request's webhookName and <authentication> is the authentication value stored on that webhook by PutWebhook (UNAUTHENTICATED in this check). This refusal is reached even when the webhook's targetPipeline names no pipeline: PutWebhook legitimately accepts a webhook whose targetPipeline does not exist (the check's earlier steps, which pass, confirm the emulator accepts it, lists it with its ARN and echoes its definition verbatim), so a dangling targetPipeline is not the condition AWS reports for an UNAUTHENTICATED webhook.

Emulator: the HTTP status (400) and the error code (ValidationException) are correct, but the message is a different refusal on a different condition:
    Webhook <name> references a pipeline <targetPipeline> which does not exist
That sentence is AWS's wording for the dangling-target-pipeline refusal of a GITHUB_HMAC webhook, not for an UNAUTHENTICATED one; the authentication-type check is therefore absent or ordered after the pipeline-existence check.

Why: a client relying on AWS's behavior cannot act on the refusal. It is told to create a pipeline when the actual requirement is that the webhook be re-created with GITHUB_HMAC authentication, and a client that branches on the message (or on the presence of the authentication-type sentence) to decide whether a webhook is third-party-registrable takes the wrong branch. A dangling targetPipeline is an AWS-accepted webhook state, so reporting it here surfaces a non-condition.

Fix: the validation of DeregisterWebhookWithThirdParty in the CodePipeline webhook handler — after resolving the webhook by name (WebhookNotFoundException when absent), check the stored webhook's authentication FIRST and raise ValidationException with AWS's GITHUB_HMAC sentence, keeping the targetPipeline-existence refusal reachable only for a GITHUB_HMAC webhook.

Already-filed issues named for context, neither of which the suite reports as owning this check: 20261117T111420Z-codepipeline-deregisterwebhookwiththirdparty-validates-the-webhook-s-target-pipeline.md describes this same mis-ordering but carries no replay key for this check, so the check still fails today; 20261119T130532Z-codepipeline-deregisterwebhookwiththirdparty-succeeds-with-200-instead-of-refusing.md is a wire issue about the pipeline-existence refusal being skipped entirely (200 instead of 400) on a GITHUB_HMAC webhook, a different path. A fix must satisfy both orderings.

Traces: codepipeline.webhook-targeting-absent-pipeline-lifecycle — every earlier step of that flow passes (PutWebhook accepts a dangling targetPipeline and echoes the definition, ListWebhooks lists it with its ARN, DeleteWebhook answers 200, a repeat DeleteWebhook answers 200, and the deleted name is refused WebhookNotFoundException), so this message is the sole surviving divergence in it.

## Evidence
Check: codepipeline.webhook-targeting-absent-pipeline-lifecycle (verdict: mismatch).

Setup (accepted, matching AWS): PutWebhook with a definition whose targetPipeline names a pipeline that does not exist, targetAction "Source", one filter {jsonPath: "$.ref", matchEquals: "refs/heads/main"}, authentication "UNAUTHENTICATED", authenticationConfiguration {}. The webhook is created, returned with its ARN, listed by ListWebhooks and its definition echoed verbatim.

Request: DeregisterWebhookWithThirdParty(webhookName=<that webhook's name>)

Expected (AWS): HTTP 400, Code "ValidationException", Message
  "Webhook <webhookName> must be configured with GITHUB_HMAC authentication type. Type is UNAUTHENTICATED."
  Fixed text: "Webhook " / " must be configured with GITHUB_HMAC authentication type. Type is " / "."; variable parts: the request's webhookName and the stored webhook's authentication value.

Actual (emulator): HTTP 400, Code "ValidationException" (correct), Message
  "Webhook <webhookName> references a pipeline <targetPipeline> which does not exist"

Replay output:
  File "check_codepipeline_webhook_targeting_absent_pipeline_lifecycle.py", line 96, in run
    expect_error(
  File "check_codepipeline_webhook_targeting_absent_pipeline_lifecycle.py", line 22, in expect_error
    assert err.get("Message") == message
  AssertionError: expected message 'Webhook sim2real-behav-cp-wh-sibling-6bcd573b92d9 must be configured with GITHUB_HMAC authentication type. Type is UNAUTHENTICATED.', got 'Webhook sim2real-behav-cp-wh-sibling-6bcd573b92d9 references a pipeline sim2real-behav-cp-pl-absent-83d69e50fab7 which does not exist'

The webhook and pipeline names in that output are per-run generated values that fill the template; they are not part of the requirement.

Model evidence: Smithy DeregisterWebhookWithThirdParty — input {webhookName: WebhookName @length 1..100, pattern ^[A-Za-z0-9.@\-_]+$}, output {} (empty), errors ValidationException (client, httpError 400) and WebhookNotFoundException (client, httpError 400).

## Steps to Reproduce
1. PutWebhook with webhook = {name: "wh1", targetPipeline: "no-such-pipeline", targetAction: "Source", filters: [{jsonPath: "$.ref", matchEquals: "refs/heads/main"}], authentication: "UNAUTHENTICATED", authenticationConfiguration: {}}. It is accepted (as on AWS) even though no pipeline named "no-such-pipeline" exists.
2. ListWebhooks — the webhook is listed with its ARN and the definition echoed verbatim (passes today).
3. Call DeregisterWebhookWithThirdParty(webhookName="wh1").
   - Expected: HTTP 400, ValidationException, message "Webhook wh1 must be configured with GITHUB_HMAC authentication type. Type is UNAUTHENTICATED."
   - Observed: HTTP 400, ValidationException, message "Webhook wh1 references a pipeline no-such-pipeline which does not exist".
4. Control for the ordering: create a pipeline actually named "no-such-pipeline" (or point the UNAUTHENTICATED webhook at an existing pipeline) and repeat step 3 — the authentication-type refusal must be raised for an UNAUTHENTICATED webhook regardless of whether the target pipeline exists, showing the two checks are mis-ordered rather than the pipeline check merely being extra.
5. Call DeregisterWebhookWithThirdParty on a webhook name that was deleted: WebhookNotFoundException must still be raised (this already matches).
6. Re-replay codepipeline.webhook-targeting-absent-pipeline-lifecycle to confirm.
