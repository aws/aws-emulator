// SPDX-License-Identifier: CC0-1.0

//! How this service frames an answer that carries NO CONTENT.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch`, with a real restJson1 request, and reads the RAW
//! response — its status, its header set and its body bytes — rather than the parsed
//! result, because the rule under test is about framing and is invisible to a client that
//! only inspects the parsed body.
//!
//! One rule is under test: a response that carries no content carries no body at all and
//! STATES the length it has (`content-length: 0`), rather than leaving the framing header
//! off; a response that carries content is left exactly as its renderer wrote it, framed
//! from the body it carries.
//!
//! The table spans what the rule generalises over: EVERY operation of this service whose
//! modelled output is empty, whose success is the no-content status and that is served —
//! `CancelJob`, all five deletes and the two tag mutations — whatever the resource it acts
//! on, the method it is bound to (`DELETE` and `POST`), and whether the request carried a
//! body of its own. The repeated-call rows pin that the framing is a property of the
//! answer and not of the first call. The contrasting table is the other half of the rule:
//! the creates, the readers and the modelled refusals, which all carry content.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::data_grants::create_data_grant;
use crate::data_sets::{call, create_data_set, harness, REGION};
use crate::event_actions::entitle_data_set;
use crate::jobs::create_job;
use crate::resource_tags::percent_encode;
use crate::revision_finalization::import_assets;
use crate::revisions::create_revision;
use crate::signed_url_uploads::base64_md5;

/// The length an answer that carries no content states for the body it does not carry.
const NO_CONTENT_LENGTH: &str = "0";

/// The status such an answer is served at: every operation of this service with an empty
/// (`smithy.api#Unit`) output answers it (`smithy_query(query="operation",
/// operation="CancelJob")` reports `http { code: 204 }`).
const NO_CONTENT_STATUS: u16 = 204;

/// The `Type` of an import whose content a client uploads to an announced URL, used to put
/// an asset in a revision so there is one to delete.
const SIGNED_URL_TYPE: &str = "IMPORT_ASSET_FROM_SIGNED_URL";

/// One RAW answer, reduced to the three things the framing assertions read.
struct Reply {
    status: u16,
    /// The `content-length` the answer states, if any.
    content_length: Option<String>,
    body: Vec<u8>,
}

/// One row of either table: how the case is named and the request that produces it.
struct Case {
    label: String,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Send one restJson1 request and keep the RAW answer: the status, the framing header and
/// the body bytes. The suites beside this one reduce an answer to its parsed body, which is
/// exactly what cannot see this rule.
async fn send(router: &MultiServiceRouter, case: &Case) -> Reply {
    let host = format!("dataexchange.{REGION}.amazonaws.com");
    let mut request = http::Request::builder()
        .method(case.method)
        .uri(format!("https://{host}{}", case.uri))
        .header("host", &host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=TESTKEY/20240101/{REGION}/dataexchange/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        );
    let payload = match &case.body {
        Some(value) => {
            request = request.header("content-type", "application/json");
            Bytes::from(serde_json::to_vec(value).expect("serializable request body"))
        }
        None => Bytes::new(),
    };
    let response = router
        .dispatch(request.body(payload).expect("a well-formed test request"))
        .await;
    let status = response.status().as_u16();
    let content_length = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    Reply {
        status,
        content_length,
        body: response.into_body().to_vec(),
    }
}

/// Assert that `reply` is framed the way an answer that carries no content is: the
/// no-content status, no body byte at all, and the length it has stated explicitly.
fn assert_no_content(label: &str, reply: &Reply) {
    assert_eq!(
        reply.status, NO_CONTENT_STATUS,
        "case {label:?}: expected status {NO_CONTENT_STATUS}, got {} body {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert!(
        reply.body.is_empty(),
        "case {label:?}: expected no body on a no-content answer, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.content_length.as_deref(),
        Some(NO_CONTENT_LENGTH),
        "case {label:?}: expected the answer to state content-length {NO_CONTENT_LENGTH}, got {:?}",
        reply.content_length
    );
}

/// A data set holding one revision, for a case that acts on either.
async fn data_set_with_revision(
    router: &MultiServiceRouter,
    name: &str,
    asset_type: &str,
) -> (String, String) {
    let data_set = create_data_set(router, name, "no-content framing", asset_type, None).await;
    assert_eq!(
        data_set.status, 201,
        "case {name:?}: expected 201 from CreateDataSet, got {} body {:?}",
        data_set.status, data_set.body
    );
    let data_set_id = data_set.string("Id");
    let revision = create_revision(router, &data_set_id, Some("no-content framing"), None, name)
        .await
        .string("Id");
    (data_set_id, revision)
}

/// One row per operation of this service whose modelled output is empty, each against a
/// resource created for it, so the table covers the whole class rather than one recorded
/// call.
///
/// Source for the class: the operations whose `output_shape` is `Unit` and whose
/// `http { code }` is the no-content status — `smithy_query(query="operation",
/// operation="CancelJob,DeleteAsset,DeleteDataGrant,DeleteDataSet,DeleteEventAction,DeleteRevision,TagResource,UntagResource")`.
async fn no_content_cases(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
) -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();

