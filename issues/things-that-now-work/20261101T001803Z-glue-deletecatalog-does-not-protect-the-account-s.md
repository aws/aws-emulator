Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.root-catalog-delete-rejected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.root-catalog-delete-rejected|run#6|AssertionError|assert code == "InvalidInputException", f"unexpected code: {code}"
Issue-Commit: e3579db6a1835cfcd1e7033eb2fb9f64dc458cca

# Glue DeleteCatalog does not protect the account's root Data Catalog: it answers 501 NotImplemented instead of refusing 400 InvalidInputException

> _Note: this issue was identified against the code base at commit `e3579db6a1835cfcd1e7033eb2fb9f64dc458cca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCatalog

## Description
Cause 1 of 1: `DeleteCatalog` has no root-catalog protection rule, so deleting the account's own root Glue Data Catalog (CatalogId == the caller's account id) is not refused the way real Glue refuses it.

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json) models `DeleteCatalog` with required input member `CatalogId` (CatalogIdString, @length 1..255, @pattern ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$) and the modeled errors AccessDeniedException, ConcurrentModificationException, EntityNotFoundException, FederationSourceException, GlueEncryptionException, InternalServiceException, InvalidInputException (all HTTP 400) and InternalServiceException (HTTP 500). The model does not state the root-catalog rule, so for that rule this is RECORDED BEHAVIOR ONLY: the recording shows that a `DeleteCatalog` whose `CatalogId` is the caller's account id — i.e. the account's root catalog, the catalog `GetCatalog(CatalogId=<account-id>)` returns — is refused HTTP 400 with error code `InvalidInputException` and the message `Only lowercase letters, numbers, underscores and hyphens are allowed for catalog name`. That message is entirely fixed text for this error class; it interpolates nothing from the request or from state. The refusal leaves the catalog intact: a following `GetCatalog` for the same account id still succeeds and still reports that `CatalogId`/`Name`. The root catalog is therefore a protected singleton — it is never deletable, and the refusal is an input-validation refusal (InvalidInputException), not EntityNotFoundException and not AccessDeniedException.

Emulator: `DeleteCatalog` answers HTTP 501 with error code `NotImplemented` and the message `operation DeleteCatalog is scaffolded but not implemented in this emulator` for this request, because the handler is still the unmodified scaffold stub — `services/service-com-amazonaws-glue-awsglue/src/ops/delete_catalog.rs` line 170 returns `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteCatalog"))` and the file still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (line 4). The replayed check asserted the refusal code and got `NotImplemented` where `InvalidInputException` is required (`AssertionError: unexpected code: NotImplemented`). `GetCatalog(CatalogId=<account-id>)` itself does answer, so the divergence is confined to the delete path.

NOTE on an adjacent open issue: `20261029T185330Z-glue-deletecatalog-stub.md` ("Implement glue DeleteCatalog: unimplemented scaffold stub") asks for the operation to be implemented, but its Issue-Trace-Ids are only `glue.catalog-delete-nonexistent-rejected`, `glue.catalog-lifecycle-create-get-delete`, `glue.catalog-lifecycle-create-get-delete-404`, `glue.concurrent-delete-catalog-conflict-rejected`, `glue.delete-catalog-nonexistent-not-found-exact-code` — it does NOT carry `glue.root-catalog-delete-rejected`, and it says nothing about the root-catalog protection rule or its InvalidInputException message. A DeleteCatalog implementation that only satisfies that issue's five units can still delete (or 404) the account's root catalog and still leave this divergence in place, which is why this cause is filed separately.

Why: a client that relies on AWS's behavior treats the account's root catalog as undeletable and distinguishes that refusal from "catalog does not exist" by the `InvalidInputException` code and its message. Against the emulator it instead sees 501 `NotImplemented`, which botocore surfaces as a transport/unsupported-API failure rather than a modeled client error: guard-rail code that asserts the root catalog cannot be dropped never runs, retry logic can treat 501 as non-retryable server breakage, and any teardown path that calls DeleteCatalog to reclaim catalogs cannot tell a protected catalog from a missing one.

Fix: implement the `DeleteCatalog` handler in `services/service-com-amazonaws-glue-awsglue/src/ops/delete_catalog.rs` (validation/refusal branch plus the catalog store it reads), and include the root-catalog guard: when the requested `CatalogId` resolves to the account's own root catalog, return the modeled `InvalidInputException` with the fixed message above and persist no change, so a subsequent `GetCatalog` for that account id still succeeds. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line only once the handler is real.

Traces: glue.root-catalog-delete-rejected.

## Evidence
Failing check: glue.root-catalog-delete-rejected [mismatch].

Expected (AWS, from the recording):
  glue.delete_catalog(CatalogId=<account-id>) -> HTTP 400
    Error.Code    = "InvalidInputException"
    Error.Message = "Only lowercase letters, numbers, underscores and hyphens are allowed for catalog name"   (fixed text; no request- or state-derived parts)
  and afterwards glue.get_catalog(CatalogId=<account-id>) still returns a Catalog whose CatalogId (or Name) is that account id.

Actual (emulator):
  glue.delete_catalog(CatalogId=<account-id>) -> HTTP 501
    Error.Code    = "NotImplemented"
    Error.Message = "operation DeleteCatalog is scaffolded but not implemented in this emulator"

Replay output:
  File "check_glue_root_catalog_delete_rejected.py", line 41, in run
    assert code == "InvalidInputException", f"unexpected code: {code}"
  AssertionError: unexpected code: NotImplemented

Code citation: services/service-com-amazonaws-glue-awsglue/src/ops/delete_catalog.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED:` marker) and line 170 (`Err(crate::scaffolded::Scaffolded::not_implemented("DeleteCatalog"))`).

Model citation: Smithy DeleteCatalog — input CatalogId (CatalogIdString, @length 1..255) required; modeled errors include InvalidInputException (HTTP 400, member Message) and EntityNotFoundException (HTTP 400). The root-catalog protection rule itself is recorded behavior only; the model does not state it.

## Steps to Reproduce
1. Boot the emulator (glue + sts).
2. account_id = sts.get_caller_identity()["Account"].
3. glue.get_catalog(CatalogId=account_id) — succeeds and reports that account id as CatalogId/Name (the account's root Data Catalog).
4. glue.delete_catalog(CatalogId=account_id).
   Expected: HTTP 400, Error.Code "InvalidInputException", Error.Message "Only lowercase letters, numbers, underscores and hyphens are allowed for catalog name".
   Observed: HTTP 501, Error.Code "NotImplemented", Error.Message "operation DeleteCatalog is scaffolded but not implemented in this emulator".
5. glue.get_catalog(CatalogId=account_id) again — must still succeed (the refusal must have no effect).
6. Replay: glue.root-catalog-delete-rejected.
