Issue-Source: trace-sweep-divergence
Issue-Trace: glue-gjwr-traj-catalog-is-regional-under-one-catalog-id
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-gjwr-traj-catalog-is-regional-under-one-catalog-id
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetDatabase|4xx->4xx|$.Message
Issue-Key: glue|GetDatabase|4xx->4xx|$.__type
Issue-Commit: 704c48b6f7bf8dd638f70ff69c421beba4994c4a

# Glue GetDatabase reports a malformed CatalogId as an absent catalog

> _Note: this issue was identified against the code base at commit `704c48b6f7bf8dd638f70ff69c421beba4994c4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDatabase

## Description
One net-new wire-replay root cause: GetDatabase called with a CatalogId that is outside Glue's catalog-id grammar is refused with the WRONG error code and the wrong message — the not-found verdict AWS reserves for a well-formed id that simply names no catalog.

Cause 1 of 1: AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json) models GetDatabase with optional CatalogId (CatalogIdString, @length min 1 max 255) and required Name, and its client errors include BOTH InvalidInputException (errorKind client, httpError 400) and EntityNotFoundException (errorKind client, httpError 400) — so the two verdicts are distinct, modeled outcomes. Recorded response for an id that cannot address a Data Catalog at all: HTTP 400, __type InvalidInputException, Message exactly 'Catalog id is invalid. The catalog id must be prepended with the account id prefix and can only contain lowercase letters, numbers, underscores, and slashes.' — 100% FIXED text: it states the grammar (the id must be the account id, optionally followed by a '/'-separated catalog path, drawn only from lowercase letters, digits, underscores and slashes) and interpolates NOTHING from the request, not even the rejected id. Glue decides this from the request alone, before any catalog is resolved; an id that satisfies the grammar but names no catalog is the other verdict, EntityNotFoundException with the fixed text 'Catalog not found.'. Emulator: the same request answers HTTP 400 with __type EntityNotFoundException and Message 'Catalog not found.' — the right status, but the wrong error code and the message for a different fault, because the grammar predicate accepts an id containing characters the grammar excludes (e.g. hyphens or uppercase) and the request falls through to catalog resolution, which misses. Why: the error code is the only thing a typed client branches on, and the two codes mean opposite things — told EntityNotFoundException/'Catalog not found.', a client (or a federated-catalog tool) concludes the catalog has not been created yet and retries or creates one, when in fact the id it sent can never name a catalog and no amount of provisioning will make the call succeed; the message it reads also states no rule, so the caller never learns the id's grammar. Fix: the catalog-id grammar check that must precede catalog resolution — crate::state::catalog_refusal and the predicate it consults (crate::state::is_data_catalog_id, named in the handler's own doc comment at services/service-com-amazonaws-glue-awsglue/src/ops/get_database.rs:10-13) must return CatalogRefusal::Invalid with AWS's fixed grammar sentence for an id outside the grammar, so the InvalidInputException branch already wired at services/service-com-amazonaws-glue-awsglue/src/ops/get_database.rs:183-195 is taken instead of the NotFound branch at :196-204. The NotFound verdict for a well-formed id naming no catalog must stay as it is. Traces: glue-gjwr-traj-catalog-is-regional-under-one-catalog-id#task#6.

Related open issue, named for the fixer — it does NOT carry this fixture#step (the replay reports this cluster as unowned, and this step is the opposite verdict from the one that issue describes): 20261105T074444Z-glue-catalog-read-and-create-refusals-restate-the.md, cause 4, covers GetTables answering AccessDeniedException where AWS answers EntityNotFoundException/'Catalog not found.' for a WELL-FORMED unknown CatalogId. The two share the catalog-resolution path, so a fixer touching crate::state::catalog_refusal should keep both verdicts distinct: malformed id -> InvalidInputException with the grammar sentence, well-formed unknown id -> EntityNotFoundException/'Catalog not found.'.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (224 fixtures, 1613 steps), service glue. Status-matching refusal (400 -> 400) with a diverging error code and message.

glue-gjwr-traj-catalog-is-regional-under-one-catalog-id#task#6, GetDatabase, expected 400 / got 400, 1 step in cluster:
  $.__type: value_mismatch (expected 'InvalidInputException', got 'EntityNotFoundException')
  $.Message: value_mismatch (expected 'Catalog id is invalid. The catalog id must be prepended with the account id prefix and can only contain lowercase letters, numbers, underscores, and slashes.', got 'Catalog not found.')

Message anatomy: AWS's text is 100% fixed — it states the grammar and interpolates no request value, so nothing in it is per-run. The emulator's text is also fixed but is AWS's wording for a DIFFERENT fault (a well-formed CatalogId naming no catalog), and the code it ships with is not the modeled outcome for a malformed id.

Source confirmation in this checkout: services/service-com-amazonaws-glue-awsglue/src/ops/get_database.rs:180-204 already has both branches (CatalogRefusal::Invalid -> InvalidInputException, CatalogRefusal::NotFound -> EntityNotFoundException) and its doc comment at lines 10-13 states the intended rule; the classification handed back by crate::state::catalog_refusal is what mis-sorts this id.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1), then inspect this step:
1. glue-gjwr-traj-catalog-is-regional-under-one-catalog-id#task#6 — GetDatabase(CatalogId=<an id outside Glue's catalog-id grammar, e.g. one containing hyphens or uppercase letters and not prefixed with the account id>, Name=<any database name>). Expect HTTP 400 with __type InvalidInputException and Message exactly 'Catalog id is invalid. The catalog id must be prepended with the account id prefix and can only contain lowercase letters, numbers, underscores, and slashes.'; observe HTTP 400 with __type EntityNotFoundException and Message 'Catalog not found.'.
2. Control that must stay correct: GetDatabase/GetTables with a CatalogId that DOES satisfy the grammar but names no catalog must keep answering 400 EntityNotFoundException / 'Catalog not found.' (see glue-tsd-foreign-and-malformed-catalog-id#task#7, owned by 20261105T074444Z-glue-catalog-read-and-create-refusals-restate-the.md).
3. Re-replay the step above to confirm the cause is closed.
