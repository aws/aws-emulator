Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatchomni-deep-link-and-identity-center-refusals
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatchomni-deep-link-and-identity-center-refusals
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni|UpdateDomain|4xx->2xx|$.__type
Issue-Key: cloudwatchomni|UpdateDomain|4xx->2xx|$.domain.customEndpointUrls[*]
Issue-Key: cloudwatchomni|UpdateDomain|4xx->2xx|$.domain.domainEndpointUrl
Issue-Key: cloudwatchomni|UpdateDomain|4xx->2xx|$.message
Issue-Key: cloudwatchomni|UpdateDomain|4xx->2xx|status
Issue-Key: cloudwatchomni|GetDomain|2xx->2xx|$.domain.identityProviders[*]
Issue-Related: 20261112T102024Z-cloudwatchomni-publishes-domain-endpoint-urls-on-a-cloudwatch.md
Issue-Commit: c2a9aa425461796da61883782981cd948eb4a841

# cloudwatchomni UpdateDomain accepts IDC identity provider with no Identity Center instance and persists the refused patch

> _Note: this issue was identified against the code base at commit `c2a9aa425461796da61883782981cd948eb4a841`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDomain

## Description
TWO net-new causes on the wire: UpdateDomain does not enforce the Identity Center precondition on identityProviders, and because the refusal never happens the patch is committed to state and read back by GetDomain. Both must be closed and both steps re-replayed.

Related but NOT covering these: open issue 20261119T162417Z-cloudwatchomni-updatedomain-accepts-a-switch-to-the-idc.md reports the same missing precondition from the BEHAVIORAL suite (keyed on check cloudwatchomni.domain-name-pattern-and-identity-center-rules-reject-updates); it carries no wire replay key, so neither of the fixture steps below is exercised by it, and it does not assert the read-back of cause 2. Open issue 20261111T022337Z-cloudwatchomni-conflict-and-validation-error-messages-diverge-from.md mentions this second refusal only in prose as "not yet reachable" (its cause 5 is the @pattern message) and carries no key for it. Open issue 20261115T152312Z-cloudwatchomni-searchprincipals-identity-center-precondition-message-diverges-from.md is a different operation.

