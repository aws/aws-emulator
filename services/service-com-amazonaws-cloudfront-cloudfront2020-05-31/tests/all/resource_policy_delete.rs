// SPDX-License-Identifier: CC0-1.0

//! Removing a CloudFront resource policy, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test: the resource-policy surface addresses a policy only by
//! the ARN of the resource it is attached to, and the ARNs it accepts are those naming the
//! ONE resource type it attaches policies to, so for the delete
//!
//! * a `ResourceArn` the surface accepts that carries a policy loses it: the delete answers
//!   HTTP 200 and from then on the resource carries no policy, so a read answers
//!   `EntityNotFound` until another write attaches one — and no other resource's policy is
//!   touched, since a policy is keyed by the ARN it was attached to alone;
//! * an accepted `ResourceArn` carrying no policy is `EntityNotFound` (404) with
//!   [`RESOURCE_POLICY_NOT_FOUND_MESSAGE`] — there is nothing to remove, so a repeated
//!   delete is refused rather than answered a second success;
//! * a `ResourceArn` that is no ARN at all is refused on its FORM (400 `InvalidArgument`,
//!   [`MALFORMED_ARN_MESSAGE`]); an ARN naming a resource this service HOLDS, of another
//!   type, is refused on the resource TYPE it names (400 `InvalidArgument`,
//!   [`INVALID_RESOURCE_POLICY_RESOURCE_TYPE_MESSAGE`] — the sentence the surface's other
//!   WRITE puts on the wire); and an ARN this service holds no resource for is reported
//!   ABSENT (404 `EntityNotFound`, [`NO_SUCH_RESOURCE_MESSAGE`]), each decided before
//!   anything is removed.
//!
//! The rows differ along what the rule generalises over: the resource type the ARN names
//! (every type this service mints an ARN for, and the forms an ARN can take that name none),
//! the ARN's partition, region and account segments, the endpoint configuration the accepted
//! resource was created from, the content and size of the policy document, how many writes
//! preceded the delete and how many deletes follow it.
//!
//! The wire harness is the one [`crate::distribution_lifecycle`] builds and the surface's
//! request helpers are the ones [`crate::resource_policy_lifecycle`] already writes.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    INVALID_RESOURCE_POLICY_RESOURCE_TYPE_MESSAGE, MALFORMED_ARN_MESSAGE,
    NO_SUCH_RESOURCE_MESSAGE, RESOURCE_POLICY_NOT_FOUND_MESSAGE, RESOURCE_POLICY_RESOURCE_TYPE,
    TAGGABLE_RESOURCE_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, create_distribution, emulator, Answer, ACCOUNT_ID, API_VERSION};
use crate::distribution_tenant_lifecycle::escape;
use crate::resource_policy_lifecycle::{
    created_vpc_origin_arn, distribution_config, get_resource_policy, message, policy,
    put_resource_policy, reported_document, OTHER_RESOURCE_TYPES,
};

/// `POST /2020-05-31/delete-resource-policy`, removing the policy attached to `arn`.
///
/// `pub(crate)` so a sibling suite that removes a policy goes over the same wire path rather
/// than growing a copy of this.
pub(crate) async fn delete_resource_policy(router: &MultiServiceRouter, arn: &str) -> Answer {
    let body = format!(
        "<DeleteResourcePolicyRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <ResourceArn>{}</ResourceArn>\
         </DeleteResourcePolicyRequest>",
        escape(arn)
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/delete-resource-policy"),
        Some(&body),
        None,
    )
    .await
}

/// Every row of the round-trip table: the case, the ARN the policy is attached to and the
/// document attached to it.
///
/// Each ARN is one a `CreateVpcOrigin` really minted, since a policy is attached to a
/// resource that EXISTS. The rows differ along the endpoint configuration's name, and along
/// the content and size of the document.
async fn rows(router: &MultiServiceRouter) -> Vec<(&'static str, String, String)> {
    let bulky = format!(
        "{{\"Version\":\"2008-10-17\",\"Statement\":[{}]}}",
        (0..40)
            .map(|index| format!(
                "{{\"Effect\":\"Allow\",\"Sid\":\"statement{index}\",\"Action\":\"cloudfront:*\"}}"
            ))
            .collect::<Vec<_>>()
            .join(",")
    );
    let table: Vec<(&'static str, String, String)> = vec![
        (
            "a document a caller would write",
            "vo-delete-1".to_string(),
            policy("minted"),
        ),
        (
            "a statement id in upper case",
            "vo-delete-upper".to_string(),
            policy("UPPER"),
        ),
        (
            "a one-character resource name",
            "7".to_string(),
            policy("short"),
        ),
        (
            "a resource name of sixty-four characters",
            "d".repeat(64),
            policy("long-name"),
        ),
        (
            "a resource name carrying separators",
            "vo.delete_5".to_string(),
            policy("separators"),
        ),
        (
            "an empty JSON object",
            "vo-delete-empty-object".to_string(),
            "{}".to_string(),
        ),
        (
            "a document of forty statements",
            "vo-delete-bulky".to_string(),
            bulky,
        ),
    ];
    let mut out = Vec::new();
    for (case, name, document) in table {
        out.push((case, created_vpc_origin_arn(router, &name).await, document));
    }
    out
}

