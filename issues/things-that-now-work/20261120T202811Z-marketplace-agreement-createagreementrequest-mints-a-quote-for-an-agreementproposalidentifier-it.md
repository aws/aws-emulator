Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-agreement.create-agreement-request-invalid-intent-rejected
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement.create-agreement-request-invalid-intent-rejected|_error_code#6|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 440dcc635d0245d2f1247fff3333ee227686f51d

# CreateAgreementRequest mints a quote for an agreementProposalIdentifier it cannot resolve, instead of ValidationException "Provided agreement proposal is inactive."

> _Note: this issue was identified against the code base at commit `440dcc635d0245d2f1247fff3333ee227686f51d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgreementRequest

## Description
Cause 1 of 1: CreateAgreementRequest never resolves the proposal its `agreementProposalIdentifier` names, so a well-formed identifier that names no active agreement proposal is accepted and a quote (agreementRequestId + chargeSummary) is minted at HTTP 200 where AWS refuses the call.

AWS: Smithy (services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/smithy-model/com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301.json) models CreateAgreementRequestInput.agreementProposalIdentifier -> AgreementProposalId (string, @length 1..64, @pattern ^(at-|ap-)[A-Za-z0-9]+$), documented as "The agreement proposal signed by the proposer ... required if the intent is not AMEND"; `intent` -> enum Intent (NEW | AMEND | REPLACE, @required); modeled errors include ValidationException (errorKind client, @httpError 400, "The input fails to satisfy the constraints specified by the service", carrying message/reason/fields/requestId) and ResourceNotFoundException (404). The model does not state the sentence itself, so the text is recorded behavior only: the recording shows a request whose `intent` IS a legal enum value (NEW) but whose `agreementProposalIdentifier` is pattern-conforming yet names no active proposal is refused with HTTP 400, Error.Code `ValidationException`, Error.Message "Provided agreement proposal is inactive." That message is entirely FIXED text for this refusal class — it interpolates no request value, no id, no limit and no clock; the identifier that provoked it belongs in the request, not in the sentence (the offending member is named in the sibling ValidationExceptionField.name). The recording also establishes that this refusal is distinct from the intent-enum refusal ("Provided intent is invalid."), i.e. a legal intent reaches a SECOND, proposal-resolution check that must still refuse.

Emulator: `create_agreement_request(intent="NEW", requestedTerms=[{"id": <name>}], agreementProposalIdentifier="ap-<identifier naming no proposal>")` returns HTTP 200 with a success body of the shape {"agreementRequestId": "ar-...", "chargeSummary": {"currencyCode": "USD", "newAgreementValue": "0"}} (the particular id and amount are per-run values; the 200 and the success shape are the divergence) and persists the quote. No ValidationException is raised. The handler enforces only the two cross-member presence rules (agreementProposalIdentifier required for non-AMEND, sourceAgreementIdentifier required for non-NEW), then idempotency and quota — there is no proposal resolution on any path.

Why: a client relying on AWS refusing a stale, revoked or mistyped proposal reference gets the opposite signal. It receives an agreementRequestId derived from nothing and can carry it forward into AcceptAgreementRequest, producing an agreement AWS would never have created; negative tests that assert the refusal pass vacuously; and the error class that tells a caller "the proposal you signed is no longer active" never appears, so the quote flow cannot be distinguished from a successful one.

Fix: the CreateAgreementRequest handler's validation path in the marketplace-agreement service crate — after the intent-enum boundary check and the two presence rules, resolve `agreementProposalIdentifier` against the proposals this emulator can hold and refuse an identifier that resolves to nothing/to a non-active proposal with the modeled ValidationException (HTTP 400, code ValidationException, message "Provided agreement proposal is inactive.", reason naming the inactive-proposal class and one `fields` entry naming `agreementProposalIdentifier`), minting nothing. Two previously resolved issues DECLINED exactly this cause on the grounds that no component publishes a proposal identifier this API's @pattern accepts (20261022T212948Z-marketplace-agreement-intent-enum-and-proposal-validation-skipped.md, Cause 2, and the "control leg" note in 20261109T032733Z-marketplace-agreement-createagreementrequest-accepts-an-intent-outside-the-intent-enum.md); both are now closed, no open issue carries this check's replay key, and the behavioral check still reports the divergence, so the cause is refiled here. Closing it needs the proposal-minting side (marketplace-discovery GetOffer$agreementProposalId) to publish identifiers satisfying ^(at-|ap-)[A-Za-z0-9]+$ so that resolvable proposals exist and the quote -> accept -> agreement flow stays reachable while unresolvable identifiers are refused.

Traces: behavioral check marketplace-agreement.create-agreement-request-invalid-intent-rejected (group overly-permissive; axis:rejection, cap:c1). Its first leg (out-of-enum intent) now passes; the failure is at the positive-control leg, check line 37. Handler source without any proposal resolution: services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/ops/create_agreement_request.rs:177-270.

## Evidence
Suite: 16/18 passed; marketplace-agreement 16/18 (89%), 2 failed.

[mismatch] marketplace-agreement.create-agreement-request-invalid-intent-rejected
  Failing call (check line 37, positive control): create_agreement_request(intent="NEW", requestedTerms=[{"id": "sim2real-term-<unique>"}], agreementProposalIdentifier="ap-sim2realnonexistentproposal").
  Expected (AWS, recorded): ClientError — HTTP 400, Error.Code "ValidationException", Error.Message "Provided agreement proposal is inactive." (fixed text; nothing request- or run-derived in it).
  Actual (emulator): HTTP 200, content-type application/x-amz-json-1.0, body {'agreementRequestId': 'ar-0000000000000001', 'chargeSummary': {'currencyCode': 'USD', 'newAgreementValue': '0'}} — a minted quote. Harness assertion (check file line 53): AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'agreementRequestId': 'ar-0000000000000001', ...}.
  The earlier leg of the same check (intent="SUPERSEDE_EVERYTHING" -> ValidationException "Provided intent is invalid.") PASSES, which is what proves this leg is a separate, un-enforced proposal-resolution check rather than a blanket missing refusal.

Model references used: smithy_query operation=CreateAgreementRequest and shape=Intent,ValidationException,AgreementProposalId on services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/smithy-model/com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301.json — intent: enum [NEW, AMEND, REPLACE] @required; agreementProposalIdentifier: @length 1..64, @pattern ^(at-|ap-)[A-Za-z0-9]+$; ValidationException: client, httpError 400, members message/reason/fields/requestId. The refusal sentence itself is not in the model and is cited as recorded behavior only.

Source: services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/ops/create_agreement_request.rs — handle() checks the two intent-conditional presence rules (lines 201-236) and then goes straight to the clock, store lock and idempotency replay (lines 238-269); no lookup of the proposal named by agreement_proposal_identifier exists anywhere in the handler.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a botocore client at it: mpa = boto3.client("marketplace-agreement", region_name="us-east-1", endpoint_url=<emulator>) — the service answers only in us-east-1.
2. Call mpa.create_agreement_request(intent="NEW", requestedTerms=[{"id": "sim2real-term-1"}], agreementProposalIdentifier="ap-sim2realnonexistentproposal") — a pattern-conforming identifier (^(at-|ap-)[A-Za-z0-9]+$) that names no active proposal in the emulator's state.
3. Observe HTTP 200 and a minted quote ({'agreementRequestId': 'ar-...', 'chargeSummary': {...}}). Real AWS raises ClientError with HTTP 400, Code "ValidationException", Message "Provided agreement proposal is inactive."
4. Confirm the first leg is not the problem: mpa.create_agreement_request(intent="SUPERSEDE_EVERYTHING", requestedTerms=[{"id": "sim2real-term-1"}]) is already refused 400 ValidationException "Provided intent is invalid."
5. Replay to confirm a fix: behavioral check marketplace-agreement.create-agreement-request-invalid-intent-rejected — both legs must refuse, each with its own message.
