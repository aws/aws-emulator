// SPDX-License-Identifier: CC0-1.0

//! A scheduled schema merge REACHES the client that polls for it, end to end over HTTP.
//!
//! RULE: creating a source-API association schedules the merge of the source API's schema
//! into the Merged API, and the service performs that merge by itself: the association
//! leaves the non-terminal `MERGE_SCHEDULED` the create answered for a TERMINAL
//! `SourceApiAssociationStatus` with no call but the reads a client uses to watch it, and
//! it does so inside the run of reads such a client makes — a client reads the association
//! again as fast as the calls themselves go, so a merge that is still only scheduled when
//! it stops asking is indistinguishable from one that is never performed. The terminal
//! status is `MERGE_SUCCESS` for a source API whose schema the account holds, and what the
//! merge left behind is the source API's types ON the Merged API.
//!
//! The association the merge settled is then addressable as the recorded flow addresses
//! it: `DisassociateMergedGraphqlApi` scoped to a DIFFERENT source API names no
//! association of that API, so it is refused 404 `NotFoundException` with AppSync's fixed
//! sentence and leaves the association readable, while the correctly scoped call is
//! accepted.
//!
//! The rows differ along what the rule generalises over: which side of the pair created
//! the association (`AssociateMergedGraphqlApi` addresses the source API,
//! `AssociateSourceGraphqlApi` the Merged API) and how many types the source API's schema
//! declares. Nothing here waits for a fixed time: each row reads the association again
//! until it reports a terminal status, under a generous bound on the reads and on the
//! wall clock alike, so a fast host and a loaded one reach the same assertion.
//!
//! The harness (`router`, `call`, `create_api`, `get_path`, the schema helpers and the
//! `SourceApiAssociationStatus` catalog) is the one the association and schema suites
//! already carry.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::schema_creation::{await_creation, get_type, start_creation};
use crate::source_api_association::{
    call, create_api, get_path, router, Answer, ACCOUNT, TERMINAL_MERGE_STATUSES,
};

/// AppSync's fixed text for an `associationId` that names no association of the API the
/// request addressed: no identifier of the request is interpolated into it.
const NO_SUCH_ASSOCIATION: &str = "No source API association was found";

/// How many back-to-back reads a client is given to see the merge it is watching: far
/// more than the handful a client needs, so the bound cannot be reached by a host being
/// slow, only by a merge that is never performed.
const READS: usize = 5_000;

/// How long such a run of reads may take before the suite gives up, so a stalled emulator
/// fails the case instead of hanging it. Generous: what is asserted is that the merge is
/// performed, never how fast this host runs.
const DEADLINE: Duration = Duration::from_secs(60);

/// Which side of the pair the row's request addresses.
#[derive(Debug, Clone, Copy)]
enum Side {
    /// `AssociateMergedGraphqlApi`: the path names the source API, the body the Merged
    /// API.
    FromTheSourceApi,
    /// `AssociateSourceGraphqlApi`: the path names the Merged API, the body the source
    /// API.
    FromTheMergedApi,
}

/// One row: which side associated the pair, and the schema the source API holds.
#[derive(Debug)]
struct Case {
    label: &'static str,
    side: Side,
    /// The document the source API's schema is created from, and the types it declares —
    /// each of which the Merged API must hold once the merge has been performed.
    document: &'static str,
    types: &'static [&'static str],
}

/// The table: both association paths, over source schemas of different sizes.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "from the source api, one type",
            side: Side::FromTheSourceApi,
            document: "type Query {\n  one: String\n}\n",
            types: &["Query"],
        },
        Case {
            label: "from the source api, a root type and two of its own",
            side: Side::FromTheSourceApi,
            document: "schema { query: Query }\ntype Query {\n  order: Order\n}\ntype Order {\n  \
                       id: ID!\n  line: Line\n}\ntype Line {\n  sku: String\n}\n",
            types: &["Query", "Order", "Line"],
        },
        Case {
            label: "from the merged api, one type",
            side: Side::FromTheMergedApi,
            document: "type Query {\n  two: String\n}\n",
            types: &["Query"],
        },
        Case {
            label: "from the merged api, a root type and two of its own",
            side: Side::FromTheMergedApi,
            document: "schema { query: Query }\ntype Query {\n  invoice: Invoice\n}\ntype \
                       Invoice {\n  id: ID!\n  row: Row\n}\ntype Row {\n  sku: String\n}\n",
            types: &["Query", "Invoice", "Row"],
        },
    ]
}

