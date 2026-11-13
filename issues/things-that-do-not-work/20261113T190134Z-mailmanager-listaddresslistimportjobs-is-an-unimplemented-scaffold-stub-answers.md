Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mailmanager.address-list-deletion-orphans-its-import-job
Issue-Shape-Id: com.amazonaws.mailmanager#MailManagerSvc
Issue-Key: mailmanager.address-list-deletion-orphans-its-import-job|run#13|AssertionError|assert code == "ResourceNotFoundException", f"ListAddressListImportJobs gave {code!r}"
Issue-Commit: 2500a4daac20cde0616d8d78af54e2cda83d8b28

# mailmanager ListAddressListImportJobs is an unimplemented scaffold stub: answers 501 NotImplemented instead of listing an address list's import jobs (or 404 ResourceNotFoundException for a deleted list)

> _Note: this issue was identified against the code base at commit `2500a4daac20cde0616d8d78af54e2cda83d8b28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAddressListImportJobs

## Description
One net-new root cause.

Cause 1 of 1: `ListAddressListImportJobs` has no implementation in the mailmanager emulator; the scaffold handler returns the generic not-implemented error for every request, so neither the success path nor the not-found path of the operation exists. The failing check reaches it on the not-found path (list the import jobs of an address list that has just been deleted), but the 501 is unconditional, so every call to this operation diverges. Failing check id: `mailmanager.address-list-deletion-orphans-its-import-job`.

AWS: Smithy (`com.amazonaws.mailmanager#ListAddressListImportJobs`) — input `AddressListId` (required, @length 1..255, pattern `^[a-zA-Z0-9-]+$`), `NextToken` (@length 1..2048), `PageSize` (@range 1..50); output `ImportJobs` (required list) + optional `NextToken`; @readonly and @paginated (inputToken=NextToken, outputToken=NextToken, items=ImportJobs, pageSize=PageSize); success status 200. Modeled errors: `AccessDeniedException` (403), `ResourceNotFoundException` (404), `ThrottlingException` (429), `ValidationException` (400). Model doc: "Lists jobs for an address list." / `ResourceNotFoundException` — "Occurs when a requested resource is not found."
For the deleted-address-list path the model states only the 404 `ResourceNotFoundException` classification; the recorded behavior shows the error names the ADDRESS LIST, not the job, with message `Could not find AddressList with id <AddressListId>` — fixed text `"Could not find AddressList with id "` followed by the `AddressListId` value taken verbatim from the request. (The same recording confirms the sibling `GetAddressListImportJob` already serves exactly this code and message for the orphaned job, so the required wording is established by the service's own behavior.)

Emulator: every `ListAddressListImportJobs` call is refused. The replayed check observes `Error.Code == 'NotImplemented'` where AWS answers `ResourceNotFoundException`; the handler returns `crate::scaffolded::Scaffolded::not_implemented("ListAddressListImportJobs")`, i.e. HTTP 501 with code `NotImplemented`, an error the operation does not model at all. The success path (200 with `ImportJobs`) is equally unreachable.

Why: a client cannot enumerate an address list's import jobs through the emulator at all — paginators over `ImportJobs` fail, and code that distinguishes "address list gone" (404 `ResourceNotFoundException`, a retry/cleanup signal) from a transport-level 501 takes the wrong branch, because botocore surfaces `NotImplemented`/501 as an un-modeled server fault rather than the modeled client error. Any import-job lifecycle flow built on CreateAddressListImportJob + List/Get is blocked.

Fix: implement the operation's handler and its state read — list the import jobs the service state holds for the requested `AddressListId`, apply `PageSize`/`NextToken` paging per the @paginated trait, and serialize 200 with `ImportJobs` (empty list when the address list exists with no jobs, since `ImportJobs` is required). When the `AddressListId` is not present in state (never created, or deleted by `DeleteAddressList`), return the modeled `ResourceNotFoundException` (404) with the message above, matching the existing `GetAddressListImportJob` not-found wording. Remove the SCAFFOLDED marker line once implemented.

Traces: failing check `mailmanager.address-list-deletion-orphans-its-import-job` (assertion `ListAddressListImportJobs gave 'NotImplemented'` at check line 968). Handler: `services/service-com-amazonaws-mailmanager-mailmanagersvc/src/ops/list_address_list_import_jobs.rs` lines 3-6 (SCAFFOLDED marker) and line 167 (the unconditional `not_implemented` return).

No open issue covers this operation; the nearest open mailmanager issues (`20261105T141841Z-mailmanager-getmemberofaddresslist-not-found-message-omits-the-of.md`, `20261030T011318Z-mailmanager-createarchive-mints-an-archiveid-that-violates-the-modeled.md`) concern different operations and do not carry this check's replay key.

## Evidence
Expected (AWS): HTTP 404, `Error.Code = "ResourceNotFoundException"`, `Error.Message = "Could not find AddressList with id <AddressListId>"` — fixed text "Could not find AddressList with id ", variable part = the `AddressListId` supplied in the request (here the id of the address list just deleted). On the normal path: HTTP 200 with the required `ImportJobs` member (plus `NextToken` when more pages remain).

Actual (emulator): HTTP 501, `Error.Code = "NotImplemented"`. Replay output:
```
[mismatch] mailmanager.address-list-deletion-orphans-its-import-job
File "check_mailmanager_address_list_deletion_orphans_its_import_job.py", line 52, in run
  assert code == "ResourceNotFoundException", f"ListAddressListImportJobs gave {code!r}"
AssertionError: ListAddressListImportJobs gave 'NotImplemented'
```
The preceding `GetAddressListImportJob` assertions in the same check PASS (correct code and message), isolating the divergence to `ListAddressListImportJobs`.

Source confirmation — `services/service-com-amazonaws-mailmanager-mailmanagersvc/src/ops/list_address_list_import_jobs.rs:167`:
```rust
Err(crate::scaffolded::Scaffolded::not_implemented("ListAddressListImportJobs"))
```
with the file header still carrying `//! SCAFFOLDED, NOT IMPLEMENTED:`. `NotImplemented` is not among the operation's modeled errors (AccessDenied/ResourceNotFound/Throttling/Validation).

## Steps to Reproduce
1. `mm = boto3.client("mailmanager", endpoint_url=<emulator>)`
2. `al_id = mm.create_address_list(AddressListName="sim2real-behav-mm-al-<unique>")["AddressListId"]`
3. `job_id = mm.create_address_list_import_job(AddressListId=al_id, Name="sim2real-behav-mm-job-<unique>", ImportDataFormat={"ImportDataType": "CSV"})["JobId"]`
4. `mm.get_address_list_import_job(JobId=job_id)` -> `Status == "CREATED"` (positive control, passes)
5. `mm.delete_address_list(AddressListId=al_id)`
6. `mm.get_address_list_import_job(JobId=job_id)` -> raises `ResourceNotFoundException` with `Could not find AddressList with id <al_id>` (passes today)
7. `mm.list_address_list_import_jobs(AddressListId=al_id)` -> EXPECT `ResourceNotFoundException` (HTTP 404) with the same message; OBSERVE `NotImplemented` (HTTP 501).
8. Also: `mm.list_address_list_import_jobs(AddressListId=<a live address list id>)` -> EXPECT HTTP 200 with `ImportJobs`; OBSERVE `NotImplemented` (HTTP 501).

Replay key: behavioral check `mailmanager.address-list-deletion-orphans-its-import-job`.
