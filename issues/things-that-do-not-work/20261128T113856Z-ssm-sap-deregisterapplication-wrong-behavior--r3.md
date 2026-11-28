Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 87114bf6684e0f878cdb2d8b5aad68cc7796cae8

# The DEREGISTER_APPLICATION operation record is unreachable through the API, contradicting the comment that justifies filing it

_Filed: 2026-11-28T11:38:56Z_
_Operation: DeregisterApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `87114bf6684e0f878cdb2d8b5aad68cc7796cae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeregisterApplication` files a `StoredOperation` record of type `DEREGISTER_APPLICATION` and then removes the application in the SAME call. The handler's own comment justifies filing that record by naming its two readers: "the record is what `ListOperations` reports and what `GetOperation` resolves".

Neither reader can ever reach it:

* `ListOperations` refuses outright when the `ApplicationId` names no application registered in the Region, and the deregistration has just removed that application — so the listing answers `ValidationException`, not a page containing the record. Even if it did not refuse, `RegionState::operations_for` returns an EMPTY vector for an id the partition holds no application under, so the record could not be selected.
* `GetOperation` resolves by `OperationId`, but `DeregisterApplicationOutput` models no members at all, so no response ever hands that id to a caller.

The result is store state no API caller can observe: the record is written, counts against `MAX_RETAINED_OPERATIONS`, and keeps the Region's partition from being reclaimed (`RegionState::is_empty` requires `operations.is_empty()`), while the comment that explains why it is written states a reachability the code cannot provide. Every sibling control call (`RegisterApplication`, `StartApplication`, `StopApplication`, `StartApplicationRefresh`, `UpdateApplicationSettings`) hands its `OperationId` back in its output, so the record it files really is the one `GetOperation` resolves; this one is the only record of the slice that is filed and then orphaned.

## Evidence

The record is filed and the application removed in the same call — `services/service-com-amazonaws-ssmsap-ssmsap/src/ops/deregister_application.rs:172-193`:
```
        // Every call that changes an application files an operation record for the work
        // it started, so the deregistration is recorded like the registration, start and
        // stop before it. `DeregisterApplicationOutput` carries no `OperationId`, so the
        // id is not handed back to the caller — the record is what `ListOperations`
        // reports and what `GetOperation` resolves. Filed BEFORE the removal, so it
        // records the registration it was filed against
        // ([`crate::state::StoredOperation::registration`]).
        region_state.record_operation(
            crate::state::OPERATION_TYPE_DEREGISTER_APPLICATION,
            &application,
            ...
            now,
        );

        // The deregistration takes effect NOW rather than at a later instant the clock
        // owes: ...
        region_state.deregister_application(&input.application_id);
```

`ListOperations` cannot report it — `src/ops/list_operations.rs:187-192`:
```
        if !region_state.applications.contains_key(&input.application_id) {
            return Err(ListOperationsErrorModeled::ValidationException(
                crate::state::application_does_not_exist(&input.application_id),
            )
            .into());
        }
```
and the selector it would use returns nothing anyway — `src/state.rs:2667-2670`:
```
    pub fn operations_for(&self, application_id: &str) -> Vec<&StoredOperation> {
        let Some(application) = self.applications.get(application_id) else {
            return Vec::new();
        };
```

`GetOperation` can never be given the id: the model gives the output no members — `services/service-com-amazonaws-ssmsap-ssmsap/OPERATIONS.md:33-38`:
```
## DeregisterApplication

- **Route:** `POST /deregister-application -> 200`
- **Input:** `ApplicationId` (string, len 1..60, pattern ^[\w\d\.-]+$, required)
- **Output:** (no members)
```

The orphaned record also pins the partition — `src/state.rs:2166-2172`:
```
    pub fn is_empty(&self) -> bool {
        self.applications.is_empty()
            && self.operations.is_empty()
            && self.configuration_check_operations.is_empty()
```
which `ComAmazonawsSsmsapSsmsapState::settle` (`src/state.rs:2854-2861`) documents as "Settling a Region can empty it (the last deregistration it held completed), and a partition that holds nothing is not one this store has to keep" — a deregistration can never empty it while its own record stays.

Test coverage: `tests/all/deregistration_window.rs:294-313` reaches INTO the store to find the id precisely because no response carries it ("so this is the one record of this slice whose id no response hands out — hence the store read"), then asserts `GetOperation` resolves it at `tests/all/deregistration_window.rs:535-555`; so the only existing check of this record works around the API rather than pinning API reachability. `tests/all/reregistered_application_history.rs:253` asserts `ListOperations` does NOT list a `DEREGISTER_APPLICATION` type.

## Steps to Reproduce

1. POST /register-application {"ApplicationId":"hdb-1","ApplicationType":"HANA","Instances":["i-0123456789abcdef0"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"HDB","CredentialType":"ADMIN","SecretId":"s"}]} and note the returned OperationId.
2. POST /deregister-application {"ApplicationId":"hdb-1"} — answered 200 with an EMPTY body: no OperationId for the DEREGISTER_APPLICATION record the handler just filed.
3. POST /list-operations {"ApplicationId":"hdb-1"} — answered 400 ValidationException, so the record the comment says this listing reports is not reported here (or anywhere).
4. POST /get-operation with the registration's OperationId — still 200, proving records survive the deregistration; there is no id to pass for the deregistration's own record, so it can never be fetched.
5. Observe in the store that `region(REGION).operations` still holds the DEREGISTER_APPLICATION record (as tests/all/deregistration_window.rs:301-313 does), i.e. state no API path can reach.
