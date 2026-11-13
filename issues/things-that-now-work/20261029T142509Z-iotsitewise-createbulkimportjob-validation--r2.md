Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4fd05cf1a47f5cbbe05dba0186301153f554fae3

# jobName is documented as "The unique name that helps identify the job request" but a second job may take a name already in use, and the modelled ResourceAlreadyExistsException is never reached

_Filed: 2026-10-29T14:25:09Z_
_Operation: CreateBulkImportJob | Class: validation_

> _Note: this issue was identified against the code base at commit `4fd05cf1a47f5cbbe05dba0186301153f554fae3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateBulkImportJob stores `jobName` unchecked. The member's documentation states it is unique, and the operation declares `ResourceAlreadyExistsException` (409) — the error this service uses when the resource a create would mint is already there — yet the handler never constructs that error and never looks at the names of the jobs already stored. Two creates with the same jobName both succeed, so the account ends up holding several bulk import jobs the caller can only tell apart by the minted jobId, and the modelled 409 is unreachable on the one dimension the create claims is unique.

This is not a rule the crate declines to implement: it implements exactly this rule, from exactly this wording, for gateways — CreateGateway refuses a second gateway taking a name in use, quoting "`gatewayName` is \"A unique name for the gateway\"", and UpdateGateway applies the same check to a rename. The crate also states the inverse case deliberately where the doc does NOT say unique (a portal's name is "A friendly name for the portal", and two portals may share one). `jobName` falls on the gateway side of that distinction and is treated as if it fell on the portal side.

## Evidence

The model sentence, smithy_query(query="doc", shape="CreateBulkImportJobRequest", member="jobName"): "The unique name that helps identify the job request." — restated in the handler's own contract header, services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/create_bulk_import_job.rs:76:
```
//!   **required** — The unique name that helps identify the job request.
```
and the operation declares the collision error (smithy_query(query="operation", operation="CreateBulkImportJob")): {"shape": "ResourceAlreadyExistsException", "errorKind": "client", "httpError": 409}, which the handler's own error enum carries unused — src/ops/create_bulk_import_job.rs:142: `    ResourceAlreadyExistsException(ResourceAlreadyExistsException),` while the only refusals it builds are src/ops/create_bulk_import_job.rs:350 `fn invalid_request`, :365 `fn limit_exceeded`, :373 `fn not_found`.

What the handler does with the name — it is written straight onto the record, src/ops/create_bulk_import_job.rs:317-319:
```
        let record = crate::state::BulkImportJobRecord {
            job_id,
            job_name: input.job_name.clone(),
```
(a grep for `job_name` in that file finds only the two header lines, this write and the echo at :335 — no uniqueness test).

The sibling of this crate that implements the identical documented rule — src/ops/create_gateway.rs:267-275:
```
        // `gatewayName` is "A unique name for the gateway", so a second gateway may
        // not take a name already in use.
        if let Some(existing) = store
            .gateways
            .values()
            .find(|record| record.name == input.gateway_name)
        {
            return Err(already_exists(existing));
        }
```
and src/ops/update_gateway.rs:190-192: "// `gatewayName` is \"A unique name for the gateway\", so a rename may not take a name another gateway already holds — the same one-name-one-gateway rule `CreateGateway` enforces".

Test coverage: nothing pins duplicate job names as allowed. tests/all/bulk_import_jobs.rs drives each row with a distinct `job_name` ("Historical import", "csv_import:2026/01", "Mixed formats", "Session import", "Bulk of a hundred files" — lines 206, 215, 237, 272, 287) and asserts only that "a create answers the name it was given" (tests/all/bulk_import_jobs.rs:357-358); no case creates two jobs under one name.

## Steps to Reproduce

1. PutStorageConfiguration with warmTier=ENABLED (so a job with no dataset destination is admitted — src/ops/create_bulk_import_job.rs:268-288).
2. POST /jobs with {"jobName": "Historical import", jobRoleArn, files, errorReportLocation} -> 202 with jobId J1.
3. POST /jobs again with the SAME {"jobName": "Historical import"} and the same body.
4. Observed: 202 and a second job J2; GET /jobs (ListBulkImportJobs) lists two jobs whose jobName is "Historical import".
5. Expected, per the member's documentation ("The unique name that helps identify the job request.") and the identical rule this crate enforces for gatewayName (src/ops/create_gateway.rs:267-275): the second create is refused with the modelled ResourceAlreadyExistsException (409) naming the job that already holds the name, and nothing is stored.