    // CancelJob: a job that has not been started, which is what a cancel is admitted on.
    let (job_data_set, job_revision) = data_set_with_revision(router, "cancel", "S3_SNAPSHOT").await;
    let job = create_job(
        router,
        SIGNED_URL_TYPE,
        json!({
            "ImportAssetFromSignedUrl": {
                "AssetName": "orders.csv",
                "DataSetId": job_data_set,
                "Md5Hash": base64_md5(b"id,name\n1,ada\n"),
                "RevisionId": job_revision,
            }
        }),
        None,
    )
    .await;
    assert_eq!(
        job.status, 201,
        "case \"CancelJob\": expected 201 from CreateJob, got {} body {:?}",
        job.status, job.body
    );
    cases.push(Case {
        label: "CancelJob".to_string(),
        method: "DELETE",
        uri: format!("/v1/jobs/{}", job.string("Id")),
        body: None,
    });

    // DeleteRevision: a revision of its own data set, so the delete of the data set below
    // is not what removed it.
    let (revision_data_set, revision_id) =
        data_set_with_revision(router, "delete-revision", "S3_SNAPSHOT").await;
    cases.push(Case {
        label: "DeleteRevision".to_string(),
        method: "DELETE",
        uri: format!("/v1/data-sets/{revision_data_set}/revisions/{revision_id}"),
        body: None,
    });

