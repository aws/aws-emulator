Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: accessanalyzer__dynamodb__iam__sts.attributes-condition-on-put-item-warned-and-put-drops-unlisted-attribute accessanalyzer__dynamodb__iam__sts.attributes-without-select-warning-leaks-whole-item-fixed-policy-denied accessanalyzer__dynamodb__iam__sts.invalid-select-value-error-and-query-grant-has-no-effect-rejected accessanalyzer__dynamodb__iam__sts.select-key-on-put-item-deny-warned-and-ignored-return-values-deny-rejected
Issue-Key: accessanalyzer__dynamodb__iam__sts.attributes-condition-on-put-item-warned-and-put-drops-unlisted-attribute|run#16|AssertionError|assert put_issues == {
Issue-Key: accessanalyzer__dynamodb__iam__sts.attributes-without-select-warning-leaks-whole-item-fixed-policy-denied|run#15|AssertionError|assert warned == [("SECURITY_WARNING", "DYNAMODB_ATTRIBUTES_WITHOUT_SELECT", _FINDING_DETAILS)], (
Issue-Key: accessanalyzer__dynamodb__iam__sts.invalid-select-value-error-and-query-grant-has-no-effect-rejected|run#14|AssertionError|assert broken == [(
Issue-Key: accessanalyzer__dynamodb__iam__sts.select-key-on-put-item-deny-warned-and-ignored-return-values-deny-rejected|run#16|AssertionError|assert wrong == [("WARNING", "DYNAMODB_WRONG_CONDITION_FOR_ACTION", _WRONG_KEY_DETAILS)], (
Issue-Commit: 7c686d71091806786cdd6101a8e36a3d7eadfcb8

# accessanalyzer ValidatePolicy emits no DynamoDB condition-key findings — four distinct finding classes are all absent (4 causes)

