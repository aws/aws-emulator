// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDING and the CLASS of this service's control-plane
//! refusals — the half of an answer a client branches on.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness and the per-operation request helpers
//! come from the sibling suites; this one adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a refusal that could not resolve the application a request named states the
//!   sentence of the SURFACE it was asked on — the fixed one for the CRUD operations
//!   that address one application, the id for every sub-API of one, and the id with the
//!   account beside it on the one path whose refusal states both — over a table of ids,
//!   with the start of a migration stating that no such application exists, naming the
//!   id, under the status the service answers its own miss with;
//! * a create refused for a name the account already uses names that name;
//! * the two data-source-attachment refusals state the application and the data source,
//!   each in the words of the operation that answers it, over a table of data sources;
//! * a value outside a member's declared expression is refused with the constraint
//!   report every other such value earns — the envelope, the member path and the
//!   expression — for `IndexName`, whose pattern the request boundary cannot run;
//! * a `migrationId` that names neither an application nor a job is refused as the
//!   malformed value it is, over a table of ways to fail the form;
//! * the observability and maintenance READS of a domain this account does not hold
//!   answer the GENERIC envelope those operations model, while the scheduled-action
//!   listing beside them answers its modelled not-found.

use serde_json::json;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    APPLICATION_DOES_NOT_EXIST, INDEX_NAME_PATTERN, NO_PROGRESS_INFORMATION,
};

use super::application_updates::{create_application, get_application, update_application};
use super::data_source_attachments::{
    application, attach, describe_attachment, list_attachments,
};
use super::data_source_registry::created_domain;
use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};
use super::index_registry::{create as create_index, get as get_index, stored_names};
use super::migration_listing::list_migrations;
use super::saved_object_migrations::get_migration;

/// The ids a lookup is driven with: the shortest and the longest `Id` the model's
/// `^[a-z0-9]{3,30}$` admits, a digits-only one and a word-shaped one — so the rule is
/// asserted over the LENGTH and the alphabet of the id and not over one value.
fn absent_application_ids() -> Vec<String> {
    vec![
        "abc".to_string(),
        "a".repeat(30),
        "0123456789".to_string(),
        "noapplicationhere".to_string(),
    ]
}

/// The error message of a refused answer. Shared with the suites that assert the
/// refusals of other surfaces in this crate.
pub(crate) fn message(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .or_else(|| answer.body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// `DeleteApplication`: `DELETE /2021-01-01/opensearch/application/{id}`.
async fn delete_application(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/application/{id}"),
            None,
        )
        .await
}

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

/// `StartMigration`: `POST /2021-01-01/opensearch/app-migrations`. The source is the
/// search-collection ARN this service validates a migration source against.
async fn start_migration(emulator: &Emulator, id: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/app-migrations"),
            Some(json!({
                "applicationId": id,
                "migrationOptions": {
                    "source": {
                        "datasourceArn":
                            "arn:aws:aoss:us-east-1:123456789012:collection/wordingsource",
                    },
                    "workspace": { "workspaceId": "ws-1" },
                },
            })),
        )
        .await
}

/// A domain of this account, answering with its ARN — the data source an attachment
/// links an application to.
async fn domain_arn(emulator: &Emulator, name: &str) -> String {
    let answer = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body["DomainStatus"], "ARN", name)
}

