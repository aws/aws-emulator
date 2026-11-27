Issue-Source: trace-sweep-divergence
Issue-Trace: simpledbv2-start-export-domain-name-format-refused
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: simpledbv2-start-export-domain-name-format-refused
Issue-Shape-Id: com.amazonaws.simpledbv2#SimpleDBv2
Issue-Key: sdb|StartDomainExport|4xx->4xx|$.message
Issue-Key: sdb|StartDomainExport|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 43ebd6ce98e7173c42d111a904ff9f0222d298b2

# StartDomainExport never enforces SimpleDB's DomainName syntax rule, refusing a malformed name as a nonexistent domain

> _Note: this issue was identified against the code base at commit `43ebd6ce98e7173c42d111a904ff9f0222d298b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDomainExport

## Description
This issue bundles ONE cause; it must be closed before submitting.

Cause 1 of 1: StartDomainExport does not validate `domainName` against SimpleDB's domain-name syntax rule, so a syntactically invalid name is carried into the domain-registry lookup and refused as a resource that does not exist (fixture simpledbv2-start-export-domain-name-format-refused, step task#2; same cluster: task#3, task#4).

  AWS: Smithy (services/service-com-amazonaws-simpledbv2-simpledbv2/smithy-model/com-amazonaws-simpledbv2-simpledbv2.json): StartDomainExport POST /v2/StartDomainExport, input member `domainName` -> DomainName (@required, @length min 1, doc "The domain name that uniquely identifies a SimpleDB domain within your account."); modeled errors ConflictException, InvalidParameterCombinationException, NoSuchDomainException (all client/400), NumberExportsLimitExceeded (409), plus the service-common InvalidParameterValueException (client/400, doc "The specified parameter value is not valid."). The model states only a minimum length, so the SYNTAX of a SimpleDB domain name is recorded behavior: the service validates the name's own form BEFORE it resolves the name against the account's domains, and a name that is not a well-formed SimpleDB domain name is refused with the parameter-value error, not with the not-found error. The recorded response for such a name is HTTP 400 with x-amzn-errortype naming the invalid-parameter-value error and message "Found invalid value for parameter DomainName. DomainName must be between 3 and 255 characters long." In that message the FIXED text is the service's standard invalid-parameter form "Found invalid value for parameter <Parameter>. <Parameter> must <requirement>."; the VARIABLE parts are the faulted parameter named in its API-documentation casing (`DomainName`, taken from the request member) and the requirement clause stating the bound the submitted value missed (here the 3..255 character length of a SimpleDB domain name). The offending value itself is NOT echoed. The ordering is the requirement: a value-level fault on `domainName` precedes the existence check, so a malformed name can never be reported as a missing domain.

  Emulator: the handler's only `domainName` value rule is the one its minted exportArn imposes — reject '/', ':' and ASCII control characters, and reject a name long enough to push the ARN past 2048 characters. Every other malformed name (notably one shorter than 3 characters, one longer than 255, or one carrying characters SimpleDB does not admit) passes that gate, reaches the registry lookup, and is answered HTTP 400 with x-amzn-errortype: NoSuchDomainException and message "The specified domain does not exist." — the service's not-found sentence for a resource the account does not hold. Status matches (400 expected, 400 actual); the error type and the message diverge.

  Why: a client that submits a mistyped or out-of-range domain name is told the domain does not exist instead of being told which parameter it got wrong and what the requirement is. The message is the only part of the response that identifies the faulted parameter, so the caller cannot distinguish "you wrote an impossible name" from "create the domain first" and will retry or create-then-retry forever. SDK error branching also keys on the error type: code written against AWS catches the invalid-parameter-value error for this input and the not-found error for a genuinely absent domain, and here the two are swapped. Because the check is also used to screen operator-declared domains, a name AWS would never accept can additionally be admitted into the registry.

  Fix: the StartDomainExport handler's input validation — specifically the shared `domainName` value rule it delegates to — must state SimpleDB's domain-name syntax (3..255 characters and the character set a SimpleDB domain name admits) in addition to the ARN-safety rule it states today, and must raise InvalidParameterValueException with the service's `Found invalid value for parameter DomainName. DomainName must …` sentence from that rule, before the registry/existence lookup runs. The same rule governs ListExports' `domainName` filter and the operator-declared-domain screen, so it belongs in the one place both already call.

  Traces: simpledbv2-start-export-domain-name-format-refused#task#2 (representative), #task#3, #task#4.

Related open issues that do NOT cover this step: 20261110T055258Z-sdb-wire-replay-domain-export-lifecycle-fixture-refused-end.md covers the unserved 2009-04-15 query surface and the registry being unpopulated for fixture simpledbv2-export-domain-to-s3-succeeds (expected 200, got 400) — a different fixture and a different fault; 20261023T081506Z-sdb-simpledbv2-client-errors-carry-non-aws-validation-message.md covers only the WORDING of three already-correctly-classified v2 refusals (GetExport exportArn, ListExports nextToken, StartDomainExport s3Bucket pattern anchors) and carries no key for this fixture. Neither carries the replay key for simpledbv2-start-export-domain-name-format-refused.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture `simpledbv2-start-export-domain-name-format-refused`, divergence key `service=sdb|StartDomainExport|status=400->400|keys=$.message,$headers.x-amzn-errortype` (verdict mismatch; cluster members simpledbv2-start-export-domain-name-format-refused#task#2, #task#3, #task#4):

- status: expected 400, got 400 — the status is correct; the classification is not.
- $.message: value_mismatch
    expected: "Found invalid value for parameter DomainName. DomainName must be between 3 and 255 characters long."
    actual:   "The specified domain does not exist."
- $headers.x-amzn-errortype: shape_mismatch — actual "NoSuchDomainException", where the recorded refusal is the invalid-parameter-value error named by the message's "Found invalid value for parameter DomainName" form.

The emulator's sentences themselves are correct for their classes: services/service-com-amazonaws-simpledbv2-simpledbv2/src/state/refusal_message.rs:59-62 builds exactly "Found invalid value for parameter <Parameter>. <Parameter> must <requirement>.", and :77-82 builds "The specified domain does not exist.". The defect is which one this input reaches.

Code evidence for the root cause:
- services/service-com-amazonaws-simpledbv2-simpledbv2/src/state.rs:598-620 (`domain_name_rejection`): the only refusals are (a) a name containing '/', ':' or an ASCII control character and (b) a name whose minted export ARN would exceed MAX_EXPORT_ARN_LEN (2048). Its own doc comment states the premise — "`DomainName` carries only `@length(min: 1)`, so the centralized walk admits any text; the rule enforced here is the one the minted ARN imposes" — i.e. SimpleDB's own 3..255 length and character-set rule is nowhere enforced.
- services/service-com-amazonaws-simpledbv2-simpledbv2/src/ops/start_domain_export.rs:234-240: that rejection is consulted first (good ordering), so once it returns None the request proceeds.
- services/service-com-amazonaws-simpledbv2-simpledbv2/src/ops/start_domain_export.rs:278-288: `!store.has_domain(&input.domain_name)` then returns NoSuchDomainException with `no_such_resource(DOMAIN_RESOURCE)` — the answer every malformed-but-ARN-safe name ends up with.
- services/service-com-amazonaws-simpledbv2-simpledbv2/src/state.rs:62-69 (`parse_existing_domains`) screens operator-declared domains through the same incomplete rule.

Fixture score for this replay: 23 steps total, 19 passed, 4 mismatched; 3 of the 4 mismatched steps are this cluster.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Observe fixture `simpledbv2-start-export-domain-name-format-refused` diverge at steps task#2, task#3 and task#4 — status 400 matches, but `$.message` and `$headers.x-amzn-errortype` do not.
3. By hand: POST /v2/StartDomainExport with {"domainName": "ab", "s3Bucket": "wire-export-bucket"} (a name shorter than SimpleDB's 3-character minimum; likewise try a name longer than 255 characters, and one carrying a character a SimpleDB domain name does not admit).
4. Observe HTTP 400 with x-amzn-errortype: NoSuchDomainException and body message "The specified domain does not exist.", instead of the invalid-parameter-value error whose message states the faulted parameter and its requirement in the form "Found invalid value for parameter DomainName. DomainName must …".
5. Confirm the ordering independently: repeat step 3 with the name of a domain that DOES exist in the registry but is still syntactically invalid — the request must still be refused on the parameter value, never accepted.
6. Re-replay to confirm the fix: simpledbv2-start-export-domain-name-format-refused#task#2, #task#3, #task#4.
