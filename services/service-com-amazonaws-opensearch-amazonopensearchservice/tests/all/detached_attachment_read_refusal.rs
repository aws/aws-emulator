// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDING of `DescribeDataSourceAttachment`'s missing-record
//! refusal — the sentence a client that confirms a detach by reading the attachment back
//! sees.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness and the per-operation request helpers
//! come from the sibling suites; this one adds no second copy of either.
//!
//! The rule asserted here, over a set of inputs rather than one recorded call: the
//! single-attachment read refuses an application and data source pair it holds no
//! attachment record for with the modelled `ResourceNotFoundException`, whose message
//! names the ABSENT ATTACHMENT keyed by both request members — the `id` first, then the
//! `dataSourceArn` — however the pair came to hold no record.
//!
//! * the pair whose attachment was made, reached `ATTACHED` and was then detached — the
//!   state the whole flow of attach, settle and detach leaves behind — over a table of
//!   data sources differing in the length and shape of the name their ARN carries, with a
//!   second attachment kept throughout so the refusal is keyed to the PAIR and not to an
//!   empty set;
//! * the pair that never had an attachment at all, over a table of data sources differing
//!   in why the account holds no record for them: a domain of this account never attached,
//!   the account's Glue Data Catalog, an ARN naming a domain this account does not hold,
//!   and a domain whose attachment belongs to ANOTHER application.
//!
//! The sibling suite [`super::control_plane_refusal_wording`] asserts the same sentence
//! on the never-attached read beside the DETACH's own, differently worded refusal; this
//! one pins it on the post-detach read and on the pair whose record lives elsewhere.

use serde_json::json;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::ATTACHMENT_STATUS_ATTACHED;

use super::control_plane_refusal_wording::message;
use super::data_source_attachments::{application, attach, describe_attachment, list_attachments};
use super::domain_lifecycle::{
    error_code, poll_until, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
    REGION,
};

/// `DetachDataSource`: `POST /2021-01-01/opensearch/application/{id}/detachDataSource`.
async fn detach(emulator: &Emulator, id: &str, data_source_arn: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/application/{id}/detachDataSource"),
            Some(json!({ "dataSourceArn": data_source_arn })),
        )
        .await
}

/// A domain of this account — the data source an attachment links an application to —
/// answering with its ARN.
async fn domain_arn(emulator: &Emulator, name: &str) -> String {
    let answer = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body["DomainStatus"], "ARN", name)
}

/// The sentence AWS states for an `id`/`dataSourceArn` pair that names no attachment:
/// fixed text with the request's two members substituted, in that order.
fn absent_attachment(id: &str, data_source_arn: &str) -> String {
    format!("Data source attachment not found for application {id} and data source {data_source_arn}")
}

