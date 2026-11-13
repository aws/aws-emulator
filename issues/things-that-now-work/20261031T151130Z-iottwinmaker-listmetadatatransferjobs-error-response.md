Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 81ad204619d3c71dcf18bfdd43cea81955af6d23

# Enum-member refusals are built from unbounded caller text without the clamp the crate states every refusal uses, so ValidationException.message leaves its modelled 2048-character bound

_Filed: 2026-10-31T15:11:30Z_
_Operation: ListMetadataTransferJobs | Class: error-response_

> _Note: this issue was identified against the code base at commit `81ad204619d3c71dcf18bfdd43cea81955af6d23`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListMetadataTransferJobs` refuses a `sourceType`, a `destinationType` or a filter `state` outside its modelled value list by interpolating the CALLER'S value straight into the refusal message, and its `invalid` closure builds the `ValidationException` with `message: Some(message)` — no `crate::state::clamped_message`.

The three shapes those members target (`SourceType`, `DestinationType`, `MetadataTransferJobState`) carry a `values` list but NO `@length`, and the handler itself states two lines above the check that "Both are modelled enumerations, whose value lists the boundary validator does not enforce". So a request naming a 10,000-character `sourceType` reaches the `format!` and produces a `ValidationException.message` roughly 10,000 characters long — far outside the `@length(min: 0, max: 2048)` the `ErrorMessage` shape the member targets declares.

That is not a rule the crate is unaware of. `DeleteComponentType` routes its refusal through `crate::state::clamped_message(message, crate::state::ERROR_MESSAGE_MAX_CHARS)` with the comment that it is stated within the bound "as every other refusal of this service is", and the crate's own test suite states the rule as covering the whole service. `ListComponents`, `ListProperties`, `CreateEntity`, `UpdateEntity`, `BatchPutPropertyValues`, `GetPropertyValue` and `GetPropertyValueHistory` all clamp; this handler does not.

The same omission sits in the sibling handlers that refuse the same kind of length-unbounded, value-list-only member, so one fix should cover them: `list_sync_resources.rs` (`filters names {member} {wanted}` for `SyncResourceState` / `SyncResourceType`), `create_metadata_transfer_job.rs` (`sources[{index}] has type {other}` / `destination has type {other}`) and `create_sync_job.rs` (`syncSource {} is not supported`, where `SyncSource` is pattern-only — `^[a-zA-Z_0-9]+$` with no length).

## Evidence

THE MODEL BOUND — `smithy_query(query="shape", shape="ValidationException")`:
```
"ValidationException": {... "httpError": 400, "members": {"message": {"target": "ErrorMessage", "length": {"min": 0, "max": 2048}}}}
```

THE MEMBERS ARE LENGTH-UNBOUNDED — `smithy_query(query="shape", shape="SourceType,DestinationType,MetadataTransferJobState")`:
```
"SourceType": {"type": "string", "values": ["s3", "iotsitewise", "iottwinmaker"]}
"DestinationType": {"type": "string", "values": ["s3", "iotsitewise", "iottwinmaker"]}
"MetadataTransferJobState": {"type": "string", "values": ["VALIDATING", "PENDING", "RUNNING", "CANCELLING", "ERROR", "COMPLETED", "CANCELLED"]}
```
No `length` on any of them.

THE HANDLER ADMITS THE VALUE REACHES IT UNVALIDATED — services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/list_metadata_transfer_jobs.rs:226-247:
```
        // The ends the listing is taken over. Both are modelled enumerations,
        // whose value lists the boundary validator does not enforce (the same
        // reason `UpdatePricingPlan` checks its `pricingMode` here), so a value
        // outside them is refused rather than quietly selecting no job. The
        // message is built from the request: it names the value and the values
        // the model admits.
        if !SOURCE_TYPE_VALUES.contains(&input.source_type.as_str()) {
            return Err(invalid(format!(
                "sourceType is {}, which is not one of {}",
                input.source_type,
                SOURCE_TYPE_VALUES.join(", ")
            ))
            .into());
        }
```

THE UNCLAMPED BUILDER — src/ops/list_metadata_transfer_jobs.rs:214-224:
```
        let invalid = |message: String| {
            ::tracing::info!(
                operation = "ListMetadataTransferJobs",
                outcome = "ValidationException",
                reason = ?message,
                "refusing a request"
            );
            ListMetadataTransferJobsErrorModeled::ValidationException(ValidationException {
                message: Some(message),
            })
        };