    // DeleteAsset: an asset of a revision of its own data set. An asset only ever arrives
    // in a revision through an import job, so the record is written through the shared
    // engine the way `revision_finalization.rs` does, and its id is read back through the
    // list reader.
    let (asset_data_set, asset_revision) =
        data_set_with_revision(router, "delete-asset", "S3_SNAPSHOT").await;
    import_assets(engine, &asset_data_set, &asset_revision, 1);
    let listed = call(
        router,
        "GET",
        &format!("/v1/data-sets/{asset_data_set}/revisions/{asset_revision}/assets"),
        None,
    )
    .await;
    let asset_id = listed.member("Assets")[0]["Id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case \"DeleteAsset\": expected the imported asset to be listed, got {:?}",
                listed.body
            )
        })
        .to_string();
    cases.push(Case {
        label: "DeleteAsset".to_string(),
        method: "DELETE",
        uri: format!(
            "/v1/data-sets/{asset_data_set}/revisions/{asset_revision}/assets/{asset_id}"
        ),
        body: None,
    });

    // DeleteDataSet.
    let doomed = create_data_set(router, "delete-data-set", "framing", "S3_SNAPSHOT", None).await;
    assert_eq!(
        doomed.status, 201,
        "case \"DeleteDataSet\": expected 201 from CreateDataSet, got {} body {:?}",
        doomed.status, doomed.body
    );
    cases.push(Case {
        label: "DeleteDataSet".to_string(),
        method: "DELETE",
        uri: format!("/v1/data-sets/{}", doomed.string("Id")),
        body: None,
    });

    // DeleteEventAction: an event action exports a revision of an entitled data set, so the
    // data set it names is entitled before the action is created.
    let entitled =
        create_data_set(router, "delete-event-action", "framing", "S3_SNAPSHOT", None).await;
    assert_eq!(
        entitled.status, 201,
        "case \"DeleteEventAction\": expected 201 from CreateDataSet, got {} body {:?}",
        entitled.status, entitled.body
    );
    let entitled_id = entitled.string("Id");
    entitle_data_set(engine, &entitled_id);
    let event_action = call(
        router,
        "POST",
        "/v1/event-actions",
        Some(json!({
            "Event": { "RevisionPublished": { "DataSetId": entitled_id } },
            "Action": {
                "ExportRevisionToS3": { "RevisionDestination": { "Bucket": "export-bucket" } }
            },
        })),
    )
    .await;
    assert_eq!(
        event_action.status, 201,
        "case \"DeleteEventAction\": expected 201 from CreateEventAction, got {} body {:?}",
        event_action.status, event_action.body
    );
    cases.push(Case {
        label: "DeleteEventAction".to_string(),
        method: "DELETE",
        uri: format!("/v1/event-actions/{}", event_action.string("Id")),
        body: None,
    });

    // DeleteDataGrant.
    let granted = create_data_set(router, "delete-data-grant", "framing", "S3_SNAPSHOT", None).await;
    assert_eq!(
        granted.status, 201,
        "case \"DeleteDataGrant\": expected 201 from CreateDataSet, got {} body {:?}",
        granted.status, granted.body
    );
    let grant = create_data_grant(
        router,
        json!({
            "Name": "grant for the framing table",
            "GrantDistributionScope": "NONE",
            "ReceiverPrincipal": "111122223333",
            "SourceDataSetId": granted.string("Id"),
        }),
    )
    .await;
    assert_eq!(
        grant.status, 201,
        "case \"DeleteDataGrant\": expected 201 from CreateDataGrant, got {} body {:?}",
        grant.status, grant.body
    );
    cases.push(Case {
        label: "DeleteDataGrant".to_string(),
        method: "DELETE",
        uri: format!("/v1/data-grants/{}", grant.string("Id")),
        body: None,
    });

    // The two tag mutations, on the two resource kinds this service tags: the write carries
    // a request body of its own and is bound to POST, the removal carries its keys in the
    // query string — so the table covers both methods and both request shapes.
    let tagged_set = create_data_set(router, "tagged-set", "framing", "S3_SNAPSHOT", None).await;
    assert_eq!(
        tagged_set.status, 201,
        "case \"TagResource\": expected 201 from CreateDataSet, got {} body {:?}",
        tagged_set.status, tagged_set.body
    );
    cases.push(Case {
        label: "TagResource".to_string(),
        method: "POST",
        uri: format!("/tags/{}", percent_encode(&tagged_set.string("Arn"))),
        body: Some(json!({ "tags": { "team": "data" } })),
    });

    let (untagged_set, _) = data_set_with_revision(router, "tagged-revision", "S3_SNAPSHOT").await;
    let tagged_revision = create_revision(
        router,
        &untagged_set,
        Some("framing"),
        Some(&json!({ "team": "data", "tier": "gold" })),
        "UntagResource",
    )
    .await;
    cases.push(Case {
        label: "UntagResource".to_string(),
        method: "DELETE",
        uri: format!(
            "/tags/{}?tagKeys=team&tagKeys=tier",
            percent_encode(&tagged_revision.string("Arn"))
        ),
        body: None,
    });

    cases
}

/// RULE: every success of this service whose modelled output is empty answers the
/// no-content status with no body at all and STATES the length it has
/// (`content-length: 0`), rather than leaving the framing header off.
#[tokio::test]
async fn every_answer_that_carries_no_content_states_the_length_it_has() {
    let (router, engine) = harness();

    for case in no_content_cases(&router, &engine).await {
        let reply = send(&router, &case).await;
        assert_no_content(&case.label, &reply);
    }
}