#[tokio::test]
async fn an_application_refusal_states_the_sentence_of_the_surface_it_was_asked_on() {
    let emulator = Emulator::new();
    // One application the account DOES hold, so every refusal below is about the id the
    // request carried rather than about an empty store.
    let held = application(&emulator, "wording-held").await;
    let arn = domain_arn(&emulator, "wording-source").await;

    for id in absent_application_ids() {
        assert_ne!(id, held, "the absent ids must not name the held application");

        // The CRUD surface addresses ONE application, so its refusal states the fixed
        // sentence and names no id.
        for (operation, answer) in [
            ("GetApplication", get_application(&emulator, &id).await),
            (
                "UpdateApplication",
                update_application(&emulator, &id, json!({})).await,
            ),
        ] {
            let case = format!("{operation} for the absent application {id:?}");
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
                "case {case}: expected the modelled not-found, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                message(&answer),
                APPLICATION_DOES_NOT_EXIST,
                "case {case}: expected {APPLICATION_DOES_NOT_EXIST:?}, got {:?}",
                answer.body
            );
        }

        // The delete asks for a state the account is already in, so it is answered.
        let dropped = delete_application(&emulator, &id).await;
        assert_eq!(
            dropped.status, 200,
            "case DeleteApplication for the absent application {id:?}: expected 200, got {} with \
             {:?}",
            dropped.status, dropped.body
        );

        // A sub-API of an application names the id it could not resolve, in the
        // not-found error its model declares.
        let expected = format!("Application {id} not found");
        for (operation, answer) in [
            (
                "ListDataSourceAttachments",
                list_attachments(&emulator, &id, json!({})).await,
            ),
            (
                "DescribeDataSourceAttachment",
                describe_attachment(&emulator, &id, &arn).await,
            ),
            ("DetachDataSource", detach(&emulator, &id, &arn).await),
        ] {
            let case = format!("{operation} for the absent application {id:?}");
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

        // `StartMigration` states what the CRUD surface states about such an id — that
        // no such application exists — with the id named in it, under the status this
        // service answers a missing resource on: the modelled shape's `@httpError` is
        // 409 and the recorded answer for a start naming an application this account
        // does not hold is 404.
        let started = start_migration(&emulator, &id).await;
        let case = format!("StartMigration for the absent application {id:?}");
        let states = format!("Application {id} does not exist");
        assert_eq!(
            (started.status, error_code(&started).as_str()),
            (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case {case}: expected the modelled not-found under {RECORDED_NOT_FOUND_STATUS}, got \
             {} with {:?}",
            started.status,
            started.body
        );
        assert_eq!(
            message(&started),
            states,
            "case {case}: expected {states:?}, got {:?}",
            started.body
        );

        // `ListMigrations` states the same sentence as the other sub-APIs of an
        // application, in the missing-resource shape they all state it with — the shape
        // this service answers a resource that does not exist with, which its own model
        // does not list and which is widened in for exactly this refusal.
        let listed = list_migrations(&emulator, &id, "").await;
        assert_eq!(
            (listed.status, error_code(&listed).as_str()),
            (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case ListMigrations for the absent application {id:?}: expected the modelled \
             not-found, got {} with {:?}",
            listed.status,
            listed.body
        );
        assert_eq!(
            message(&listed),
            expected,
            "case ListMigrations for the absent application {id:?}: expected {expected:?}, got \
             {:?}",
            listed.body
        );

        // The attach path states the account the id was resolved in beside the id.
        let attached = attach(&emulator, &id, json!({ "dataSourceArn": arn })).await;
        let expected_for_account =
            format!("{expected} for account {}", ::engine::emulator_account_id());
        assert_eq!(
            (attached.status, error_code(&attached).as_str()),
            (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case AttachDataSource for the absent application {id:?}: expected the modelled \
             not-found, got {} with {:?}",
            attached.status,
            attached.body
        );
        assert_eq!(
            message(&attached),
            expected_for_account,
            "case AttachDataSource for the absent application {id:?}: expected \
             {expected_for_account:?}, got {:?}",
            attached.body
        );
    }

    // The application the account does hold is untouched by every refusal above.
    let read = get_application(&emulator, &held).await;
    assert_eq!(
        read.status, 200,
        "case the held application: it still resolves, got {} with {:?}",
        read.status, read.body
    );
}

#[tokio::test]
async fn a_duplicate_application_name_refusal_names_the_name() {
    let emulator = Emulator::new();

    // One row per shape of name `CreateApplication`'s `^[a-z][a-z0-9\-]+$` admits within
    // its `@length(min: 3)`: the shortest, a hyphenated one and a digit-carrying one.
    for name in ["abc", "wording-dup-app", "app9"] {
        let first = create_application(&emulator, json!({ "name": name })).await;
        assert_eq!(
            first.status, 200,
            "case {name:?}: expected the first create to be accepted, got {} with {:?}",
            first.status, first.body
        );
        let again = create_application(&emulator, json!({ "name": name })).await;
        let expected = format!("Application name {name} already exists");
        assert_eq!(
            (again.status, error_code(&again).as_str()),
            (409, "ConflictException"),
            "case {name:?}: expected the modelled conflict, got {} with {:?}",
            again.status,
            again.body
        );
        assert_eq!(
            message(&again),
            expected,
            "case {name:?}: expected {expected:?}, got {:?}",
            again.body
        );
    }
}

#[tokio::test]
async fn the_attachment_refusals_name_the_application_and_the_data_source() {
    let emulator = Emulator::new();
    let id = application(&emulator, "wording-attach").await;

    // One row per data source a refusal can be about: a domain of this account that was
    // never attached, and a second one — so the sentence is asserted over the ARN rather
    // than over one value.
    let unattached = vec![
        domain_arn(&emulator, "wording-unattached-a").await,
        domain_arn(&emulator, "wording-unattached-b").await,
    ];
    // "The application must be in the ACTIVE state" for a detach, so the provisioning the
    // create owes the application is let fall due before the rows below.
    tokio::time::sleep(SETTLE_WAIT).await;
    for arn in &unattached {
        // (operation, the request, the sentence that operation states). The two
        // operations word the same miss differently: the READ states it as the absent
        // attachment RECORD, the DETACH as the link that was never made.
        let read_sentence =
            format!("Data source attachment not found for application {id} and data source {arn}");
        let detach_sentence = format!("Data source {arn} is not attached to application {id}.");
        for (operation, answer, expected) in [
            (
                "DescribeDataSourceAttachment",
                describe_attachment(&emulator, &id, arn).await,
                read_sentence.as_str(),
            ),
            (
                "DetachDataSource",
                detach(&emulator, &id, arn).await,
                detach_sentence.as_str(),
            ),
        ] {
            let case = format!("{operation} for the unattached data source {arn:?}");
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
    }

    // An attachment of a data source that is still being provisioned is PENDING, and a
    // detach of one is refused while it is — stated with the ARN, the application and the
    // status the attachment rests in.
    for name in ["wording-pending-a", "wording-pending-b"] {
        let arn = domain_arn(&emulator, name).await;
        let attached = attach(&emulator, &id, json!({ "dataSourceArn": arn })).await;
        assert_eq!(
            attached.status, 200,
            "case AttachDataSource({name:?}): expected 200, got {} with {:?}",
            attached.status, attached.body
        );
        let answer = detach(&emulator, &id, &arn).await;
        let expected =
            format!("Data source {arn} attachment to application {id} is still PENDING.");
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (409, "ConflictException"),
            "case DetachDataSource({name:?}) while pending: expected the modelled conflict, got \
             {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            message(&answer),
            expected,
            "case DetachDataSource({name:?}) while pending: expected {expected:?}, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn an_index_name_outside_its_pattern_is_the_constraint_report() {
    let emulator = Emulator::new();
    let domain = "wording-index";
    created_domain(&emulator, domain).await;
    tokio::time::sleep(SETTLE_WAIT).await;

    // One row per part of `IndexName`'s pattern a name can break — an upper-case letter,
    // a leading underscore, a leading hyphen, each of the two names the lookahead refuses
    // outright, and one of the forbidden characters. Every row is a name that is also
    // safe in a request URI, so the SAME name is driven through the create and the read.
    // Source: `smithy_query(query="operation",operation="CreateIndex")`.
    for name in [
        "INVALID_UPPER",
        "camelCase",
        "_leading-underscore",
        "-leading-hyphen",
        ".",
        "..",
        "has,comma",
    ] {
        let expected = format!(
            "1 validation error detected: Value '{name}' at 'indexName' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: {INDEX_NAME_PATTERN}"
        );
        for (operation, answer) in [
            (
                "CreateIndex",
                create_index(
                    &emulator,
                    domain,
                    json!({
                        "IndexName": name,
                        "IndexSchema": { "mappings": { "properties": {} }, "settings": {} },
                    }),
                )
                .await,
            ),
            ("GetIndex", get_index(&emulator, domain, name).await),
        ] {
            let case = format!("{operation} for the index name {name:?}");
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (400, "ValidationException"),
                "case {case}: expected the modelled validation error, got {} with {:?}",
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
    }

    // A refused create persisted nothing: the domain holds no index at all.
    let stored = stored_names(&emulator, domain, "refused index names");
    assert!(
        stored.is_empty(),
        "case refused index names: expected the domain to hold no index, got {stored:?}"
    );
}

#[tokio::test]
async fn a_migration_id_that_names_no_migration_is_refused_as_malformed() {
    let emulator = Emulator::new();

    // One row per way a value fails to name a migration: no hyphen at all (so it names
    // no application and no job), a leading half shorter than the `Id` lower bound,
    // longer than its upper bound, outside its alphabet (an upper-case letter, a dot),
    // and a hyphen with nothing after it.
    for id in [
        "0123456789".to_string(),
        "ab-1".to_string(),
        format!("{}-x", "a".repeat(31)),
        "ABCDEF-1".to_string(),
        "has.dot-1".to_string(),
        "abc-".to_string(),
    ] {
        let answer = get_migration(&emulator, &id).await;
        let expected = format!("Invalid migrationId format: {id}");
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case GetMigration({id:?}): expected the modelled validation error, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            message(&answer),
            expected,
            "case GetMigration({id:?}): expected {expected:?}, got {:?}",
            answer.body
        );
    }

    // A request that carries NO id at all addresses the collection the path names, which
    // is the listing — and that operation requires the application the request names
    // none of.
    let empty = get_migration(&emulator, "").await;
    let expected = "1 validation error detected: Value null at 'applicationId' failed to satisfy \
                    constraint: Member must not be null";
    assert_eq!(
        (empty.status, error_code(&empty).as_str()),
        (400, "ValidationException"),
        "case GetMigration(\"\"): expected the modelled validation error, got {} with {:?}",
        empty.status,
        empty.body
    );
    assert_eq!(
        message(&empty),
        expected,
        "case GetMigration(\"\"): expected {expected:?}, got {:?}",
        empty.body
    );

    // A WELL-FORMED id this account holds no application for is the other refusal
    // entirely: the application it names could not be resolved. The opaque half is the
    // service's to mint, so the rows differ in what it spells.
    for (id, application_id) in [
        ("abc-000000000000", "abc"),
        ("0123456789-9f8e7d6c", "0123456789"),
        ("notheldhere-a-b-c", "notheldhere"),
    ] {
        let answer = get_migration(&emulator, id).await;
        let expected = format!("Application {application_id} not found");
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case GetMigration({id:?}): expected the modelled not-found, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            message(&answer),
            expected,
            "case GetMigration({id:?}): expected {expected:?}, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn the_observability_reads_of_an_absent_domain_answer_the_generic_envelope() {
    let emulator = Emulator::new();

    // One row per way a well-formed name resolves to no domain: one nothing was ever
    // created under, and one whose domain was deleted.
    let deleted = "wording-deleted";
    created_domain(&emulator, deleted).await;
    let dropped = emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/domain/{deleted}"),
            None,
        )
        .await;
    assert_eq!(
        dropped.status, 200,
        "case DeleteDomain({deleted:?}): expected 200, got {} with {:?}",
        dropped.status, dropped.body
    );

    for name in ["wording-absent", deleted] {
        let not_found = format!("Domain not found: {name}");
        // (operation, the request, the status, the code, the sentence). The reads that
        // answer the generic envelope, and the scheduled-action listing beside them
        // that answers its modelled not-found.
        let rows: Vec<(&str, String, u16, &str, &str)> = vec![
            (
                "DescribeDomainHealth",
                format!("{PREFIX}/opensearch/domain/{name}/health"),
                400,
                "BaseException",
                not_found.as_str(),
            ),
            (
                "DescribeDomainNodes",
                format!("{PREFIX}/opensearch/domain/{name}/nodes"),
                400,
                "BaseException",
                not_found.as_str(),
            ),
            (
                "ListDomainMaintenances",
                format!("{PREFIX}/opensearch/domain/{name}/domainMaintenances"),
                400,
                "BaseException",
                not_found.as_str(),
            ),
            (
                // The sibling READ of ONE action: `MaintenanceId` is `@required`, so the
                // row names one — the domain is what resolves to nothing, and that is
                // what the answer is about.
                "GetDomainMaintenanceStatus",
                format!(
                    "{PREFIX}/opensearch/domain/{name}/domainMaintenance\
                     ?maintenanceId=00000000-0000-0000-0000-000000000000"
                ),
                400,
                "BaseException",
                not_found.as_str(),
            ),
            (
                "DescribeDomainChangeProgress",
                format!("{PREFIX}/opensearch/domain/{name}/progress"),
                400,
                "BaseException",
                NO_PROGRESS_INFORMATION,
            ),
            (
                "ListScheduledActions",
                format!("{PREFIX}/opensearch/domain/{name}/scheduledActions"),
                RECORDED_NOT_FOUND_STATUS,
                "ResourceNotFoundException",
                not_found.as_str(),
            ),
        ];
        for (operation, uri, status, code, sentence) in rows {
            let answer = emulator.call("GET", &uri, None).await;
            let case = format!("{operation} for the absent domain {name:?}");
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (status, code),
                "case {case}: expected {status} {code}, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                message(&answer),
                sentence,
                "case {case}: expected {sentence:?}, got {:?}",
                answer.body
            );
        }
    }
}