/// The role a Merged API is created with: "the IAM role to be used by AppSync to merge
/// the source APIs" (`GraphqlApi$mergedApiExecutionRoleArn`). This account's own ARN,
/// built the way every emulated ARN is.
fn merge_execution_role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/appsync-merge-execution")
}

/// Create one Merged API with that role and answer its `apiId`.
async fn create_merged_api(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": name,
            "apiType": state::MERGED_API_TYPE,
            "authenticationType": "API_KEY",
            "mergedApiExecutionRoleArn": merge_execution_role(),
            "queryDepthLimit": 0,
            "resolverCountLimit": 0,
            "xrayEnabled": false,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case create the merged API {name:?}: expected 200, got {answer:?}"
    );
    answer.body["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The row's source API with its schema active, an unrelated API, a Merged API and the
/// association between the pair: the source API id, the unrelated API id, the Merged API
/// id and the `associationId`.
async fn fixture(router: &MultiServiceRouter, case: &Case) -> (String, String, String, String) {
    let source = create_api(router, &format!("{}-source", case.label), state::GRAPHQL_API_TYPE)
        .await;
    let unrelated =
        create_api(router, &format!("{}-unrelated", case.label), state::GRAPHQL_API_TYPE).await;
    let merged_api_id = create_merged_api(router, &format!("{}-merged", case.label)).await;

    let started = start_creation(router, &source.id, case.document.as_bytes()).await;
    assert_eq!(
        started.status, 200,
        "case {:?}: expected StartSchemaCreation 200, got {started:?}",
        case.label
    );
    await_creation(router, &source.id, case.label).await;

    let (path, body) = match case.side {
        Side::FromTheSourceApi => (
            format!("/v1/sourceApis/{}/mergedApiAssociations", source.id),
            json!({"mergedApiIdentifier": merged_api_id}),
        ),
        Side::FromTheMergedApi => (
            format!("/v1/mergedApis/{merged_api_id}/sourceApiAssociations"),
            json!({"sourceApiIdentifier": source.id}),
        ),
    };
    let created = call(router, "POST", &path, Some(body)).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected the association to be created, got {created:?}",
        case.label
    );
    let association = &created.body["sourceApiAssociation"];
    // The create's own answer is the one that reports the merge as merely scheduled.
    assert_eq!(
        association["sourceApiAssociationStatus"].as_str(),
        Some(state::MERGE_SCHEDULED),
        "case {:?}: expected the create to answer the non-terminal scheduled status, got \
         {association:?}",
        case.label
    );
    let association_id = association["associationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (source.id, unrelated.id, merged_api_id, association_id)
}

/// Read the association once, asserting the read itself was served.
async fn read(
    router: &MultiServiceRouter,
    case: &str,
    merged_api_id: &str,
    association_id: &str,
) -> Answer {
    let answer = call(
        router,
        "GET",
        &get_path(merged_api_id, association_id),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetSourceApiAssociation 200, got {answer:?}"
    );
    answer
}

/// Read the association back-to-back — as a client watching the merge does, with no wait
/// between two reads — until it reports a terminal status, and answer that record.
async fn merge_reaching_a_poller(
    router: &MultiServiceRouter,
    case: &str,
    merged_api_id: &str,
    association_id: &str,
) -> Value {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    for attempt in 1..=READS {
        let answer = read(router, case, merged_api_id, association_id).await;
        let association = answer.body["sourceApiAssociation"].clone();
        let status = association["sourceApiAssociationStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if TERMINAL_MERGE_STATUSES.contains(&status.as_str()) {
            return association;
        }
        assert!(
            attempt < READS && tokio::time::Instant::now() < deadline,
            "case {case:?}: expected the scheduled merge to have been performed by the time a \
             client watching the association stops reading it, got {status:?} after {attempt} \
             reads"
        );
    }
    unreachable!("the loop above returns or fails on its last attempt")
}

#[tokio::test]
async fn a_scheduled_merge_settles_while_a_client_is_still_reading_the_association() {
    let router = router();

    for case in cases() {
        let (source_api_id, _unrelated, merged_api_id, association_id) =
            fixture(&router, &case).await;

        let settled =
            merge_reaching_a_poller(&router, case.label, &merged_api_id, &association_id).await;
        assert_eq!(
            settled["sourceApiAssociationStatus"].as_str(),
            Some(state::MERGE_SUCCESS),
            "case {:?}: expected the merge of a schema this account holds to have succeeded, got \
             {settled:?}",
            case.label
        );
        assert!(
            settled["lastSuccessfulMergeDate"]
                .as_f64()
                .is_some_and(|epoch| epoch > 0.0),
            "case {:?}: expected the merge date the performed merge left, got {:?}",
            case.label,
            settled["lastSuccessfulMergeDate"]
        );

        // Reading the settled association leaves the next caller the same record: the
        // reads that watched the merge are reports, not steps of it.
        let again = read(&router, case.label, &merged_api_id, &association_id).await;
        assert_eq!(
            again.body["sourceApiAssociation"], settled,
            "case {:?}: expected the same record for the next caller, got {:?}",
            case.label, again.body["sourceApiAssociation"]
        );

        // The merge is BACKED by the Merged API's own schema: every type the source API
        // declares is readable on the merged api id, carrying the source's definition.
        for type_name in case.types {
            let on_the_source = get_type(&router, &source_api_id, type_name, state::SDL_FORMAT).await;
            let on_the_merged = get_type(&router, &merged_api_id, type_name, state::SDL_FORMAT).await;
            assert_eq!(
                on_the_merged.status, 200,
                "case {:?}, type {type_name:?}: expected the merged API to hold the source \
                 API's type, got {on_the_merged:?}",
                case.label
            );
            assert_eq!(
                on_the_merged.body["type"]["definition"],
                on_the_source.body["type"]["definition"],
                "case {:?}, type {type_name:?}: expected the source API's own definition, got \
                 {:?} against {:?}",
                case.label,
                on_the_merged.body["type"],
                on_the_source.body["type"]
            );
        }
    }
}

#[tokio::test]
async fn a_settled_association_is_disassociated_only_through_its_own_source_api() {
    let router = router();

    for case in cases() {
        let (source_api_id, unrelated, merged_api_id, association_id) =
            fixture(&router, &case).await;
        let settled =
            merge_reaching_a_poller(&router, case.label, &merged_api_id, &association_id).await;

        // The `associationId` is scoped to the source API that is a side of it: addressed
        // under another API it names nothing, so the call is refused and nothing is
        // scheduled.
        let refused = call(
            &router,
            "DELETE",
            &format!("/v1/sourceApis/{unrelated}/mergedApiAssociations/{association_id}"),
            None,
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {:?}: expected the wrongly scoped disassociate to be refused 404, got \
             {refused:?}",
            case.label
        );
        assert_eq!(
            refused.error_type, "NotFoundException",
            "case {:?}: expected NotFoundException, got {refused:?}",
            case.label
        );
        assert_eq!(
            refused.body["message"].as_str(),
            Some(NO_SUCH_ASSOCIATION),
            "case {:?}: expected AppSync's fixed text {NO_SUCH_ASSOCIATION:?}, got {:?}",
            case.label,
            refused.body["message"]
        );

        // The refusal left the association exactly as the merge did.
        let intact = read(&router, case.label, &merged_api_id, &association_id).await;
        assert_eq!(
            intact.body["sourceApiAssociation"], settled,
            "case {:?}: expected the refused disassociate to leave the association, got {:?}",
            case.label, intact.body["sourceApiAssociation"]
        );

        // Scoped to its OWN source API the same call is accepted, and it reports the
        // status the association is left in.
        let accepted = call(
            &router,
            "DELETE",
            &format!("/v1/sourceApis/{source_api_id}/mergedApiAssociations/{association_id}"),
            None,
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {:?}: expected the correctly scoped disassociate to be accepted, got \
             {accepted:?}",
            case.label
        );
        let status = accepted.body["sourceApiAssociationStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            status,
            state::DELETION_SCHEDULED,
            "case {:?}: expected the accepted disassociate to report the status it left the \
             association in ({:?}), got {status:?}",
            case.label,
            state::DELETION_SCHEDULED
        );
    }
}
