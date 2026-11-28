Issue-Severity: MEDIUM
Issue-Commit: e053caff888afb9bf6b05592273b8831bb8fc570

# DescribeAppInstanceBot on a well-formed AppInstanceBotArn that names no bot answers 404 NotFoundException instead of the 403 ForbiddenException AWS serves

> _Note: this issue was identified against the code base at commit `e053caff888afb9bf6b05592273b8831bb8fc570`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAppInstanceBot

## Description
Cause 1 of 1: DescribeAppInstanceBot classifies "this ARN names no bot I hold" as a not-found report, where AWS refuses the read as a request the caller may not make. Failing check: chime-sdk-identity.describe-app-instance-bot-not-found-rejected.

AWS: recorded behavior only for the choice between the two classes — the Smithy model declares BOTH on this operation (DescribeAppInstanceBot errors: ForbiddenException httpError 403, "The client is permanently forbidden from making the request."; NotFoundException httpError 404, "One or more of the resources in the request does not exist in the system."), so the model does not by itself say which a non-existent bot earns. The recording settles it: for a syntactically valid AppInstanceBotArn under a LIVE AppInstance that names no AppInstanceBot, Chime SDK Identity answers HTTP 403 with Code "ForbiddenException" and Message "You are not allowed to make this request." — AWS does not disclose whether such a bot exists. In that message the whole sentence is fixed text: it interpolates nothing from the request or from state, and in particular it never names the ARN that was addressed. The sibling AppInstance in the same request stays readable (DescribeAppInstance on the parent answers 200), so the refusal is per-resource and not a credential failure.