// ---------------------------------------------------------------------------
// The removal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_the_policy_of_an_accepted_arn_removes_only_that_policy() {
    let router = emulator();
    let rows = rows(&router).await;

    // Every row is attached first, so each delete below runs against a store holding every
    // other row's policy too.
    for (case, arn, document) in &rows {
        let written = put_resource_policy(&router, arn, document).await;
        assert_eq!(
            written.status, 200,
            "case {case}: expected HTTP 200 from the write, got {} ({})",
            written.status, written.raw
        );
    }

    for (index, (case, arn, _)) in rows.iter().enumerate() {
        let deleted = delete_resource_policy(&router, arn).await;
        assert_eq!(
            deleted.status, 200,
            "case {case}: expected HTTP 200 from the delete, got {} ({})",
            deleted.status, deleted.raw
        );
        assert_eq!(
            deleted.error_code(),
            "",
            "case {case}: expected the delete to report no error, got {:?} ({})",
            deleted.error_code(),
            deleted.raw
        );

        // The resource carries no policy any more: the READ is what the delete changed.
        let read = get_resource_policy(&router, arn).await;
        assert_eq!(
            read.status, 404,
            "case {case}: expected HTTP 404 reading the deleted policy, got {} ({})",
            read.status, read.raw
        );
        assert_eq!(
            read.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound reading the deleted policy, got {:?} ({})",
            read.error_code(),
            read.raw
        );
        assert_eq!(
            message(&read),
            RESOURCE_POLICY_NOT_FOUND_MESSAGE,
            "case {case}: expected message {RESOURCE_POLICY_NOT_FOUND_MESSAGE:?}, got {:?}",
            message(&read)
        );

        // Nothing but that one key moved: every row not yet deleted still reads back with
        // the document it was attached, and every row already deleted is still gone.
        for (other_case, other_arn, other_document) in &rows {
            let other = get_resource_policy(&router, other_arn).await;
            if other_arn == arn {
                continue;
            }
            let already_deleted = rows
                .iter()
                .take(index)
                .any(|(_, deleted_arn, _)| deleted_arn == other_arn);
            if already_deleted {
                assert_eq!(
                    other.status, 404,
                    "case {case} then {other_case}: expected HTTP 404 for an already deleted \
                     policy, got {} ({})",
                    other.status, other.raw
                );
            } else {
                assert_eq!(
                    other.status, 200,
                    "case {case} then {other_case}: expected HTTP 200 for a policy no delete \
                     named, got {} ({})",
                    other.status, other.raw
                );
                assert_eq!(
                    reported_document(&other),
                    other_document.as_str(),
                    "case {case} then {other_case}: expected the document \
                     {other_document:?}, got {:?}",
                    reported_document(&other)
                );
            }
        }
    }
}