/// Attach `data_source_arn` to `id` and wait for the attachment the clock owes a move to
/// reach `ATTACHED` — the condition the flow this suite is about starts from.
async fn attached(emulator: &Emulator, id: &str, data_source_arn: &str) {
    let answer = attach(emulator, id, json!({ "dataSourceArn": data_source_arn })).await;
    assert_eq!(
        answer.status, 200,
        "case AttachDataSource({data_source_arn:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let case = format!("the attachment of {data_source_arn:?} landing");
    poll_until(&case, || async {
        let read = describe_attachment(emulator, id, data_source_arn).await;
        read.body["status"].as_str() == Some(ATTACHMENT_STATUS_ATTACHED)
    })
    .await;
}

/// Assert that reading the attachment of `data_source_arn` on `id` is the modelled
/// missing-record refusal, in AWS's words.
async fn assert_absent(emulator: &Emulator, id: &str, data_source_arn: &str, case: &str) {
    let answer = describe_attachment(emulator, id, data_source_arn).await;
    let expected = absent_attachment(id, data_source_arn);
    assert_eq!(
        (answer.status, error_code(&answer).as_str()),
        (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
        "case {case}: expected the modelled not-found, got {} with {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        message(&answer),
        expected,
        "case {case}: expected {expected:?}, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn the_read_of_a_detached_attachment_names_the_absent_attachment() {
    let emulator = Emulator::new();
    let id = application(&emulator, "detached-read").await;

    // One row per shape of name a data source's ARN can carry within `DomainName`'s own
    // `^[a-z][a-z0-9\-]+$` and `@length(3..28)`
    // (`smithy_query(query="shape",shape="DomainName")`): the shortest, a digit-carrying
    // one, and one at the published upper bound — so the sentence is asserted over the
    // ARN rather than over one value.
    let names = ["dr0", "detached-read-9", "detached-read-long-name-0123"];
    let mut arns = Vec::new();
    for name in names {
        arns.push(domain_arn(&emulator, name).await);
    }
    // One attachment the application keeps throughout, so every refusal below is about
    // the pair the request names rather than about an application holding nothing.
    let kept = domain_arn(&emulator, "detached-read-kept").await;

    for arn in arns.iter().chain(std::iter::once(&kept)) {
        attached(&emulator, &id, arn).await;
    }

    for (name, arn) in names.iter().zip(&arns) {
        let dropped = detach(&emulator, &id, arn).await;
        assert_eq!(
            dropped.status, 200,
            "case DetachDataSource({name:?}): expected 200, got {} with {:?}",
            dropped.status, dropped.body
        );

        // The record is gone from the set — the condition a client polls for — before the
        // read of it is asserted.
        let case = format!("the attachment of {name:?} leaving the set");
        poll_until(&case, || async {
            let listed = list_attachments(&emulator, &id, json!({})).await;
            let entries = listed.body["attachments"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            entries
                .iter()
                .all(|entry| entry["dataSourceArn"].as_str() != Some(arn.as_str()))
        })
        .await;

        assert_absent(
            &emulator,
            &id,
            arn,
            &format!("a read of the detached data source {name:?}"),
        )
        .await;
    }

    // The attachment that was never detached still reads back, so no refusal above
    // removed a record it was not about.
    let read = describe_attachment(&emulator, &id, &kept).await;
    assert_eq!(
        (read.status, read.body["status"].as_str()),
        (200, Some(ATTACHMENT_STATUS_ATTACHED)),
        "case the kept attachment: it still reads back attached, got {} with {:?}",
        read.status,
        read.body
    );
}

#[tokio::test]
async fn the_read_of_a_pair_that_never_had_an_attachment_names_the_absent_attachment() {
    let emulator = Emulator::new();
    let id = application(&emulator, "absent-read").await;
    let other = application(&emulator, "absent-read-other").await;

    let never_attached = domain_arn(&emulator, "absent-read-src").await;
    let elsewhere = domain_arn(&emulator, "absent-read-elsewhere").await;
    // The attachment of `elsewhere` belongs to the OTHER application, so the pair this
    // application and that data source make still names no attachment.
    attached(&emulator, &other, &elsewhere).await;

    let account = ::engine::emulator_account_id();
    // One row per way the pair holds no attachment record, each ARN satisfying `ARN`'s
    // `@length(20..2048)` (`smithy_query(query="shape",shape="ARN")`) so the refusal
    // comes from the store rather than from the request boundary.
    let rows = vec![
        (
            "a domain of this account that was never attached",
            never_attached,
        ),
        (
            "the account's Glue Data Catalog, never attached",
            format!("arn:aws:glue:{REGION}:{account}:catalog"),
        ),
        (
            "an ARN naming a domain this account does not hold",
            format!("arn:aws:es:{REGION}:{account}:domain/absent-read-unheld"),
        ),
        (
            "a data source whose attachment belongs to another application",
            elsewhere.clone(),
        ),
    ];

    for (case, arn) in &rows {
        assert_absent(&emulator, &id, arn, case).await;
    }

    // The record the other application does hold is untouched: the refusals above are
    // about the pair, so none of them removed or hid an attachment of another
    // application.
    let read = describe_attachment(&emulator, &other, &elsewhere).await;
    assert_eq!(
        (read.status, read.body["status"].as_str()),
        (200, Some(ATTACHMENT_STATUS_ATTACHED)),
        "case the other application's attachment: it still reads back attached, got {} with {:?}",
        read.status,
        read.body
    );
}