Emulator: the handler reports absence instead. For the same input it answers HTTP 404 with Code "NotFoundException" and Message "AppInstanceBot <AppInstanceBotArn> does not exist" — a message shape AWS never serves here, whose variable part is the ARN taken straight from the request. The handler reaches this deliberately: when the bot lookup misses it consults state.unresolved_arn(..) and routes only UnresolvedArn::DeletedAppInstance to ForbiddenException (with the crate's own correct FORBIDDEN_REQUEST_MESSAGE), while UnresolvedArn::NeverExisted is turned into the 404. AWS serves the 403 for the NeverExisted branch too, so the branch, not the helper, is what diverges.

Why: a client branching on the error code sees the wrong class and the wrong retry/handling contract. Code "NotFoundException" with 404 reads as "the resource is absent, the call itself was permitted", so a caller probing for a bot treats the answer as authoritative absence; against AWS the same call yields 403 ForbiddenException, which SDK error handling and IAM-aware retry/alarm logic classify as an authorization refusal and never as absence. It is also an information-disclosure divergence of the kind AWS avoids deliberately: the emulator confirms, and echoes back, an ARN AWS declines to say anything about, so a caller can enumerate which bot ARNs exist against the emulator and cannot against AWS. Any code that distinguishes "no such bot" from "not allowed" takes the opposite branch on the emulator.

Fix: the absence branch of the DescribeAppInstanceBot handler's refusal selection — the arm that maps UnresolvedArn::NeverExisted to NotFoundException should refuse with the modelled ForbiddenException carrying the crate's existing FORBIDDEN_REQUEST_MESSAGE constant, the same refusal its DeletedAppInstance sibling arm already mints, so a bot ARN that resolves to nothing is refused identically however it came to be unresolvable. The ARN-interpolating "AppInstanceBot ... does not exist" sentence should not be served by this operation at all. Note this is NOT the same defect as the open issue 20261122T042546Z-chime-sdk-identity-getappinstanceretentionsettings-error-response (which is about a handler that never calls unresolved_arn): this handler does call it and mis-routes one of its two outcomes. The inverse-direction issue 20261120T131514Z-chime-sdk-meetings-getattendee-error-response is a different service and does not cover this.

Traces: chime-sdk-identity.describe-app-instance-bot-not-found-rejected. Code citations: services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/describe_app_instance_bot.rs:169-189 (the two-arm refusal; the NeverExisted arm at :178-187), and the helper and message constant it uses at src/state.rs:579-588 (unresolved_arn), :314-324 (UnresolvedArn) and :126 (FORBIDDEN_REQUEST_MESSAGE).

## Evidence
Failing check: chime-sdk-identity.describe-app-instance-bot-not-found-rejected (group "overly-permissive", axis:rejection, plane:control).

Assertion that failed, from the replay output:
    assert error["Code"] == "ForbiddenException", f"expected ForbiddenException, got {error!r}"
    AssertionError: expected ForbiddenException, got {'Message': 'AppInstanceBot arn:aws:chime:us-east-1:<account>:app-instance/<instance-id>/bot/<bot-id> does not exist', 'Code': 'NotFoundException'}

Expected (AWS, as the check pins it):
  HTTP 403, Code "ForbiddenException", Message "You are not allowed to make this request." (entire sentence is fixed text, no interpolation)

Actual (emulator):
  HTTP 404, Code "NotFoundException", Message "AppInstanceBot <AppInstanceBotArn> does not exist" (ARN echoed from the request)

The check also asserts the two controls that show this is not a credential problem and that the emulator's state is otherwise right: ListAppInstanceBots on the parent lists no such bot, and DescribeAppInstance on the parent AppInstanceArn answers 200. Those both pass; only the refusal class, status and message diverge.

Smithy (model_path services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/smithy-model/com-amazonaws-chimesdkidentity-chimeidentityservice.json):
  DescribeAppInstanceBot: GET /app-instance-bots/{AppInstanceBotArn}, success 200; errors include ForbiddenException (403) and NotFoundException (404) — both modelled, so the handler is choosing between them.
  ForbiddenException: httpError 403, doc "The client is permanently forbidden from making the request."
  NotFoundException: httpError 404, doc "One or more of the resources in the request does not exist in the system."
  Input AppInstanceBotArn: ChimeArn, required, httpLabel, length 5..1600, pattern ^arn:[a-z0-9-\.]{1,63}:... — the ARN the check sends satisfies this, so the request is well-formed and nothing in validation is at issue.

Emulator source, services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/describe_app_instance_bot.rs:169-189:
        let Some(record) = state.bot(&input.app_instance_bot_arn) else {
            return Err(match state.unresolved_arn(&input.app_instance_bot_arn) {
                crate::state::UnresolvedArn::DeletedAppInstance => {
                    DescribeAppInstanceBotErrorModeled::ForbiddenException(ForbiddenException {
                        code: Some("Forbidden".to_string()),
                        message: Some(crate::state::FORBIDDEN_REQUEST_MESSAGE.to_string()),
                    })
                    .into()
                }
                crate::state::UnresolvedArn::NeverExisted => {
                    DescribeAppInstanceBotErrorModeled::NotFoundException(NotFoundException {
                        code: Some("NotFound".to_string()),
                        message: Some(format!(
                            "AppInstanceBot {} does not exist",
                            input.app_instance_bot_arn
                        )),
                    })
                    .into()
                }
            });
        };

The handler comment at :156-168 states the intent explicitly — "An `AppInstanceBotArn` this service holds no bot for is reported as ABSENT" and "The permanent 403 refusal stays reserved for ... an ARN at or under an `AppInstance` this store has DELETED" — so the divergence is a deliberate design choice contradicted by the recording, and that comment must change with the fix.

The correct message text already exists in the crate, src/state.rs:126:
    pub const FORBIDDEN_REQUEST_MESSAGE: &str = "You are not allowed to make this request.";
and src/state.rs:572-588 shows unresolved_arn returning DeletedAppInstance vs NeverExisted, the discriminator the handler mis-routes.

## Steps to Reproduce
1. POST /app-instances with a Name, Metadata "" and a fresh ClientRequestToken; keep the returned AppInstanceArn. (Chime SDK Identity's control plane is addressed in us-east-1.)
2. Build a well-formed but non-existent bot ARN under that live instance: <AppInstanceArn>/bot/<a fresh UUID>. Create no bot.
3. GET /app-instance-bots/<that ARN>.
   Expected (AWS): HTTP 403, body Code "ForbiddenException", Message "You are not allowed to make this request." — no mention of the ARN.
   Actual (emulator): HTTP 404, body Code "NotFoundException", Message "AppInstanceBot <that ARN> does not exist".
4. Controls, both of which already behave correctly and must keep doing so after the fix: GET /app-instances/<AppInstanceArn>/bots lists no bot with that ARN, and GET /app-instances/<AppInstanceArn> answers 200 with that AppInstanceArn — the same credentials are plainly able to make requests, so the 403 in step 3 is a per-resource refusal, not an auth failure.
5. Contrast with the branch that is already right: DELETE /app-instances/<AppInstanceArn>, then repeat step 3 against the same bot ARN — the emulator answers 403 ForbiddenException "You are not allowed to make this request." via the UnresolvedArn::DeletedAppInstance arm. AWS answers that same 403 in BOTH cases; the emulator only serves it after the parent is deleted.
6. Re-replay chime-sdk-identity.describe-app-instance-bot-not-found-rejected to confirm.