#[tokio::test]
async fn deleting_a_policy_an_accepted_arn_does_not_carry_is_entity_not_found() {
    let router = emulator();

    // Rows spanning the ways an accepted ARN comes to carry no policy: never written,
    // written and already deleted, deleted twice, and an ARN beside one that does carry a
    // policy.
    let neighbour = created_vpc_origin_arn(&router, "vo-keeps-its-policy").await;
    let kept = policy("kept");
    let written = put_resource_policy(&router, &neighbour, &kept).await;
    assert_eq!(
        written.status, 200,
        "case the neighbour: expected HTTP 200 from the write, got {} ({})",
        written.status, written.raw
    );

    let never_written = created_vpc_origin_arn(&router, "vo-never-written-delete").await;
    let once_written = created_vpc_origin_arn(&router, "vo-written-once").await;
    let twice_deleted = created_vpc_origin_arn(&router, "vo-deleted-twice").await;
    for arn in [&once_written, &twice_deleted] {
        let put = put_resource_policy(&router, arn, &policy("to-be-removed")).await;
        assert_eq!(
            put.status, 200,
            "case preparing {arn}: expected HTTP 200 from the write, got {} ({})",
            put.status, put.raw
        );
        let deleted = delete_resource_policy(&router, arn).await;
        assert_eq!(
            deleted.status, 200,
            "case preparing {arn}: expected HTTP 200 from the delete, got {} ({})",
            deleted.status, deleted.raw
        );
    }

    let rows = [
        ("an ARN no write ever named", never_written),
        ("an ARN whose policy was deleted", once_written),
        ("the same ARN deleted a second time", twice_deleted.clone()),
        ("the same ARN deleted a third time", twice_deleted),
        (
            "a resource created after the others, no write named",
            created_vpc_origin_arn(&router, "vo-late-arrival").await,
        ),
    ];

    for (case, arn) in &rows {
        let answer = delete_resource_policy(&router, arn).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {} ({})",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound, got {:?} ({})",
            answer.error_code(),
            answer.raw
        );
        assert_eq!(
            message(&answer),
            RESOURCE_POLICY_NOT_FOUND_MESSAGE,
            "case {case}: expected message {RESOURCE_POLICY_NOT_FOUND_MESSAGE:?}, got {:?}",
            message(&answer)
        );
    }

    // None of those refusals removed the policy the neighbouring resource does carry.
    let read = get_resource_policy(&router, &neighbour).await;
    assert_eq!(
        reported_document(&read),
        kept.as_str(),
        "case after the not-found deletes: expected the document {kept:?}, got {:?}",
        reported_document(&read)
    );
}

