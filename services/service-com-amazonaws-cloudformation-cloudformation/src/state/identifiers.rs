// SPDX-License-Identifier: CC0-1.0

//! The identifiers this service MINTS: the opaque values a client reads back as a stack id,
//! an operation id, a change set id, a client request token and the rest.
//!
//! One concern, in one place: every id has the FORM CloudFormation gives that kind of id, and
//! every one of them is derived here rather than at the call site, so two operations minting
//! the same kind of id cannot give it two shapes.

use super::*;

/// Mint a fresh identifier in the 8-4-4-4-12 lowercase-hex form CloudFormation's stack
/// ids, event ids and operation ids carry.
///
/// The value is a SHA-256 digest of a per-process counter, the current instant and the
/// caller's seed: the counter alone makes every call distinct, so two ids minted in the
/// same nanosecond still differ, and the digest gives the opaque 128-bit shape a client
/// sees. It is an identifier, never a secret — nothing authenticates with it, and the
/// crate's existing `sha2`/`hex` dependencies are all it needs.
pub(crate) fn new_identifier(seed: &str) -> String {
    let digest = new_identifier_digits(seed, 32);
    format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32]
    )
}

/// The digest behind [`new_identifier`], as `digits` lowercase hexadecimal characters and
/// without the 8-4-4-4-12 grouping — for an identifier whose published form is not a
/// uuid's (the id of the saved Simple Monthly Calculator estimate `EstimateTemplateCost`
/// names, which is 40 hexadecimal characters).
///
/// Never more than the digest's own 64 characters, so a caller cannot ask for digits
/// nothing computed.
pub fn new_identifier_digits(seed: &str, digits: usize) -> String {
    use ::sha2::{Digest, Sha256};

    static MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);
    let counter = MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    hasher.update(counter.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let digest = ::hex::encode(hasher.finalize());
    digest[..digits.min(digest.len())].to_string()
}

/// The lowercase-hexadecimal digest of `value`, as `digits` characters: the STABLE
/// identifier of something whose identity IS that value, where [`new_identifier_digits`]
/// mints a fresh one on every call.
///
/// Same digest, same shape, same `sha2`/`hex` dependencies — only the input differs: nothing
/// per-call goes into it, so the same value answers the same identifier in every Region, on
/// every read and across restarts. It is an identifier, never a secret.
///
/// Never more than the digest's own 64 characters, so a caller cannot ask for digits nothing
/// computed.
pub fn stable_identifier_digits(value: &str, digits: usize) -> String {
    use ::sha2::{Digest, Sha256};

    let digest = ::hex::encode(Sha256::digest(value.as_bytes()));
    digest[..digits.min(digest.len())].to_string()
}

/// Mint a stack id: `arn:aws:cloudformation:<region>:<account>:stack/<name>/<uuid>`,
/// the form CloudFormation gives a stack. The account segment comes from the
/// emulator-wide [`::emulator_server::emulator_arn`] helper.
pub fn new_stack_id(region: &str, stack_name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("stack/{stack_name}/{}", new_identifier(stack_name)),
    )
}

/// Mint an id for one stack operation (`CreateStackOutput$OperationId` and the
/// `StackEvent$OperationId` of every event that operation causes).
pub fn new_operation_id() -> String {
    new_identifier("operation")
}

/// Mint the client request token CloudFormation assigns a stack operation whose caller
/// supplied none.
///
/// `StackEvent$ClientRequestToken`: "All events triggered by a given stack operation are
/// assigned the same client request token, which you can use to track operations" — so an
/// operation HAS a token whether or not the request named one. The value satisfies the
/// modelled `ClientRequestToken` (length 1..128, pattern `^[a-zA-Z0-9][-a-zA-Z0-9]*$`).
pub fn new_client_request_token() -> String {
    new_identifier("client-request-token")
}

/// Mint an id for one drift-detection operation (`DetectStackDriftOutput`'s
/// `StackDriftDetectionId`, which `DescribeStackDriftDetectionStatus` reads a result back
/// by): the same 8-4-4-4-12 form as every other identifier this service mints, which is 36
/// characters and so satisfies the modelled `StackDriftDetectionId` length (1..36).
pub fn new_drift_detection_id() -> String {
    new_identifier("drift")
}

/// Mint the `PublisherId` a `RegisterPublisher` call assigns this account.
///
/// The same minted identifier as every other one this service hands out, with the group
/// separators dropped: `PublisherId` is modelled `@pattern("^[0-9a-zA-Z]{12,40}$")`, which
/// admits no `-`, so the id is the digest's 32 hex characters — inside that pattern and
/// inside the modelled length (1..40).
pub fn new_publisher_id() -> String {
    new_identifier("publisher").replace('-', "")
}

/// Mint a change set id: `arn:aws:cloudformation:<region>:<account>:changeSet/<name>/<uuid>`,
/// the form CloudFormation gives a change set (`ChangeSetId` is modelled
/// `@pattern("^arn:[-a-zA-Z0-9:/]*$")`).
pub fn new_change_set_id(region: &str, change_set_name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("changeSet/{change_set_name}/{}", new_identifier(change_set_name)),
    )
}

/// Mint a generated template's id, which is its ARN: `GeneratedTemplateId` is "the Amazon
/// Resource Name (ARN) of the generated template. The format is
/// `arn:${Partition}:cloudformation:${Region}:${Account}:generatedtemplate/${Id}`" — the
/// same 8-4-4-4-12 id every other identifier this service mints carries.
pub fn new_generated_template_id(region: &str, generated_template_name: &str) -> String {
    ::emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!(
            "generatedtemplate/{}",
            new_identifier(generated_template_name)
        ),
    )
}

/// Mint a resource scan's id, which is its ARN: `ResourceScanId` is "the Amazon Resource Name
/// (ARN) of the resource scan. The format is
/// `arn:${Partition}:cloudformation:${Region}:${Account}:resourceScan/${Id}`" — the same
/// 8-4-4-4-12 id every other identifier this service mints carries, in the ARN form
/// [`resource_scans::resource_scan_arn`] builds.
pub fn new_resource_scan_id(region: &str) -> String {
    resource_scans::resource_scan_arn(region, &new_identifier("resource-scan"))
}

/// Mint a stack refactor's id — `CreateStackRefactorOutput$StackRefactorId`, which
/// `DescribeStackRefactor` reads the refactor back by.
///
/// The same 8-4-4-4-12 lowercase-hex identifier every other id this service mints carries:
/// the model constrains `StackRefactorId` with neither a length nor a pattern, and the
/// refactor is a record of this account and Region rather than a resource with an ARN of its
/// own, so there is nothing else for the id to be.
pub fn new_stack_refactor_id() -> String {
    new_identifier("stack-refactor")
}