> _Note: this issue was identified against the code base at commit `7c686d71091806786cdd6101a8e36a3d7eadfcb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ValidatePolicy

## Description
ValidatePolicy answers HTTP 200 with an EMPTY `findings` list for every identity policy whose statements carry DynamoDB attribute-level condition keys (`dynamodb:Attributes`, `dynamodb:Select`, `dynamodb:ReturnValues`). The operation is reachable and well-formed — it is the policy-check corpus for these condition keys that is missing. Four distinct finding classes are each a separate missing check and must be closed separately; the empty list is only the symptom they share.

Cause 1 of 4: an `Attributes`-limited `dynamodb:PutItem` grant draws no finding.
AWS: Smithy: ValidatePolicy POST /policy/validation, output member `findings` is @required (ValidatePolicyFindingList); each finding carries issueCode, findingType (ValidatePolicyFindingType) and findingDetails. Recorded behavior for the corpus itself: a statement allowing `dynamodb:PutItem` under `ForAllValues:StringEquals {dynamodb:Attributes: [...]}` draws EXACTLY two findings — issueCode `DYNAMODB_REPLACE_OPERATION_WITH_ATTRIBUTES` at findingType `WARNING`, whose findingDetails is the fixed sentence "The dynamodb:PutItem action replaces the entire item, so dynamodb:Attributes cannot restrict access at the attribute level. That condition only applies to UpdateItem; use dynamodb:UpdateItem for attribute-level access control.", and issueCode `DYNAMODB_ATTRIBUTES_WITHOUT_RETURN_VALUES` at findingType `SECURITY_WARNING`, whose findingDetails is the fixed sentence "Restricting dynamodb:Attributes without limiting dynamodb:ReturnValues allows write operations to return the entire item, which can expose attributes outside the allowed list. We recommend that you restrict dynamodb:ReturnValues to NONE, UPDATED_OLD, or UPDATED_NEW." The SAME condition on `dynamodb:UpdateItem` draws only the second of those two (the replace-operation warning does NOT apply to UpdateItem). The action name in the warning text is the statement's own action; the rest of each sentence is fixed text.
Emulator: `findings` is `[]` for both the PutItem and the UpdateItem statement.
Why: a client using Access Analyzer as a policy gate is told an attribute-level PutItem restriction is sound. It is not: PutItem replaces the whole item, so the condition silently does nothing, and unrestricted ReturnValues hands the whole item back. The gate passes the exact policy it exists to flag.
Fix: the accessanalyzer ValidatePolicy handler's policy-check corpus — add the DynamoDB write-action/Attributes checks and their findingType/issueCode/findingDetails.
Traces: accessanalyzer__dynamodb__iam__sts.attributes-condition-on-put-item-warned-and-put-drops-unlisted-attribute.

Cause 2 of 4: an `Attributes`-only `dynamodb:GetItem` grant draws no attributes-without-select warning.
AWS: recorded behavior only for the corpus. A statement allowing `dynamodb:GetItem` under `ForAllValues:StringEquals {dynamodb:Attributes: [...]}` and NO `dynamodb:Select` condition draws EXACTLY one finding: issueCode `DYNAMODB_ATTRIBUTES_WITHOUT_SELECT` at findingType `SECURITY_WARNING`. Adding `StringEquals {dynamodb:Select: "SPECIFIC_ATTRIBUTES"}` to the same statement draws NO findings — the warning is driven by the absence of the Select condition, not by the presence of Attributes.
Emulator: `findings` is `[]` for the Attributes-only statement (and, trivially, for the paired one).
Why: the policy that leaks the whole item through an unprojected read is reported as clean, and the client cannot tell the unsafe statement from its safe twin — the two answer identically.
Fix: the same policy-check corpus — add the Attributes-without-Select read check.
Traces: accessanalyzer__dynamodb__iam__sts.attributes-without-select-warning-leaks-whole-item-fixed-policy-denied.

Cause 3 of 4: an invalid `dynamodb:Select` condition VALUE draws no error finding.
AWS: recorded behavior only for the corpus. A statement conditioning `dynamodb:Query` on `StringEquals {dynamodb:Select: "<lowercase or otherwise non-enum value>"}` draws EXACTLY one finding at findingType `ERROR`, issueCode `DYNAMODB_INVALID_SELECT_VALUE`, with findingDetails "The value for dynamodb:Select is not valid. Update <the offending value> to one of ALL_ATTRIBUTES, ALL_PROJECTED_ATTRIBUTES, SPECIFIC_ATTRIBUTES, or COUNT." — the offending value is taken from the request's policy document; the surrounding sentence and the four-value list are fixed text. The exact value `SPECIFIC_ATTRIBUTES` draws no findings.
Why this matters: the condition value is case-sensitive, so such a statement matches no request at all and the grant is dead.
Emulator: `findings` is `[]`, so the dead grant is reported as valid; the same policy then refuses the Query it was written to allow, with no diagnostic pointing at the typo.
Fix: the same policy-check corpus — validate condition VALUES of enum-valued DynamoDB condition keys and emit the ERROR finding.
Traces: accessanalyzer__dynamodb__iam__sts.invalid-select-value-error-and-query-grant-has-no-effect-rejected.

Cause 4 of 4: a `Select`-keyed Deny on a WRITE action draws no wrong-condition-for-action warning.
AWS: recorded behavior only for the corpus. A `Deny` on `dynamodb:PutItem` conditioned on `StringEquals {dynamodb:Select: ...}` draws EXACTLY one finding at findingType `WARNING`, issueCode `DYNAMODB_WRONG_CONDITION_FOR_ACTION`, whose findingDetails ends with the fixed guidance "Use dynamodb:Select with read actions and dynamodb:ReturnValues with write actions." The same Deny keyed on `dynamodb:ReturnValues` instead draws NO findings.
Emulator: `findings` is `[]` for the Select-keyed Deny.
Why: `dynamodb:Select` is never present in a PutItem request context, so the Deny never fires — a client believes it has blocked `ReturnValues=ALL_OLD` writes while they are served and return the replaced item in full. Access Analyzer is the tool that is supposed to catch exactly this mis-keyed condition.
Fix: the same policy-check corpus — add the read-key-on-write-action (and write-key-on-read-action) condition check.
Traces: accessanalyzer__dynamodb__iam__sts.select-key-on-put-item-deny-warned-and-ignored-return-values-deny-rejected.

## Evidence
Expected (AWS) vs actual (emulator), verbatim from the replay:

1. Expected findings {(DYNAMODB_REPLACE_OPERATION_WITH_ATTRIBUTES, WARNING, <replace-operation sentence>), (DYNAMODB_ATTRIBUTES_WITHOUT_RETURN_VALUES, SECURITY_WARNING, <return-values sentence>)}. Actual: 'AssertionError: an Attributes-limited PutItem grant should draw the replace-operation warning and the attributes-without-return-values warning; findings were []'.

2. Expected [("SECURITY_WARNING", "DYNAMODB_ATTRIBUTES_WITHOUT_SELECT", <details>)]. Actual: 'AssertionError: an Attributes-only GetItem grant should draw exactly the attributes-without-select security warning; findings were []'.

3. Expected [("ERROR", "DYNAMODB_INVALID_SELECT_VALUE", "The value for dynamodb:Select is not valid. Update <value> to one of ALL_ATTRIBUTES, ALL_PROJECTED_ATTRIBUTES, SPECIFIC_ATTRIBUTES, or COUNT.")]. Actual: 'AssertionError: a lowercase Select value should draw exactly the invalid-select-value error; got []'.

4. Expected [("WARNING", "DYNAMODB_WRONG_CONDITION_FOR_ACTION", <details ending "Use dynamodb:Select with read actions and dynamodb:ReturnValues with write actions.">)]. Actual: 'AssertionError: a Select-keyed Deny on PutItem should draw exactly the wrong-condition warning; findings were []'.

In every case ValidatePolicy itself answered 200 with a paginated, empty `findings` list — no error, no partial result. Table names, role names and ARNs in the policy documents are per-run harness values and are not part of the requirement.

## Steps to Reproduce
1. Call accessanalyzer ValidatePolicy (POST /policy/validation) with policyType=IDENTITY_POLICY and a single-statement identity policy, paginating `findings`.
2. Statement A: {"Effect":"Allow","Action":"dynamodb:PutItem","Resource":"<table arn>","Condition":{"ForAllValues:StringEquals":{"dynamodb:Attributes":["sku","label"]}}} — expect DYNAMODB_REPLACE_OPERATION_WITH_ATTRIBUTES (WARNING) + DYNAMODB_ATTRIBUTES_WITHOUT_RETURN_VALUES (SECURITY_WARNING); observe []. Repeat with Action dynamodb:UpdateItem — expect only the SECURITY_WARNING.
3. Statement B: the same Attributes condition on dynamodb:GetItem — expect DYNAMODB_ATTRIBUTES_WITHOUT_SELECT (SECURITY_WARNING); observe []. Add StringEquals dynamodb:Select=SPECIFIC_ATTRIBUTES — expect [].
4. Statement C: Allow dynamodb:Query with Condition StringEquals dynamodb:Select="specific_attributes" — expect one ERROR DYNAMODB_INVALID_SELECT_VALUE naming the value and the four valid values; observe [].
5. Statement D: Allow dynamodb:PutItem plus Deny dynamodb:PutItem conditioned on StringEquals dynamodb:Select=ALL_ATTRIBUTES — expect one WARNING DYNAMODB_WRONG_CONDITION_FOR_ACTION; observe []. Re-key the Deny on dynamodb:ReturnValues=ALL_OLD — expect [].
