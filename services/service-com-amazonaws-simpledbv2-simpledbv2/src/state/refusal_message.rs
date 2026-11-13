// SPDX-License-Identifier: CC0-1.0

//! The sentences this service refuses a request with.
//!
//! Every modelled client error of this service carries a `@required` `message`, and
//! that member is the only part of the response that says WHICH parameter or resource
//! the service faulted — a client reading the refusal, or a fixture asserting on it,
//! has nothing else to go on. So the wording is part of the contract, and these rules
//! decide every sentence here:
//!
//! * a refusal that faults ONE PARAMETER names that parameter as the API documents it
//!   ([`api_parameter`]) and states the requirement its value or its company had to
//!   meet — [`invalid_parameter_value`] for a value the parameter does not admit,
//!   [`parameter_only_provided_when`] for one supplied without the value it is only
//!   expected alongside;
//! * a refusal about a RESOURCE the account does not hold names the kind of resource
//!   ([`no_such_resource`]), whichever identifier the request used to name it;
//! * an error with neither — a forged pagination token, a conflicting idempotency
//!   token, a start past the export quota — carries the sentence the service publishes
//!   for it, which is the sentence its own shape documents (the constants below, each
//!   with the `smithy_query` that reads it).
//!
//! No sentence here interpolates a value the request supplied: the requirement, or the
//! resource, is what the caller has to fix, never the one value that missed it, so
//! nothing a caller sent is reflected back out of this service.
//!
//! The clauses of a refusal the CENTRALIZED walk raises — a value outside a modelled
//! `@length`, `@pattern` or `@range`, an absent `@required` member — are not stated
//! here: those belong to the framework's `ValidationException` envelope, which
//! `crate::service`'s refusal seam words.

/// `member` as the API documents the parameter it names: the modelled member name with
/// its first character upper-cased (`exportArn` -> `ExportArn`, `domainName` ->
/// `DomainName`).
///
/// A member of an AWS API is lowerCamel on the wire and UpperCamel in the API
/// documentation a caller reads, and the refusal names the documented spelling, so the
/// one is derived from the other rather than restated member by member.
#[must_use]
pub fn api_parameter(member: &str) -> String {
    let mut characters = member.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// The sentence this service states for a parameter whose value it refused: the
/// parameter, named as the API documents it, and the requirement its value had to meet.
///
/// `requirement` completes `<Parameter> must …` and carries no final stop, so
/// `invalid_parameter_value("exportArn", "be a valid AWS SimpleDB Domain Export ARN")`
/// reads `Found invalid value for parameter ExportArn. ExportArn must be a valid AWS
/// SimpleDB Domain Export ARN.`
///
/// The offending value is NOT part of the sentence: what the caller has to fix is the
/// requirement, which is the same whatever was submitted.
#[must_use]
pub fn invalid_parameter_value(member: &str, requirement: &str) -> String {
    let parameter = api_parameter(member);
    format!("Found invalid value for parameter {parameter}. {parameter} must {requirement}.")
}

/// The requirement an `exportArn` has to meet: it identifies one domain export, so a
/// value that is not an ARN naming an export of this service names no export at all.
pub const EXPORT_ARN_REQUIREMENT: &str = "be a valid AWS SimpleDB Domain Export ARN";

/// The sentence this service states for a resource the account does not hold: the kind
/// of resource the request named, in the one form every not-found refusal of this
/// service takes.
///
/// `resource` is the resource word — [`DOMAIN_RESOURCE`], [`EXPORT_RESOURCE`] — so
/// `no_such_resource(EXPORT_RESOURCE)` reads `The specified export does not exist.`
/// Which resource was meant is the whole of it: the identifier the request supplied is
/// not echoed, so the sentence is the same for every value that names nothing.
#[must_use]
pub fn no_such_resource(resource: &str) -> String {
    format!("The specified {resource} does not exist.")
}

/// The resource a `NoSuchDomainException` is about, as its sentence names it.
pub const DOMAIN_RESOURCE: &str = "domain";

/// The resource a `NoSuchExportException` is about, as its sentence names it.
pub const EXPORT_RESOURCE: &str = "export";

/// The sentence this service states for a parameter supplied without the companion
/// value it is only expected alongside: the parameter, named as the API documents it,
/// and the condition under which it may be provided at all.
///
/// `condition` completes `… must be provided only when <condition>.`, so
/// `parameter_only_provided_when("s3SseKmsKeyId", "S3-SSE Algorithm is KMS")` reads
/// `Parameter S3SseKmsKeyId must be provided only when S3-SSE Algorithm is KMS.` The
/// values the request supplied are no part of it: what the caller has to fix is the
/// combination.
#[must_use]
pub fn parameter_only_provided_when(member: &str, condition: &str) -> String {
    format!(
        "Parameter {} must be provided only when {condition}.",
        api_parameter(member)
    )
}

/// The `message` every `InvalidNextTokenException` this service raises carries —
/// source: `smithy_query(query="shape",shape="InvalidNextTokenException")` → doc "The
/// specified next token is not valid."
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "The specified next token is not valid.";

/// The `message` every `ConflictException` this service raises carries — source:
/// `smithy_query(query="shape",shape="ConflictException")` → doc "Indicates a conflict
/// with one or more parameters of the request."
///
/// The shape's documentation is the only wording the service publishes for this error,
/// and the `clientToken` the conflict is about came from the caller, so it is stated
/// without it.
pub const CONFLICT_MESSAGE: &str =
    "Indicates a conflict with one or more parameters of the request.";

/// The `message` every `NumberExportsLimitExceeded` this service raises carries —
/// source: `smithy_query(query="shape",shape="NumberExportsLimitExceeded")` → doc
/// "Cannot start export as export quota limit was exceeded" (no final stop, as the
/// model writes it).
///
/// The sentence names no number: the service publishes no figure for its export quota,
/// and [`super::MAX_EXPORTS`] is this emulator's own ceiling rather than that quota.
pub const EXPORTS_LIMIT_MESSAGE: &str = "Cannot start export as export quota limit was exceeded";