// ---------------------------------------------------------------------------
// The parameter gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_under_an_arn_the_surface_does_not_accept_is_refused() {
    let router = emulator();

    // The policy the gate must leave alone: every refusal below is decided before anything
    // is removed.
    let kept_arn = created_vpc_origin_arn(&router, "vo-survives-the-gate").await;
    let kept = policy("survivor");
    let written = put_resource_policy(&router, &kept_arn, &kept).await;
    assert_eq!(
        written.status, 200,
        "case the surviving policy: expected HTTP 200 from the write, got {} ({})",
        written.status, written.raw
    );

    // A resource that EXISTS, of a type the surface attaches no policy to: refused on the
    // TYPE the ARN names, with the sentence this WRITE words that condition in.
    let created = create_distribution(&router, &distribution_config("ref-delete-gate")).await;
    assert_eq!(
        created.status, 201,
        "the distribution must be created for its row to be about its TYPE, got {} ({})",
        created.status, created.raw
    );
    let distribution_arn = created
        .root()
        .find_text("ARN")
        .unwrap_or_default()
        .to_string();
    let wrong_type = [(
        "an ARN naming a distribution that exists".to_string(),
        distribution_arn,
    )];

    // Every other resource type this service mints an ARN for, naming a resource the store
    // holds none of, plus the ARN forms that name no resource at all: reported ABSENT, since
    // there is no resource to remove a policy from. The type this surface DOES accept is
    // answered by whether the ARN carries a policy, which this suite's own delete rows cover.
    let mut absent: Vec<(String, String)> = TAGGABLE_RESOURCE_TYPES
        .iter()
        .chain(OTHER_RESOURCE_TYPES.iter())
        .filter(|kind| **kind != RESOURCE_POLICY_RESOURCE_TYPE)
        .map(|kind| {
            (
                format!("an ARN naming an absent {kind}"),
                format!("arn:aws:cloudfront::{ACCOUNT_ID}:{kind}/absent-resource"),
            )
        })
        .collect();
    absent.extend([
        ("a bare ARN prefix".to_string(), "arn:".to_string()),
        (
            "an ARN carrying no resource part".to_string(),
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:"),
        ),
        (
            "an ARN of another service naming the accepted type".to_string(),
            format!("arn:aws:ec2:us-east-1:{ACCOUNT_ID}:{RESOURCE_POLICY_RESOURCE_TYPE}/vo-1"),
        ),
        (
            "an ARN with no partition".to_string(),
            format!("arn::cloudfront::{ACCOUNT_ID}:{RESOURCE_POLICY_RESOURCE_TYPE}/vo-1"),
        ),
        (
            "an ARN whose resource type merely starts with the accepted one".to_string(),
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:{RESOURCE_POLICY_RESOURCE_TYPE}-group/vo-1"),
        ),
        (
            "an ARN naming the accepted type with no id".to_string(),
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:{RESOURCE_POLICY_RESOURCE_TYPE}"),
        ),
        (
            "an ARN naming the accepted type with an empty id".to_string(),
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:{RESOURCE_POLICY_RESOURCE_TYPE}/"),
        ),
    ]);

    // The forms a value that is no ARN at all takes: refused on its FORM.
    let malformed: Vec<(String, String)> = vec![
        ("the empty string".to_string(), String::new()),
        ("a bare word".to_string(), "not-an-arn".to_string()),
        (
            "the resource part alone".to_string(),
            format!("{RESOURCE_POLICY_RESOURCE_TYPE}/vo-1"),
        ),
        (
            "the ARN of the policy that exists, with the prefix stripped".to_string(),
            kept_arn
                .strip_prefix("arn:")
                .unwrap_or_default()
                .to_string(),
        ),
        (
            "the same ARN with the prefix in upper case".to_string(),
            format!("ARN:{}", kept_arn.strip_prefix("arn:").unwrap_or_default()),
        ),
        (
            "a URL rather than an ARN".to_string(),
            format!("https://cloudfront.amazonaws.com/{RESOURCE_POLICY_RESOURCE_TYPE}/vo-1"),
        ),
        (
            "a value that merely contains an ARN".to_string(),
            format!("resource={kept_arn}"),
        ),
    ];

    let cases = wrong_type
        .iter()
        .map(|(case, value)| {
            (
                case,
                value,
                400,
                "InvalidArgument",
                INVALID_RESOURCE_POLICY_RESOURCE_TYPE_MESSAGE,
            )
        })
        .chain(absent.iter().map(|(case, value)| {
            (
                case,
                value,
                404,
                "EntityNotFound",
                NO_SUCH_RESOURCE_MESSAGE,
            )
        }))
        .chain(
            malformed
                .iter()
                .map(|(case, value)| (case, value, 400, "InvalidArgument", MALFORMED_ARN_MESSAGE)),
        );

    for (case, value, status, code, expected) in cases {
        let answer = delete_resource_policy(&router, value).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status} for {value:?}, got {} ({})",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case}: expected {code} for {value:?}, got {:?} ({})",
            answer.error_code(),
            answer.raw
        );
        assert_eq!(
            message(&answer),
            expected,
            "case {case}: expected message {expected:?}, got {:?}",
            message(&answer)
        );
    }

    // The refusals removed nothing: the one policy the store holds is still readable.
    let read = get_resource_policy(&router, &kept_arn).await;
    assert_eq!(
        read.status, 200,
        "case after the refusals: expected HTTP 200, got {} ({})",
        read.status, read.raw
    );
    assert_eq!(
        reported_document(&read),
        kept.as_str(),
        "case after the refusals: expected the document {kept:?}, got {:?}",
        reported_document(&read)
    );
}

// ---------------------------------------------------------------------------
// Writing again after a delete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_resource_takes_a_new_policy_after_its_policy_is_deleted() {
    let router = emulator();
    let arn = created_vpc_origin_arn(&router, "vo-reattached").await;

    // Attach, remove and attach again, several times over and with a different document
    // each round: the delete removes the policy rather than the key's ability to carry one,
    // so each round reads back its own document.
    let documents = [policy("first"), "{}".to_string(), policy("third")];
    for (round, document) in documents.iter().enumerate() {
        let written = put_resource_policy(&router, &arn, document).await;
        assert_eq!(
            written.status, 200,
            "case round {round}: expected HTTP 200 from the write, got {} ({})",
            written.status, written.raw
        );
        let read = get_resource_policy(&router, &arn).await;
        assert_eq!(
            reported_document(&read),
            document.as_str(),
            "case round {round}: expected the document {document:?}, got {:?}",
            reported_document(&read)
        );

        let deleted = delete_resource_policy(&router, &arn).await;
        assert_eq!(
            deleted.status, 200,
            "case round {round}: expected HTTP 200 from the delete, got {} ({})",
            deleted.status, deleted.raw
        );
        let absent = get_resource_policy(&router, &arn).await;
        assert_eq!(
            absent.status, 404,
            "case round {round}: expected HTTP 404 after the delete, got {} ({})",
            absent.status, absent.raw
        );
    }
}