```

THE SIBLING THAT DOES CLAMP, AND STATES IT IS THE CRATE-WIDE RULE — src/ops/delete_component_type.rs:236-253:
```
        // What is built on the component type is deleted before the component
        // type itself, so nothing stored is left instantiating or extending a
        // type the store no longer holds. The refusal names as many ids as
        // reference it, so it is stated within the bound the member it travels
        // on declares ([`crate::state::clamped_message`]), as every other
        // refusal of this service is.
        let references = workspace.component_type_references(&input.component_type_id);
        if let Some(message) = references.in_use_message(&input.component_type_id) {
            return Err(DeleteComponentTypeErrorModeled::ValidationException(
                ValidationException {
                    message: Some(crate::state::clamped_message(
                        message,
                        crate::state::ERROR_MESSAGE_MAX_CHARS,
                    )),
                },
            )
            .into());
        }
```
and src/ops/list_components.rs:216-223:
```
        let invalid = |message: String| {
            ListComponentsErrorModeled::ValidationException(ValidationException {
                message: Some(crate::state::clamped_message(
                    message,
                    crate::state::ERROR_MESSAGE_MAX_CHARS,
                )),
            })
        };
```

THE CRATE'S OWN STATEMENT OF THE RULE AS SERVICE-WIDE — tests/all/error_message_bounds.rs:6-18:
```
//! RULE under test: a reason this service states on a modelled member is
//! rendered within that member's declared `@length` maximum. ...
//! ... and every modelled
//! refusal of this service states its reason on the `ErrorMessage` shape,
//! bounded at 2048 ...
```
and the bound restated at tests/all/error_message_bounds.rs:48: `pub(crate) const ERROR_MESSAGE_MAX: usize = 2048;`

TEST COVERAGE: tests/all/error_message_bounds.rs only drives the rule through a `propertyName` and a `componentPath` (the five `componentPath` readers, `a_reader_refusal_message_fits_its_modelled_member` at line 470, and `a_batch_entry_error_message_fits_its_modelled_member` at line 312). No test anywhere in tests/all/ sends an over-long enum-shaped member to `ListMetadataTransferJobs`, `ListSyncResources`, `CreateMetadataTransferJob` or `CreateSyncJob`, which is why the omission is unnoticed.

THE SAME OMISSION IN THE SIBLINGS a fix should cover — src/ops/list_sync_resources.rs:212-222 (`invalid` with `message: Some(message)`) feeding src/ops/list_sync_resources.rs:276-282:
```
            if !admitted.contains(&wanted.as_str()) {
                return Err(invalid(format!(
                    "filters names {member} {wanted}, which is not one of {}",
                    admitted.join(", ")
                ))
                .into());
            }
```
src/ops/create_metadata_transfer_job.rs:376-381:
```
        other => {
            return Some(format!(
                "sources[{index}] has type {other}, which is not one of {}",
                SOURCE_TYPE_VALUES.join(", ")
            ));
        }
```
src/ops/create_sync_job.rs:255-266:
```
        if input.sync_source != crate::state::SYNC_SOURCE_SITEWISE {
            return Err(CreateSyncJobErrorModeled::ValidationException(
                ValidationException {
                    message: Some(format!(
                        "syncSource {} is not supported; the only supported syncSource is {}",
                        input.sync_source,
                        crate::state::SYNC_SOURCE_SITEWISE
                    )),
                },
            )
            .into());
        }
```

## Steps to Reproduce

1. POST /metadata-transfer-jobs-list with body {"sourceType": "<a 5000-character string of 'x'>", "destinationType": "s3"} (the member targets `SourceType`, which carries no `@length`, so the boundary validator admits it).
2. Observe HTTP 400 ValidationException whose `message` is `"sourceType is xxxx…(5000 chars)…, which is not one of s3, iotsitewise, iottwinmaker"` — well over the 2048-character maximum `ErrorMessage` declares for the member it travels on.
3. Contrast ListComponents with an over-long componentPath: src/ops/list_components.rs:216-223 routes the same kind of reason through `crate::state::clamped_message(message, crate::state::ERROR_MESSAGE_MAX_CHARS)`, so its message stops at 2048.
4. Repeat with {"sourceType": "s3", "destinationType": "s3", "filters": [{"state": "<5000 x's>"}]} for the filter-state refusal at src/ops/list_metadata_transfer_jobs.rs:264-274.
5. Repeat against ListSyncResources (filters.state / filters.resourceType), CreateMetadataTransferJob (sources[0].type) and CreateSyncJob (a 5000-character alphanumeric syncSource in the path) for the sibling occurrences.
