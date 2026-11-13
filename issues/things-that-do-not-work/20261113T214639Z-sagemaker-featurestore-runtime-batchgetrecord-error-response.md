Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 6111c3fa081e7f8e952c1f0c4a75668e426695a2

# BatchGetRecord's per-entry ErrorMessage is emitted unbounded, though its modelled Message maximum is 2048 and BatchWriteRecord cuts the same member to that bound

_Filed: 2026-11-13T21:46:39Z_
_Operation: BatchGetRecord | Class: error-response_

> _Note: this issue was identified against the code base at commit `6111c3fa081e7f8e952c1f0c4a75668e426695a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When an entry's `FeatureGroupName` resolves to no feature group, BatchGetRecord reports one `BatchGetRecordError` per identifier with `error_message: message.clone()` — the message verbatim, with no length bound. The message is `crate::state::no_such_feature_group(arn)`, which embeds the fully qualified feature-group ARN built by `feature_group_arn(region, name)`; that `region` is taken from the request's own SigV4 credential scope (`request_region`), which the parser bounds in no way at all, so the member this handler fills can exceed the maximum its own target shape declares.

`BatchGetRecordError.ErrorMessage` targets `Message`, which the model bounds at `length 0..2048`. The crate already states and solves this rule one module over: BatchWriteRecord routes every per-entry message through `bounded_message`, cutting it at the last character boundary within `MAX_ERROR_MESSAGE = 2048`, for the identical `ErrorMessage`/`Message` bound — and the message it bounds is produced by the same `crate::state` helpers. BatchGetRecord does not call it, so one batch API answers inside the modelled bound and its sibling outside it.

## Evidence

The modelled bound on the member the handler fills — `smithy_query(query="shape", shape="BatchGetRecordError")`:
`"ErrorMessage": {"target": "Message", "clientOptional": true, "required": true, "length": {"min": 0, "max": 2048}, "doc": "<p>The error message of an error that has occurred when attempting to retrieve a record in the batch.</p>"}`

The unbounded fill — services/service-com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime/src/ops/batch_get_record.rs:219-229:
```
            if let Some(message) = refusal {
                errors.extend(wanted.record_identifiers_value_as_string.iter().map(
                    |identifier| BatchGetRecordError2 {
                        feature_group_name: group_name.clone(),
                        record_identifier_value_as_string: identifier.clone(),
                        error_code: crate::state::VALIDATION_ERROR_CODE.to_owned(),
                        error_message: message.clone(),
                    },
                ));
                continue;
            }
```

What that message carries — src/state.rs:783-785:
```
pub fn no_such_feature_group(arn: &str) -> String {
    format!("Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name [{arn}].")
}
```
built from the request's own region — src/state.rs:542-548 (`feature_group_arn` -> `::emulator_server::emulator_arn(ARN_SERVICE, region, …)`, which interpolates `region` verbatim: vendor/server/src/lib.rs:170-172 `format!("arn:aws:{service}:{region}:{EMULATOR_ACCOUNT_ID}:{resource}")`) and src/state.rs:527-532:
```
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_owned())
}
```
with the scope parser applying no length bound to that field — vendor/server/src/lib.rs:1962-1975 (it only rejects an EMPTY region).

The other side of the inconsistency, in this crate — src/ops/batch_write_record.rs:373-404:
```
fn entry_error(
    entry: &BatchWriteRecordEntry,
    error_code: &str,
    message: String,
) -> BatchWriteRecordError2 {
    BatchWriteRecordError2 {
        entry: entry.clone(),
        error_code: error_code.to_owned(),
        error_message: bounded_message(message),
    }
}

/// `message`, cut to the modelled maximum length of an `ErrorMessage`.
...
/// The modelled maximum length of a `BatchWriteRecordError`'s `ErrorMessage`: its target
/// `Message` is `len 0..2048`
const MAX_ERROR_MESSAGE: usize = 2048;
```
and that bound is applied to the very same `no_such_feature_group` text at src/ops/batch_write_record.rs:260-266.

Test coverage: no test asserts any BatchGetRecord `ErrorMessage` length — tests/all/online_store_record_lifecycle.rs:1015-1045 reads only `FeatureGroupName`, `RecordIdentifierValueAsString` and `ErrorCode` out of `Errors`, so bounding the message breaks nothing.

## Steps to Reproduce

1. Boot the emulator with this service mounted.
2. POST /BatchGetRecord with body {"Identifiers":[{"FeatureGroupName":"nosuchgroup","RecordIdentifiersValueAsString":["X1"]}]} and an Authorization header whose credential scope names a long region, e.g. `AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/<4000 'a' characters>/sagemaker/aws4_request, SignedHeaders=host, Signature=x` — the scope parser accepts any non-empty region (vendor/server/src/lib.rs:1962-1975).
3. Observe the 200 body: `Errors[0].ErrorMessage` is the whole `Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name [arn:aws:sagemaker:<4000 chars>:000000000000:feature-group/nosuchgroup].` string (batch_get_record.rs:225 via state.rs:783-785), i.e. roughly 4100 characters against the modelled `Message` maximum of 2048.
4. POST /BatchWriteRecord with Entries=[{"FeatureGroupName":"nosuchgroup","Record":[{"FeatureName":"customer_id","ValueAsString":"C1"}]}] and the same Authorization header — the per-entry `ErrorMessage` for the identical refusal is cut to 2048 characters (batch_write_record.rs:381, 390-399), so the two batch APIs answer the same refusal in and out of the modelled bound.
