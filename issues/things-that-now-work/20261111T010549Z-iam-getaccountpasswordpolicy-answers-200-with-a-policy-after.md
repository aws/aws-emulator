Issue-Source: trace-sweep-divergence
Issue-Trace: iam-account-password-policy-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-account-password-policy-lifecycle iam-account-settings-alias-and-password-policy
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|GetAccountPasswordPolicy|4xx->2xx|$.ErrorResponse
Issue-Key: iam|GetAccountPasswordPolicy|4xx->2xx|status
Issue-Commit: 7eb0bd3bda5e5fd14f4b01ea8dd2daf93fb4b55d

# iam GetAccountPasswordPolicy answers 200 with a policy after DeleteAccountPasswordPolicy instead of 404 NoSuchEntity

> _Note: this issue was identified against the code base at commit `7eb0bd3bda5e5fd14f4b01ea8dd2daf93fb4b55d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountPasswordPolicy

## Description
One net-new cause.

Cause 1 of 1: after the account password policy has been deleted, GetAccountPasswordPolicy still serves a policy.
AWS: Smithy iam#GetAccountPasswordPolicy takes no input, returns PasswordPolicy (required) and models NoSuchEntityException (awsQueryError code NoSuchEntity, httpResponseCode 404) plus ServiceFailureException (500). When the account has no custom password policy — which is the state DeleteAccountPasswordPolicy leaves behind — AWS answers HTTP 404 with an ErrorResponse whose Error.Type is 'Sender', Error.Code is 'NoSuchEntity' and Error.Message is "The Password Policy with domain name <accountId> cannot be found." In that message the fixed text is "The Password Policy with domain name " ... " cannot be found." and the only variable part is the 12-digit account id taken from the caller's account (in the recording, the emulator account 000000000000); the RequestId alongside it is per-request and is not part of the requirement.
Emulator: HTTP 200 with a GetAccountPasswordPolicyResponse body and no ErrorResponse at all — the read ignores that the policy was removed, so the account looks as though a password policy is configured when none is.
Why: a client that uses GetAccountPasswordPolicy to discover whether a custom policy exists (the documented probe — catch NoSuchEntity to mean "account defaults") concludes a policy is in force and reports or enforces password rules the account does not have; a delete-then-verify teardown can never confirm the delete.
Fix: the GetAccountPasswordPolicy handler and the account-level password-policy state it reads — the delete must clear the stored policy (or the read must treat the cleared state as absent) and the handler must raise the modelled NoSuchEntityException with AWS's 404/NoSuchEntity/Sender wording.
Traces: iam-account-password-policy-lifecycle#task#5, and the same cluster's iam-account-settings-alias-and-password-policy#task#14.

Note: this cluster is reported with owned_by empty and stub_path empty, so no open issue carries its replay key. The related open issue '20261029T184359Z-iam-account-level-iam-actions-answer-invalidaction-instead-of.md' owns the sibling clusters where DeleteAccountPasswordPolicy / GetAccountSummary answer 400 InvalidAction; it does not cover this 200-instead-of-404 read.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout:

iam-account-password-policy-lifecycle#task#5, operation GetAccountPasswordPolicy, verdict mismatch, affected_count=2 (also iam-account-settings-alias-and-password-policy#task#14):
  status: expected 404, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'NoSuchEntity', 'Message': 'The Password Policy with domain name 000000000000 cannot be found.'}, 'RequestId': 'e06be7a8-f6d3-4922-9dbd-3d924c0867d8'}, got None)

Model grounding (smithy_query, com-amazonaws-iam-awsidentitymanagementv20100508.json): GetAccountPasswordPolicy input Unit, output GetAccountPasswordPolicyResponse{PasswordPolicy required}, errors NoSuchEntityException{errorKind client, httpError 404, awsQueryError NoSuchEntity/404} and ServiceFailureException{500}.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Replay fixture iam-account-password-policy-lifecycle up to step task#5: UpdateAccountPasswordPolicy, DeleteAccountPasswordPolicy, then POST Action=GetAccountPasswordPolicy on the IAM Query endpoint.
4. Observe HTTP 200 with a GetAccountPasswordPolicyResponse body, where AWS answers HTTP 404 with Error.Code NoSuchEntity, Error.Type Sender and message "The Password Policy with domain name <accountId> cannot be found."
5. Same step reachable via iam-account-settings-alias-and-password-policy task#14.
