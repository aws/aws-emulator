Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-recommended-actions-error-invalid-filter-key
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-recommended-actions-error-invalid-filter-key bcm-recommended-actions-error-invalid-match-option
Issue-Shape-Id: com.amazonaws.bcmrecommendedactions#AWSBillingAndCostManagementRecommendedActions
Issue-Key: bcm-recommended-actions|ListRecommendedActions|4xx->4xx|$.fieldList[name="filter.actions.1.member.key"].message
Issue-Key: bcm-recommended-actions|ListRecommendedActions|4xx->4xx|$.fieldList[name="filter.actions.1.member.matchOption"].message
Issue-Commit: 48f5ae65361dbef57cab4f34f481002587fd379a

# ListRecommendedActions ValidationException fieldList message repeats the member path AWS keeps only in fieldList.name

> _Note: this issue was identified against the code base at commit `48f5ae65361dbef57cab4f34f481002587fd379a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRecommendedActions

## Description
ListRecommendedActions refuses an out-of-enum filter member with the right HTTP status (400), the right modelled error shape (ValidationException with message/reason/fieldList) and — after the earlier fix — the right summary message and the right enum value-set order. What still diverges is the PER-FIELD clause: every entry of ValidationException$fieldList repeats the dotted member path inside its `message`, where AWS states the path only once, in that entry's `name`. Two causes, one per refused enum member, bundled because they share one service, one operation and one rendering site.

Cause 1 of 2: the fieldList entry for ActionFilter$key carries an ` at '<member path>'` clause AWS does not put in that message.
  AWS: Smithy (model com-amazonaws-bcmrecommendedactions): ListRecommendedActions.filter -> RequestFilter$actions -> ActionFilter$key targets the enum FilterName (FEATURE, SEVERITY, TYPE), @required; an unmodelled value is refused with ValidationException (errorKind client, httpError 400). ValidationException$fieldList targets ValidationExceptionFieldList of ValidationExceptionField, whose members are documented as name = "Provides the name of the field that failed validation" and message = "Provides a message explaining why the field failed validation" — the path is the `name` member's job. The model does not state the message text, so the wording is recorded behavior only: the recording's fieldList entry has name = the dotted member path and message = `Value <rejected value> failed to satisfy constraint: Member must satisfy enum value set: [FEATURE, TYPE, SEVERITY]`, with the request-supplied value span redacted by the fixture as `[redacted]`. Fixed text of this message class: the literal `Value ` prefix, the ` failed to satisfy constraint: Member must satisfy enum value set: [` / `]` frame, and the enum listing in AWS's order for this member (FEATURE, TYPE, SEVERITY). Variable parts, taken from the request and the model: the rejected value (quoted, redacted in the fixture, so the value itself is NOT a requirement — only its position in the template is) and the value set from the model. There is NO member-path clause: the path is carried by the sibling `name`, and the fixture does not redact member paths (the summary message's unredacted `Value at 'filter.actions.1.member.key'` shows a path is reproduced verbatim when AWS emits one), so its absence from the expected fieldList message is AWS's rendering, not redaction.
  Emulator: emits the summary clause correctly but renders the fieldList entry's message with the path spliced in: `Value 'NOT_A_REAL_FILTER_KEY' at 'filter.actions.1.member.key' failed to satisfy constraint: Member must satisfy enum value set: [FEATURE, TYPE, SEVERITY]` for an entry whose `name` is already `filter.actions.1.member.key`. Status 400, error code ValidationException, reason fieldValidationFailed and the enum order are all correct; only this message text diverges.
  Why: a client, SDK test or operator that reads or matches the per-field explanation gets a sentence AWS never returns, and the path is reported twice in one entry — so an assertion pinned to AWS's per-field wording fails on every refusal, and a caller that concatenates name + message to build an operator-facing error renders the member path twice.
  Fix: the fieldList rendering in this operation's refusal path — `field_validation_failed` builds each ValidationExceptionField with `message: violation.message()`, and the framework's `Violation::message()` is the path-bearing clause the summary envelope wants. It needs the mirror of the existing `without_echoed_value` projection (a path-free clause that keeps the rejected value) for the per-field half, so the summary states path+requirement and the entry states value+requirement.
  Traces: bcm-recommended-actions-error-invalid-filter-key#task#1. Code: services/service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions/src/ops/list_recommended_actions.rs:240-272 (field_validation_failed, the `message: violation.message()` at line 267) and its doc comment at lines 234-239, which already states the intended split.

Cause 2 of 2: the same path duplication in the fieldList entry for ActionFilter$matchOption, confirming it is the rendering site and not one member's special case.
  AWS: Smithy: ActionFilter$matchOption targets the enum MatchOption (EQUALS, NOT_EQUALS), @required; an unmodelled value is refused with ValidationException (400). Message text is recorded behavior only: the fieldList entry's name is the member path and its message is `Value <rejected value> failed to satisfy constraint: Member must satisfy enum value set: [NOT_EQUALS, EQUALS]` (value span redacted in the fixture). Same fixed text and same variable parts as cause 1, with this member's value-set order (NOT_EQUALS, EQUALS) — again with no member-path clause in the message.
  Emulator: `Value 'NOT_A_REAL_MATCH_OPTION' at 'filter.actions.1.member.matchOption' failed to satisfy constraint: Member must satisfy enum value set: [NOT_EQUALS, EQUALS]` on an entry whose `name` is `filter.actions.1.member.matchOption`.
  Why: same client impact as cause 1, on a second member — and it shows the defect is in the shared per-field clause builder, so a fix scoped to one member's refusal would leave the other diverging.
  Fix: same component as cause 1 — one path-free per-field clause used for every violation, whatever member or refusal path produced it.
  Traces: bcm-recommended-actions-error-invalid-match-option#task#1. Code: same rendering site, src/ops/list_recommended_actions.rs:262-270.

RELATED ISSUES: the RESOLVED wire issue 20261029T211113Z-bcm-recommended-actions-listrecommendedactions-error-messages-diverge-enum-value-set-order covered these two fixtures for the enum value-set ORDER, the summary message's echoed value and the nextToken wording; those three are now correct (the malformed-next-token step and the summary assertions pass), and this residual per-field path duplication is what remains — a resolved issue does not cover it. The OPEN behavioral issue 20261029T214023Z-bcm-recommended-actions-listrecommendedactions-validationexception-message-rendering-diverges-rejected-value-echoed is about the SUMMARY clause echoing the value for the @length/@range classes and about the nextToken sentence, in the behavioral corpus; it carries no wire replay key and says nothing about the fieldList entry repeating the path, so a fixer closing it would not re-replay these two fixture steps.

## Evidence
Replay of trace set simparity-wire-aws-v1 against this checkout: 6 steps, 4 passed, 2 mismatched, score 66.7%; emulator up (/__admin/health 200). Both mismatches are service=bcm-recommended-actions, operation=ListRecommendedActions, expected_status=400 / actual_status=400 — status, error shape, reason and enum ordering all match; the divergence is confined to $.fieldList[*].message.

Cause 1 — fixture bcm-recommended-actions-error-invalid-filter-key, step task#1:
  $.fieldList[name="filter.actions.1.member.key"].message
    expected: "Value [redacted] failed to satisfy constraint: Member must satisfy enum value set: [FEATURE, TYPE, SEVERITY]"
    actual:   "Value 'NOT_A_REAL_FILTER_KEY' at 'filter.actions.1.member.key' failed to satisfy constraint: Member must satisfy enum value set: [FEATURE, TYPE, SEVERITY]"
  The enum listing now matches; the diverging span is the extra " at '<member path>'" clause. Fixed text vs values: `Value ` and ` failed to satisfy constraint: Member must satisfy enum value set: [`...`]` plus the member's value set are fixed; the quoted rejected value comes from the request (the fixture redacts it, so 'NOT_A_REAL_FILTER_KEY' is this recording's input, not a requirement) and 'filter.actions.1.member.key' is this recording's path — which belongs in the entry's `name`, not in its `message`.

Cause 2 — fixture bcm-recommended-actions-error-invalid-match-option, step task#1:
  $.fieldList[name="filter.actions.1.member.matchOption"].message
    expected: "Value [redacted] failed to satisfy constraint: Member must satisfy enum value set: [NOT_EQUALS, EQUALS]"
    actual:   "Value 'NOT_A_REAL_MATCH_OPTION' at 'filter.actions.1.member.matchOption' failed to satisfy constraint: Member must satisfy enum value set: [NOT_EQUALS, EQUALS]"

Model cross-check (smithy_query): ListRecommendedActions errors include ValidationException (client, httpError 400); ValidationException members message (required), reason (required, ValidationExceptionReason), fieldList (ValidationExceptionFieldList); ValidationExceptionField members name (required, "Provides the name of the field that failed validation") and message (required, "Provides a message explaining why the field failed validation"); ActionFilter$key -> FilterName @required, ActionFilter$matchOption -> MatchOption (EQUALS, NOT_EQUALS) @required. The model does not specify message text, so the per-field wording is recorded behavior only.

The other four steps of this service's fixtures pass, including the malformed-next-token refusal and the summary $.message assertions, so only the per-field clause is still wrong.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: POST an awsJson ListRecommendedActions request whose filter.actions[0].key is a string outside the FilterName enum (fixture bcm-recommended-actions-error-invalid-filter-key, step task#1). The response is 400 ValidationException with reason fieldValidationFailed and the correct $.message; compare $.fieldList[name="filter.actions.1.member.key"].message against the recorded body — the emulator adds an " at '<member path>'" clause that AWS's per-field message does not carry (the path is already the entry's `name`).
3. Cause 2: the same request with filter.actions[0].matchOption outside the MatchOption enum (fixture bcm-recommended-actions-error-invalid-match-option, step task#1); $.fieldList[name="filter.actions.1.member.matchOption"].message shows the same duplicated path.
4. Confirm the scope: $.message, $.reason, the HTTP status and the enum value-set order all match in both steps, and the other four steps of the corpus for this service pass — so re-replay both fixtures after changing only the fieldList clause rendering.
