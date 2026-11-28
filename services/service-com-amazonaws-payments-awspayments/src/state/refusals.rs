// SPDX-License-Identifier: CC0-1.0

//! The wording this service states its failures in.
//!
//! One place for the texts more than one handler states, so six operations cannot
//! word the same failure six ways. Each constant names the modelled member it fills
//! and quotes the model for its text; a text only one operation states stays in that
//! operation's own file.

/// `InternalServerException$message` — what the service says when it cannot reach
/// its own store.
///
/// The model documents `InternalServerException` as "the request processing failed
/// because of an unknown error, exception, or failure. Retry your request", which is
/// exactly this case: the engine the operation was built from does not hold this
/// service's state, so there is nothing to report and retrying is all the caller can
/// do.
pub const STORE_UNREACHABLE: &str =
    "The request processing failed because of an unknown error, exception, or failure. Retry your request.";

/// `ResourceNotFoundException$message` — what the service says when nothing on the
/// account matches the identifier the request named.
///
/// The model documents the error as "the requested resource couldn't be found.
/// Verify that the resource ID is correct"; the resource the caller asked for is
/// named in the modelled `resourceId` and `resourceType` members beside it, which is
/// what those members are for.
pub const RESOURCE_NOT_FOUND: &str =
    "The requested resource couldn't be found. Verify that the resource ID is correct.";

/// `ResourceNotFoundException$resourceType` for a payment instrument — the resource
/// type the service's own ARNs name it by
/// (`arn:…:payments::…:payment-instrument:…`).
pub const PAYMENT_INSTRUMENT_RESOURCE_TYPE: &str = "payment-instrument";

/// `ResourceNotFoundException$resourceType` for an invoice or a credit memo.
pub const FINANCIAL_RECORD_RESOURCE_TYPE: &str = "financial-record";

/// `ValidationExceptionReason$FIELD_VALIDATION_FAILED` — "one or more members failed
/// validation. `fieldList` names them."
pub const REASON_FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// `ValidationException$message` for a member carrying a value this service cannot
/// read as the enum the model types it as.
///
/// `declared` is that enum's members, in model order. One wording for every such
/// member of every operation, so two handlers cannot word the same refusal
/// differently.
pub fn unreadable_enum_member<T: ::std::fmt::Display>(member: &str, declared: &[T]) -> String {
    let values: Vec<String> = declared
        .iter()
        .map(::std::string::ToString::to_string)
        .collect();
    format!("{member} accepts only the values {}.", values.join(", "))
}

/// `ValidationException$message` when a request names both ways of identifying a
/// financial record, or neither.
///
/// Both `GetFinancialRecord` and the `FinancialRecordKey` union state the same rule:
/// "specify exactly one member: the ID that Amazon Web Services Payments issued, in
/// `identifier`, or the invoice ID that Amazon Web Services Invoicing issued, in
/// `invoiceIdentifier`. If you specify both or neither, the request fails with
/// `ValidationException`."
pub const EXACTLY_ONE_IDENTIFIER: &str =
    "Specify exactly one of identifier or invoiceIdentifier.";

/// `GetFinancialRecordRequest$identifier` / `FinancialRecordKey$identifier`, as a
/// `ValidationExceptionField$name` names it.
pub const IDENTIFIER_MEMBER: &str = "identifier";

/// `GetFinancialRecordRequest$invoiceIdentifier` /
/// `FinancialRecordKey$invoiceIdentifier`, as a `ValidationExceptionField$name`
/// names it.
pub const INVOICE_IDENTIFIER_MEMBER: &str = "invoiceIdentifier";