/// RULE: the framing is a property of the answer, not of the first call — a repeat of a
/// no-content call that is still admitted is framed the same way.
///
/// `UntagResource` is `@idempotent` (`smithy_query(query="operation",
/// operation="UntagResource")`), so removing keys that are already gone is admitted again
/// and again; a repeat of `TagResource` writes the same map.
#[tokio::test]
async fn a_repeated_no_content_call_is_framed_the_same_way() {
    let (router, _engine) = harness();
    let created = create_data_set(&router, "repeat", "framing", "S3_SNAPSHOT", None).await;
    assert_eq!(
        created.status, 201,
        "case \"repeat\": expected 201 from CreateDataSet, got {} body {:?}",
        created.status, created.body
    );
    let arn = percent_encode(&created.string("Arn"));

    let rows = [
        Case {
            label: "TagResource".to_string(),
            method: "POST",
            uri: format!("/tags/{arn}"),
            body: Some(json!({ "tags": { "team": "data" } })),
        },
        Case {
            label: "UntagResource".to_string(),
            method: "DELETE",
            uri: format!("/tags/{arn}?tagKeys=team"),
            body: None,
        },
    ];

    for round in 1..=3 {
        for case in &rows {
            let reply = send(&router, case).await;
            assert_no_content(&format!("{} (round {round})", case.label), &reply);
        }
    }
}

/// RULE, the other half: an answer that CARRIES content is left exactly as the model's
/// protocol renders it — it keeps its body and does not state the no-content length, which
/// the transport frames from the body it carries.
///
/// The rows span the answers of this service that carry content: a create, the two kinds of
/// reader (one resource and a page of them), and the modelled refusals at a 4xx.
#[tokio::test]
async fn an_answer_that_carries_content_keeps_its_body_and_its_own_framing() {
    let (router, _engine) = harness();
    let created = create_data_set(&router, "with-content", "framing", "S3_SNAPSHOT", None).await;
    assert_eq!(
        created.status, 201,
        "case \"CreateDataSet\": expected 201 from CreateDataSet, got {} body {:?}",
        created.status, created.body
    );
    let data_set_id = created.string("Id");
    // A well-formed `Id` (`^[a-zA-Z0-9]{30,40}$`) naming nothing this emulator holds, so the
    // refusal rows are about the resource rather than about the request.
    let absent = "0123456789abcdef0123456789abcdef";

    let rows = [
        (
            201,
            Case {
                label: "CreateDataSet".to_string(),
                method: "POST",
                uri: "/v1/data-sets".to_string(),
                body: Some(json!({
                    "AssetType": "S3_SNAPSHOT",
                    "Description": "framing",
                    "Name": "another-set",
                })),
            },
        ),
        (
            200,
            Case {
                label: "GetDataSet".to_string(),
                method: "GET",
                uri: format!("/v1/data-sets/{data_set_id}"),
                body: None,
            },
        ),
        (
            200,
            Case {
                label: "ListDataSets".to_string(),
                method: "GET",
                uri: "/v1/data-sets".to_string(),
                body: None,
            },
        ),
        (
            404,
            Case {
                label: "GetJob on an absent job".to_string(),
                method: "GET",
                uri: format!("/v1/jobs/{absent}"),
                body: None,
            },
        ),
        (
            400,
            Case {
                label: "CreateDataSet with no AssetType".to_string(),
                method: "POST",
                uri: "/v1/data-sets".to_string(),
                body: Some(json!({ "Description": "framing", "Name": "no-asset-type" })),
            },
        ),
    ];

    for (expected_status, case) in &rows {
        let reply = send(&router, case).await;
        assert_eq!(
            reply.status, *expected_status,
            "case {:?}: expected status {expected_status}, got {} body {:?}",
            case.label,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            !reply.body.is_empty(),
            "case {:?}: expected the answer to carry its rendered body, got none",
            case.label
        );
        assert_ne!(
            reply.content_length.as_deref(),
            Some(NO_CONTENT_LENGTH),
            "case {:?}: an answer that carries content must not state content-length \
             {NO_CONTENT_LENGTH}, got {:?}",
            case.label,
            reply.content_length
        );
    }
}