Cause 1 of 2: UpdateDomain does not require an Identity Center instance when the resulting identityProviders include IDC (fixture cloudwatchomni-deep-link-and-identity-center-refusals, step task#6).
  AWS: Smithy UpdateDomain is @idempotent and takes domainId (DomainId, required, @pattern ^d-[0-9a-z]{1,25}$), name (@length 3..63, @pattern ^[a-z0-9]+(-[a-z0-9]+)*$), identityProviders (IdentityProviderList, @length 1..2, @uniqueItems) and identityProviderConfiguration (IdentityProviderConfiguration); it declares ValidationException (errorKind client, @httpError 400). The model states the rule on IdentityProviderConfiguration.identityCenterConfiguration, documented "Identity Center configuration. Required when identityProviders includes IDC." The refusal's text is recorded behavior only: HTTP 400, __type com.amazon.cloudwatchomnifrontend#ValidationException, message "identityCenterInstanceArn is required when authorizationProviders includes IDC" — the entire sentence is FIXED text with NO variable part (no domainId, no provider list, no request value appears in it).
  Emulator: the same request answers HTTP 200 and returns the patched `domain` envelope (with identityProviders switched to IDC and the domain's endpoint URLs), raising no ValidationException. The handler validates only enum membership of each provider value.
  Why: a client that relies on AWS refusing this request believes it holds a domain federated through Identity Center when no instance backs it. The 200 persists a state the model's own documentation forbids, so the error surfaces far from the call that caused it — on a later Identity-Center-dependent call, with a different error code — and every negative-path test asserting the refusal passes on AWS and fails here.
  Fix: UpdateDomain's input validation in the cloudwatchomni service crate (services/service-com-amazonaws-cloudwatchomni-cloudwatchomnifrontend, the UpdateDomain handler) — before any mutation, when the post-patch provider list contains IDC, require an identityCenterConfiguration from the request or the stored domain and otherwise return ValidationException (400) with AWS's fixed message; gate any Identity Center application-ARN minting behind the same precondition.
  Traces: cloudwatchomni-deep-link-and-identity-center-refusals#task#6 (UpdateDomain, expected 400, actual 200).

Cause 2 of 2: the un-refused patch is committed, so GetDomain reports the switched provider instead of the domain's unchanged state (fixture cloudwatchomni-deep-link-and-identity-center-refusals, step task#7).
  AWS: Smithy GetDomain is @readonly and returns the required `domain` member (Domain). Recorded behavior: because the preceding UpdateDomain was refused with 400, the domain is left exactly as it was and GetDomain reports $.domain.identityProviders[0] == 'IAM'.
  Emulator: GetDomain answers 200 with $.domain.identityProviders[0] == 'IDC' — the rejected mutation was written to the store before (and instead of) any validation.
  Why: a refusal that still mutates state is worse than a wrong status: a client that handled the 400 and moved on reads a domain whose identity configuration it never successfully set, and the divergence is permanent for the life of the domain. It also shows the validation gap of cause 1 is not merely a wrong status code but an ordering defect — the store is written before the precondition is evaluated.
  Fix: the same UpdateDomain handler — evaluate all input/precondition validation before the domain record is updated, so a refused update leaves identityProviders and identityProviderConfiguration untouched.
  Traces: cloudwatchomni-deep-link-and-identity-center-refusals#task#7 (GetDomain, 200, $.domain.identityProviders[0] expected 'IAM', got 'IDC'). The host_not_own verdicts on $.domain.domainEndpointUrl / customEndpointUrls[0] in this same step belong to open issue 20261112T102024Z-cloudwatchomni-publishes-domain-endpoint-urls-on-a-cloudwatch.md and are not part of this issue.

## Evidence
Fixture cloudwatchomni-deep-link-and-identity-center-refusals, verdict mismatch on both steps.

Cause 1 — step task#6, UpdateDomain, expected status 400, actual 200. Replay detail:
  status: expected 400, got 200
  $.__type: missing (expected 'com.amazon.cloudwatchomnifrontend#ValidationException', got None)
  $.message: missing (expected 'identityCenterInstanceArn is required when authorizationProviders includes IDC', got None)
  $.domain.domainEndpointUrl: host_not_own (expected None, got 'https://d-<id>.cloudwatch-omni.global.app.aws')
  $.domain.customEndpointUrls[0]: host_not_own (expected None, got 'https://<domain-name>.cloudwatch-omni.global.app.aws')
The expected message is entirely fixed text — no variable parts. The two URL members are flagged only because no response body should exist at all here.

Cause 2 — step task#7, GetDomain, expected status 200, actual 200. Replay detail:
  $.domain.identityProviders[0]: value_mismatch (expected 'IAM', got 'IDC')
Expected: the domain still carries the provider list it had before the refused UpdateDomain. Actual: the refused switch is persisted and served.

Smithy cited: UpdateDomain @idempotent, input domainId (required, @pattern ^d-[0-9a-z]{1,25}$), name (@length 3..63, @pattern ^[a-z0-9]+(-[a-z0-9]+)*$), identityProviders (IdentityProviderList @length 1..2 @uniqueItems), identityProviderConfiguration; errors AccessDenied 403, Conflict 409, InternalServer 500, ResourceNotFound 404, ServiceQuotaExceeded 402, Throttling 429, Validation 400. IdentityProviderConfiguration.identityCenterConfiguration doc: "Identity Center configuration. Required when identityProviders includes IDC." GetDomain @readonly, output domain required. The refusal message text is recorded behavior only.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus and follow fixture 'cloudwatchomni-deep-link-and-identity-center-refusals' to step task#6: a domain exists with identityProviders == ["IAM"] and no Identity Center configuration.
3. Cause 1 — step task#6: call UpdateDomain(domainId=<that domain>, identityProviders=["IDC"]) with NO identityProviderConfiguration. Expected: HTTP 400, __type com.amazon.cloudwatchomnifrontend#ValidationException, message exactly 'identityCenterInstanceArn is required when authorizationProviders includes IDC' (no variable parts). Actual: HTTP 200 with the patch applied.
4. Cause 2 — step task#7: call GetDomain(domainId=<that domain>). Expected: $.domain.identityProviders == ["IAM"], i.e. the refused update left the domain untouched. Actual: $.domain.identityProviders == ["IDC"].
5. Re-replay both steps after fixing: cause 2 is only confirmed by the GetDomain read-back, so fixing the status alone without moving validation ahead of the store write leaves it open.
