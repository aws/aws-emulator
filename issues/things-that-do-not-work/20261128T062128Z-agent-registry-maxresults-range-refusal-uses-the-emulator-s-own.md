Issue-Source: trace-sweep-divergence
Issue-Trace: agent-registry-search-discoverable-registry-records-validation-and-not-found
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: agent-registry-search-discoverable-registry-records-validation-and-not-found
Issue-Shape-Id: com.amazonaws.agentregistry#AgentRegistry
Issue-Key: agent-registry|SearchDiscoverableRegistryRecords|4xx->4xx|$.message
Issue-Commit: 74825bb7b17dbb57a26bde46f40a20da6c3d756d

# maxResults range refusal uses the emulator's own sentence instead of AWS's constraint-violation message

> _Note: this issue was identified against the code base at commit `74825bb7b17dbb57a26bde46f40a20da6c3d756d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchDiscoverableRegistryRecords

## Description
One cause.

Cause 1 of 1: SearchDiscoverableRegistryRecords refuses an out-of-range `maxResults` with the right status (400) and the right error shape (ValidationException), but the `message` is worded by this crate rather than in AWS's constraint-violation form. Fixture `agent-registry-search-discoverable-registry-records-validation-and-not-found`, step `task#4`.

AWS: `maxResults` is a constrained input member — Smithy: SearchDiscoverableRegistryRecords.maxResults targets Integer with @range{min:1,max:50} and @default 10, and its own member doc narrows the admissible range by search type ("Valid values are 1 through 50 when searchType is KEYWORD, and 1 through 20 when it is HYBRID or SEMANTIC"). A value past that upper bound is refused with modeled error ValidationException (httpError 400). The recorded response shows AWS wording the refusal in the standard framework constraint-violation form:

    HTTP 400, ValidationException,
    message = "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 20"

Fixed text of that message, which is a requirement: `"<n> validation error detected: Value at '<member>' failed to satisfy constraint: Member must have value less than or equal to <max>"`. Its variable parts are taken from the request and the model, not from any one run: `<n>` is the number of constraint violations detected in the request, `<member>` is the offending input member's name as the model spells it (`maxResults`), and `<max>` is the upper bound in force for that request — 20 for a HYBRID/SEMANTIC search, 50 for a KEYWORD one, per the member's documented range. Note that AWS's message does NOT echo the rejected value for this violation class; it names only the member and the bound it failed.

Emulator: same status and same error shape, but the message is this crate's own member-and-sentence form: `maxResults: 21 is outside the valid range 1 through 20.` (replay detail: `$.message: value_mismatch`, expected vs actual as above). It is assembled by `page_size`, which formats `"{requested} is outside the valid range {min} through {max}."` and hands it to the shared `field_validation(member, message)` helper, which prefixes `"{member}: "`. So none of AWS's fixed text is present: no "N validation error detected" prefix, no "Value at '<member>' failed to satisfy constraint" clause, no "Member must have value less than or equal to <max>" clause — and it adds the rejected value, which AWS's message for this class omits.

Why: the error message is part of the contract for a validation refusal. Clients, CLI output, SDK retry/classification helpers and test assertions routinely match on AWS's constraint-violation text — the "N validation error detected: ...failed to satisfy constraint..." shape is uniform across AWS services and is what tooling greps for, including the "Member must have value less than or equal to <max>" clause that tells a caller the bound it must respect. A caller built against AWS sees a 400 ValidationException whose message matches nothing it expects, and cannot read the enforced limit out of it in the documented way; worse, prose-matching code treats the refusal as an unrecognized error class.

Fix: belongs in this operation's input validation / refusal wording — the shared page-size range check and the shared validation-message builder it uses, so the range violation is rendered in AWS's constraint-violation form (error count prefix, `Value at '<member>' failed to satisfy constraint:`, `Member must have value less than or equal to <max>`) with the member name and the in-force upper bound substituted. Because the helper is shared by every paginated read of this service, the same wording defect is latent on the other reads that call it; fix it in the one place so they do not drift.

Traces: fixture `agent-registry-search-discoverable-registry-records-validation-and-not-found`, step `task#4` (the only cluster member). Code sites for orientation: `services/service-com-amazonaws-agentregistry-agentregistry/src/state.rs:167-182` (`page_size`, which formats the range sentence) and `src/state.rs:124-133` (`field_validation`, which prefixes `"{member}: "` and sets reason `FieldValidationFailed`); the caller is `services/service-com-amazonaws-agentregistry-agentregistry/src/ops/search_discoverable_registry_records.rs:198-212`. Existing open agent-registry issues `20261125T070825Z-agent-registry-searchdiscoverableregistryrecords-validation.md` (empty `$and`/`$or` operand arrays) and `20261127T171154Z-agent-registry-searchdiscoverableregistryrecords-wrong-behavior.md` (KEYWORD/HYBRID ranking fields) are about this same operation but neither covers the `maxResults` refusal message, so this is net-new.

## Evidence
Replay cluster (divergence_key `service=agent-registry|SearchDiscoverableRegistryRecords|status=400->400|keys=$.message`), fixture `agent-registry-search-discoverable-registry-records-validation-and-not-found`, step `task#4`, verdict `mismatch`, affected_count 1.

Expected status 400 / actual status 400 — the divergence is the body member `$.message`:

  Expected (recorded AWS):
    "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 20"

  Actual (emulator):
    "maxResults: 21 is outside the valid range 1 through 20."

Model support: smithy_query(operation="SearchDiscoverableRegistryRecords") reports input member `maxResults` -> Integer, range {min:1, max:50}, default 10, and modeled error `ValidationException` with httpError 400. The operation's member doc narrows the effective bound to 1..20 for HYBRID/SEMANTIC and 1..50 for KEYWORD, which is the `<max>` AWS substitutes into the message (20 here, matching the search type used by the recorded step).

Fixed vs variable parts of the AWS message: fixed = `"<n> validation error detected: Value at '"`, `"' failed to satisfy constraint: Member must have value less than or equal to "`; variable = the violation count `<n>` (1 here), the member name `maxResults`, and the upper bound `20` (the limit in force for the request's search type). The rejected value (21 in this recording) is a per-request value and does not appear in AWS's message at all.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture `agent-registry-search-discoverable-registry-records-validation-and-not-found`; step `task#4` mismatches on `$.message`.
2. Equivalently by hand: POST /discoverable-records-search to the agent-registry endpoint with a body naming exactly one registry, a searchQuery, a HYBRID or SEMANTIC searchType (or no searchType, so the default HYBRID/SEMANTIC bound of 20 applies), and `"maxResults": 21`.
3. Observe HTTP 400 with error `ValidationException` and message `maxResults: 21 is outside the valid range 1 through 20.`
4. AWS answers HTTP 400 `ValidationException` with message `1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 20` — same status and error code, different message text.
5. Repeat with `"searchType": "KEYWORD"` and `"maxResults": 51` to confirm the same wording defect with the other documented bound (`...less than or equal to 50`).
