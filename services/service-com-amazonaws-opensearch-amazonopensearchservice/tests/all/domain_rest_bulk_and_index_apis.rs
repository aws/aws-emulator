// SPDX-License-Identifier: CC0-1.0

//! The domain REST data plane's BULK, ALIAS, SETTINGS, MAPPING, `_cat` and COMPONENT
//! TEMPLATE surfaces, driven over the real wire path: a signed request to the endpoint
//! `DescribeDomain` published, through the multi-service router, into the mounted service.
//! What is asserted is what a client sees.
//!
//! The rules, each over a SET of inputs rather than one recorded call:
//!
//! * a RESERVED first path segment (one beginning with `_`) names an API of the engine and
//!   never an index, so a reserved path the surface serves is answered by its handler and
//!   one it does not serve is answered as a path with no handler;
//! * the bulk API answers HTTP 200 whatever its items did, one item per action in the order
//!   the stream carried them, each with its own status and — where it failed — its own
//!   error, and every item that succeeded is backed by the document the index now holds;
//! * every write answers the `_seq_no`/`_primary_term` pair the document is now at, and a
//!   conditional write proceeds only while the document is still at the pair it names;
//! * an index's settings and mappings are read and written through the index's own record:
//!   a dynamic setting is merged and read back, a STATIC one is refused on an open index,
//!   and a mapping update may add a field but not re-type one;
//! * an alias is a virtual index name: a write addressed to one lands in the index it is
//!   the write index of, and a read addressed to one reads the indexes it points at;
//! * a component template is what an index template is composed of, and two index
//!   templates may not match one name at equal priority;
//! * the cluster health follows the replica copies the TOPOLOGY can hold: copies past them
//!   are unassigned and the cluster is yellow until the count fits.

use serde_json::{json, Value};

use super::data_source_registry::created_domain;
use super::domain_lifecycle::{Answer, Emulator, DEFAULT_ACCESS_KEY_ID};
use super::domain_rest_data_plane::{endpoint_of, error_type, rest, rest_payload};

/// The engine's error type for a request this cluster's own bounds or the request's own
/// members refuse — the one a refused bulk item and a refused settings update carry.
const ILLEGAL_ARGUMENT: &str = "illegal_argument_exception";

/// The `_primary_term` every document of this cluster reports: one run of a cluster takes
/// every write in one term.
const PRIMARY_TERM: i64 = 1;

/// A domain of `nodes` data nodes, created and ready to serve its data plane.
async fn created_cluster(emulator: &Emulator, name: &str, nodes: i64) -> String {
    let answer = emulator
        .create(json!({
            "DomainName": name,
            "EngineVersion": "OpenSearch_2.11",
            "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": nodes },
        }))
        .await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}, {nodes} nodes): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    endpoint_of(emulator, name)
}

/// `PUT /{index}` with the schema `body`, asserting the create was acknowledged.
pub(crate) async fn created_index(emulator: &Emulator, host: &str, index: &str, body: Value) {
    let answer = rest(
        emulator,
        DEFAULT_ACCESS_KEY_ID,
        host,
        "PUT",
        &format!("/{index}"),
        Some(body),
    )
    .await;
    assert_eq!(
        (answer.status, answer.body["acknowledged"].as_bool()),
        (200, Some(true)),
        "case \"PUT /{index}\": expected the create to be acknowledged, got {} with {:?}",
        answer.status,
        answer.body
    );
}

/// One request whose answer is NOT JSON — the aligned text a `_cat` request is answered
/// with — as the status and the bytes the client was served.
async fn rest_text(emulator: &Emulator, host: &str, method: &str, path: &str) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={DEFAULT_ACCESS_KEY_ID}/20260101/us-east-1/es/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::new())
        .expect("the test request is well formed");
    emulator.dispatch_bytes(request).await
}

/// One bulk request: the lines joined by the newline the stream is delimited by, and
/// terminated by one as the API documents.
pub(crate) async fn bulk(emulator: &Emulator, host: &str, path: &str, lines: &[Value]) -> Answer {
    let mut payload = String::new();
    for line in lines {
        payload.push_str(&line.to_string());
        payload.push('\n');
    }
    rest_payload(
        emulator,
        DEFAULT_ACCESS_KEY_ID,
        host,
        "POST",
        path,
        payload,
    )
    .await
}

/// The items a bulk answer carries, each as `(the action's name, its status, its result)` —
/// `result` is absent on an item that failed, which is what its error stands in for.
fn items(answer: &Answer) -> Vec<(String, i64, Option<String>, Option<String>)> {
    answer.body["items"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|item| {
            let (action, reported) = item.as_object()?.iter().next()?;
            Some((
                action.clone(),
                reported["status"].as_i64().unwrap_or_default(),
                reported["result"].as_str().map(str::to_owned),
                reported["error"]["type"].as_str().map(str::to_owned),
            ))
        })
        .collect()
}

/// How many documents an index holds, as its own count API reports it.
async fn counted(emulator: &Emulator, host: &str, index: &str) -> i64 {
    let answer = rest(
        emulator,
        DEFAULT_ACCESS_KEY_ID,
        host,
        "GET",
        &format!("/{index}/_count"),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case \"GET /{index}/_count\": expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["count"].as_i64().unwrap_or(-1)
}

/// One document as the index holds it, or `None` where it holds none of that id.
async fn document(emulator: &Emulator, host: &str, index: &str, id: &str) -> Option<Value> {
    let answer = rest(
        emulator,
        DEFAULT_ACCESS_KEY_ID,
        host,
        "GET",
        &format!("/{index}/_doc/{id}"),
        None,
    )
    .await;
    (answer.status == 200).then(|| answer.body.clone())
}

/// RULE: the bulk API answers HTTP 200 whatever its items did, one item per action in the
/// order the stream carried them, each with its own status and — where it failed — its own
/// error; every item that succeeded is backed by the document the index then holds.
///
/// The rows span the action vocabulary and both outcomes of each: an index that creates and
/// one that replaces, a create that conflicts, an update that merges, one of a document the
/// index does not hold, one that upserts it, a delete of a present and of an absent
/// document, and an item the index's own mappings refuse beside one they accept.
#[tokio::test]
async fn the_bulk_api_answers_one_item_per_action_out_of_the_documents_it_stored() {
    let emulator = Emulator::new();
    created_domain(&emulator, "bulk-target").await;
    let host = endpoint_of(&emulator, "bulk-target");
    created_index(
        &emulator,
        &host,
        "loaded",
        json!({
            "settings": { "number_of_shards": 1, "number_of_replicas": 0 },
            "mappings": { "dynamic": "strict", "properties": {
                "title": { "type": "keyword" },
                "count": { "type": "integer" },
            } },
        }),
    )
    .await;

    struct Row {
        case: &'static str,
        lines: Vec<Value>,
        /// (the action, the status, the result, the error type) each item must report.
        items: Vec<(&'static str, i64, Option<&'static str>, Option<&'static str>)>,
        errors: bool,
    }
    let rows: Vec<Row> = vec![
        Row {
            case: "two index actions for ids the index does not hold",
            lines: vec![
                json!({ "index": { "_index": "loaded", "_id": "a" } }),
                json!({ "title": "alpha", "count": 1 }),
                json!({ "index": { "_index": "loaded", "_id": "b" } }),
                json!({ "title": "beta", "count": 2 }),
            ],
            items: vec![
                ("index", 201, Some("created"), None),
                ("index", 201, Some("created"), None),
            ],
            errors: false,
        },
        Row {
            case: "an index action for an id the index already holds",
            lines: vec![
                json!({ "index": { "_index": "loaded", "_id": "a" } }),
                json!({ "title": "alpha again", "count": 1 }),
            ],
            items: vec![("index", 200, Some("updated"), None)],
            errors: false,
        },
        Row {
            case: "a create action for an id the index already holds",
            lines: vec![
                json!({ "create": { "_index": "loaded", "_id": "a" } }),
                json!({ "title": "alpha third", "count": 1 }),
            ],
            items: vec![(
                "create",
                409,
                None,
                Some("version_conflict_engine_exception"),
            )],
            errors: true,
        },
        Row {
            case: "an update action merging one field of a document the index holds",
            lines: vec![
                json!({ "update": { "_index": "loaded", "_id": "b" } }),
                json!({ "doc": { "count": 20 } }),
            ],
            items: vec![("update", 200, Some("updated"), None)],
            errors: false,
        },
        Row {
            case: "an update action for a document the index does not hold",
            lines: vec![
                json!({ "update": { "_index": "loaded", "_id": "absent" } }),
                json!({ "doc": { "count": 1 } }),
            ],
            items: vec![("update", 404, None, Some("document_missing_exception"))],
            errors: true,
        },
        Row {
            case: "the same update asking for the partial document to be upserted",
            lines: vec![
                json!({ "update": { "_index": "loaded", "_id": "upserted" } }),
                json!({ "doc": { "title": "gamma", "count": 3 }, "doc_as_upsert": true }),
            ],
            items: vec![("update", 201, Some("created"), None)],
            errors: false,
        },
        Row {
            case: "a delete of a document the index holds and one of a document it does not",
            lines: vec![
                json!({ "delete": { "_index": "loaded", "_id": "upserted" } }),
                json!({ "delete": { "_index": "loaded", "_id": "never-written" } }),
            ],
            items: vec![
                ("delete", 200, Some("deleted"), None),
                ("delete", 404, Some("not_found"), None),
            ],
            errors: false,
        },
        Row {
            case: "an item the index's own mappings refuse beside one they accept",
            lines: vec![
                json!({ "index": { "_index": "loaded", "_id": "c" } }),
                json!({ "title": "delta", "count": 4 }),
                json!({ "index": { "_index": "loaded", "_id": "d" } }),
                json!({ "title": "epsilon", "unmapped": "x" }),
            ],
            items: vec![
                ("index", 201, Some("created"), None),
                ("index", 400, None, Some("strict_dynamic_mapping_exception")),
            ],
            errors: true,
        },
    ];

    for row in &rows {
        let answered = bulk(&emulator, &host, "/_bulk", &row.lines).await;
        assert_eq!(
            answered.status, 200,
            "case {:?}: a bulk request is answered 200 whatever its items did, got {} with \
             {:?}",
            row.case, answered.status, answered.body
        );
        assert_eq!(
            answered.body["errors"].as_bool(),
            Some(row.errors),
            "case {:?}: expected errors {}, got {:?}",
            row.case,
            row.errors,
            answered.body
        );
        let expected: Vec<(String, i64, Option<String>, Option<String>)> = row
            .items
            .iter()
            .map(|(action, status, result, error)| {
                (
                    (*action).to_string(),
                    *status,
                    result.map(str::to_owned),
                    error.map(str::to_owned),
                )
            })
            .collect();
        assert_eq!(
            items(&answered),
            expected,
            "case {:?}: expected the items the stream's actions earn, got {:?}",
            row.case,
            answered.body
        );
        assert!(
            answered.body["took"].as_i64().is_some(),
            "case {:?}: expected the time the request took, got {:?}",
            row.case,
            answered.body
        );
    }

    // Every item that reported a success is backed by what the index holds: the four
    // documents the rows above left, with the merged field and the replaced value.
    assert_eq!(
        counted(&emulator, &host, "loaded").await,
        3,
        "case \"the documents the stream left\": expected the three the items created minus \
         the one they deleted"
    );
    let merged = document(&emulator, &host, "loaded", "b")
        .await
        .expect("the index holds the updated document");
    assert_eq!(
        merged["_source"],
        json!({ "title": "beta", "count": 20 }),
        "case \"an update merges one field\": expected the other field to stay, got {merged:?}"
    );
    let replaced = document(&emulator, &host, "loaded", "a")
        .await
        .expect("the index holds the replaced document");
    assert_eq!(
        (
            replaced["_source"]["title"].as_str(),
            replaced["_version"].as_i64()
        ),
        (Some("alpha again"), Some(2)),
        "case \"an index action replaces\": expected the second write's document at version \
         2, got {replaced:?}"
    );
    assert!(
        document(&emulator, &host, "loaded", "d").await.is_none(),
        "case \"an item the mappings refuse\": expected no document for the refused item"
    );

    // The index-scoped form takes the index from the PATH for an action that names none.
    let scoped = bulk(
        &emulator,
        &host,
        "/loaded/_bulk",
        &[
            json!({ "index": { "_id": "scoped" } }),
            json!({ "title": "zeta", "count": 9 }),
        ],
    )
    .await;
    assert_eq!(
        (scoped.status, scoped.body["errors"].as_bool()),
        (200, Some(false)),
        "case \"POST /{{index}}/_bulk\": expected the path's index to be used, got {} with \
         {:?}",
        scoped.status,
        scoped.body
    );
    assert_eq!(
        scoped.body["items"][0]["index"]["_index"].as_str(),
        Some("loaded"),
        "case \"POST /{{index}}/_bulk\": expected the item to name the path's index, got {:?}",
        scoped.body
    );

    // A stream this surface cannot read as the action/source grammar at all is the whole
    // request's own failure: there is no item to report it against.
    struct Malformed {
        case: &'static str,
        payload: &'static str,
    }
    let malformed: &[Malformed] = &[
        Malformed {
            case: "an action line that is not a JSON object",
            payload: "\"index\"\n{}\n",
        },
        Malformed {
            case: "an action nothing is registered for",
            payload: "{\"upsert\":{\"_index\":\"loaded\",\"_id\":\"x\"}}\n{}\n",
        },
        Malformed {
            case: "an action carrying no source line",
            payload: "{\"index\":{\"_index\":\"loaded\",\"_id\":\"x\"}}\n",
        },
        Malformed {
            case: "an action naming no index at all",
            payload: "{\"index\":{\"_id\":\"x\"}}\n{\"title\":\"a\"}\n",
        },
        Malformed {
            case: "a body carrying no action",
            payload: "\n",
        },
    ];
    for row in malformed {
        let answered = rest_payload(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "POST",
            "/_bulk",
            row.payload.to_string(),
        )
        .await;
        assert_eq!(
            (answered.status, error_type(&answered)),
            (400, ILLEGAL_ARGUMENT.to_string()),
            "case {:?}: expected the whole request to be refused, got {} with {:?}",
            row.case,
            answered.status,
            answered.body
        );
    }
}

/// RULE: every write answers the `_seq_no`/`_primary_term` pair the document is now at, and
/// a write naming that pair as `if_seq_no`/`if_primary_term` proceeds only while the
/// document is still at it — so exactly one of two writers that read the same revision
/// wins.
///
/// The rows span what the rule generalises over: the first write of a document and a later
/// one, the pair read back through the document API, a conditional write naming the current
/// pair, one naming a revision the document has moved past, one naming a revision of a
/// document that does not exist, and a conditional delete.
#[tokio::test]
async fn a_write_reports_the_revision_a_conditional_write_is_taken_against() {
    let emulator = Emulator::new();
    created_domain(&emulator, "revisions").await;
    let host = endpoint_of(&emulator, "revisions");
    created_index(&emulator, &host, "counters", json!({})).await;

    // Each write of ONE index takes the next sequence number, counted from 0.
    let first = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/counters/_doc/counter",
        Some(json!({ "n": 1 })),
    )
    .await;
    assert_eq!(
        (
            first.status,
            first.body["_seq_no"].as_i64(),
            first.body["_primary_term"].as_i64(),
            first.body["_version"].as_i64(),
        ),
        (201, Some(0), Some(PRIMARY_TERM), Some(1)),
        "case \"the first write of a document\": expected the first revision of the index, \
         got {} with {:?}",
        first.status,
        first.body
    );
    let read = document(&emulator, &host, "counters", "counter")
        .await
        .expect("the index holds the written document");
    assert_eq!(
        (read["_seq_no"].as_i64(), read["_primary_term"].as_i64()),
        (Some(0), Some(PRIMARY_TERM)),
        "case \"a read of the document\": expected the same pair the write reported, got \
         {read:?}"
    );

    // (the case, the query the write carries, the status, the sequence number it leaves)
    struct Row {
        case: &'static str,
        path: &'static str,
        status: u16,
        seq_no: Option<i64>,
        error: Option<&'static str>,
    }
    let rows: &[Row] = &[
        Row {
            case: "a conditional write naming the revision the document is at",
            path: "/counters/_doc/counter?if_seq_no=0&if_primary_term=1",
            status: 200,
            seq_no: Some(1),
            error: None,
        },
        Row {
            case: "a conditional write naming a revision the document has moved past",
            path: "/counters/_doc/counter?if_seq_no=0&if_primary_term=1",
            status: 409,
            seq_no: None,
            error: Some("version_conflict_engine_exception"),
        },
        Row {
            case: "a conditional write of a document the index does not hold",
            path: "/counters/_doc/absent?if_seq_no=7&if_primary_term=1",
            status: 409,
            seq_no: None,
            error: Some("version_conflict_engine_exception"),
        },
        Row {
            case: "an unconditional write, which takes the next number",
            path: "/counters/_doc/counter",
            status: 200,
            seq_no: Some(2),
            error: None,
        },
    ];
    for row in rows {
        let answered = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "PUT",
            row.path,
            Some(json!({ "n": 2 })),
        )
        .await;
        assert_eq!(
            answered.status, row.status,
            "case {:?}: expected {}, got {} with {:?}",
            row.case, row.status, answered.status, answered.body
        );
        assert_eq!(
            answered.body["_seq_no"].as_i64(),
            row.seq_no,
            "case {:?}: expected sequence number {:?}, got {:?}",
            row.case,
            row.seq_no,
            answered.body
        );
        if let Some(error) = row.error {
            assert_eq!(
                error_type(&answered),
                error,
                "case {:?}: expected {error}, got {:?}",
                row.case,
                answered.body
            );
        }
    }

    // The refusal states both revisions, which is what tells a client its read was stale.
    let stale = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/counters/_doc/counter?if_seq_no=0&if_primary_term=1",
        Some(json!({ "n": 3 })),
    )
    .await;
    assert_eq!(
        stale.body["error"]["reason"].as_str(),
        Some(
            "[counter]: version conflict, required seqNo [0], primary term [1]. current \
             document has seqNo [2] and primary term [1]"
        ),
        "case \"a stale conditional write\": expected the engine's own sentence, got {:?}",
        stale.body
    );

    // A delete is a write like any other: it is held to the same condition and takes a
    // sequence number of its own.
    let refused = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "DELETE",
        "/counters/_doc/counter?if_seq_no=0&if_primary_term=1",
        None,
    )
    .await;
    assert_eq!(
        (refused.status, error_type(&refused)),
        (409, "version_conflict_engine_exception".to_string()),
        "case \"a conditional delete naming a stale revision\": got {} with {:?}",
        refused.status,
        refused.body
    );
    let deleted = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "DELETE",
        "/counters/_doc/counter?if_seq_no=2&if_primary_term=1",
        None,
    )
    .await;
    assert_eq!(
        (deleted.status, deleted.body["_seq_no"].as_i64()),
        (200, Some(3)),
        "case \"a conditional delete naming the current revision\": got {} with {:?}",
        deleted.status,
        deleted.body
    );
}

/// RULE: an index's settings and mappings are read out of and written into the index's own
/// record — a dynamic setting is merged and read back as text, a STATIC one is refused on an
/// open index, and a mapping update may add a field but may not re-type one.
#[tokio::test]
async fn an_index_reports_and_updates_its_own_settings_and_mappings() {
    let emulator = Emulator::new();
    created_domain(&emulator, "configured").await;
    let host = endpoint_of(&emulator, "configured");
    created_index(
        &emulator,
        &host,
        "shaped",
        json!({
            "settings": { "number_of_shards": 2, "number_of_replicas": 1 },
            "mappings": { "properties": { "title": { "type": "text" } } },
        }),
    )
    .await;

    // Every setting is reported as TEXT, whatever the create wrote it as, with the
    // index's own name and the id the cluster minted beside them.
    let settings = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/shaped/_settings",
        None,
    )
    .await;
    assert_eq!(
        (
            settings.status,
            settings.body["shaped"]["settings"]["index"]["number_of_shards"].as_str(),
            settings.body["shaped"]["settings"]["index"]["number_of_replicas"].as_str(),
            settings.body["shaped"]["settings"]["index"]["provided_name"].as_str(),
        ),
        (200, Some("2"), Some("1"), Some("shaped")),
        "case \"GET /shaped/_settings\": expected the settings as text, got {} with {:?}",
        settings.status,
        settings.body
    );

    // (the case, the update, whether it is taken, the name and value to read back)
    struct Row {
        case: &'static str,
        update: Value,
        accepted: bool,
        read_back: Option<(&'static str, &'static str)>,
    }
    let rows: Vec<Row> = vec![
        Row {
            case: "the refresh interval turned off",
            update: json!({ "index": { "refresh_interval": "-1" } }),
            accepted: true,
            read_back: Some(("refresh_interval", "-1")),
        },
        Row {
            case: "the refresh interval set to a span",
            update: json!({ "index": { "refresh_interval": "30s" } }),
            accepted: true,
            read_back: Some(("refresh_interval", "30s")),
        },
        Row {
            case: "a replica count the topology can hold",
            update: json!({ "index": { "number_of_replicas": 0 } }),
            accepted: true,
            read_back: Some(("number_of_replicas", "0")),
        },
        Row {
            case: "a setting written under its flat dotted name",
            update: json!({ "index.max_result_window": 5000 }),
            accepted: true,
            read_back: Some(("max_result_window", "5000")),
        },
        Row {
            case: "a nested dynamic setting, as a slow-log threshold is",
            update: json!({ "index": { "search": { "slowlog": { "threshold": {
                "query": { "warn": "2s" } } } } } }),
            accepted: true,
            read_back: None,
        },
        Row {
            case: "the shard count, which is settled when the index is created",
            update: json!({ "index": { "number_of_shards": 4 } }),
            accepted: false,
            read_back: None,
        },
        Row {
            case: "the analysis chain, which an open index may not change",
            update: json!({ "index": { "analysis": { "analyzer": { "a": { "type": "custom",
                "tokenizer": "standard" } } } } }),
            accepted: false,
            read_back: None,
        },
    ];
    for row in &rows {
        let answered = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "PUT",
            "/shaped/_settings",
            Some(row.update.clone()),
        )
        .await;
        if row.accepted {
            assert_eq!(
                (answered.status, answered.body["acknowledged"].as_bool()),
                (200, Some(true)),
                "case {:?}: expected the update to be acknowledged, got {} with {:?}",
                row.case,
                answered.status,
                answered.body
            );
        } else {
            assert_eq!(
                (answered.status, error_type(&answered)),
                (400, ILLEGAL_ARGUMENT.to_string()),
                "case {:?}: expected a static setting to be refused, got {} with {:?}",
                row.case,
                answered.status,
                answered.body
            );
            assert!(
                answered.body["error"]["reason"]
                    .as_str()
                    .is_some_and(|reason| reason
                        .starts_with("Can't update non dynamic settings [[index.")),
                "case {:?}: expected the engine's own sentence, got {:?}",
                row.case,
                answered.body
            );
        }
        if let Some((name, value)) = row.read_back {
            let read = rest(
                &emulator,
                DEFAULT_ACCESS_KEY_ID,
                &host,
                "GET",
                "/shaped/_settings",
                None,
            )
            .await;
            assert_eq!(
                read.body["shaped"]["settings"]["index"][name].as_str(),
                Some(value),
                "case {:?}: expected {name} to read back as {value:?}, got {:?}",
                row.case,
                read.body
            );
        }
    }
    // A nested setting is reported under the objects its name names.
    let nested = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/shaped/_settings",
        None,
    )
    .await;
    assert_eq!(
        nested.body["shaped"]["settings"]["index"]["search"]["slowlog"]["threshold"]["query"]
            ["warn"]
            .as_str(),
        Some("2s"),
        "case \"a nested dynamic setting\": expected it under its own objects, got {:?}",
        nested.body
    );
    // The shard count the refused update did not change.
    assert_eq!(
        nested.body["shaped"]["settings"]["index"]["number_of_shards"].as_str(),
        Some("2"),
        "case \"a refused static update\": expected the index to keep its shard count, got \
         {:?}",
        nested.body
    );

    // A read may narrow the report to the setting names it asks for.
    let filtered = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/shaped/_settings/index.number_of_*",
        None,
    )
    .await;
    assert_eq!(
        (
            filtered.status,
            filtered.body["shaped"]["settings"]["index"]["number_of_shards"].as_str(),
            filtered.body["shaped"]["settings"]["index"]["refresh_interval"].as_str(),
        ),
        (200, Some("2"), None),
        "case \"GET /shaped/_settings/index.number_of_*\": expected only the named settings, \
         got {} with {:?}",
        filtered.status,
        filtered.body
    );

    // The FLAT form reports each setting under the dotted name it HAS, rather than nested
    // in the sections that name spells out.
    let flat = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/shaped/_settings?flat_settings=true",
        None,
    )
    .await;
    assert_eq!(
        (
            flat.status,
            flat.body["shaped"]["settings"]["index"]["search.slowlog.threshold.query.warn"]
                .as_str(),
            flat.body["shaped"]["settings"]["index"]["refresh_interval"].as_str(),
        ),
        (200, Some("2s"), Some("30s")),
        "case \"GET /shaped/_settings?flat_settings=true\": expected the dotted names, got {} \
         with {:?}",
        flat.status,
        flat.body
    );

    // The mappings are read back keyed by the index, and an update may ADD a field.
    let mapping = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/shaped/_mapping",
        None,
    )
    .await;
    assert_eq!(
        (
            mapping.status,
            mapping.body["shaped"]["mappings"]["properties"]["title"]["type"].as_str()
        ),
        (200, Some("text")),
        "case \"GET /shaped/_mapping\": expected the mapped field, got {} with {:?}",
        mapping.status,
        mapping.body
    );

    // (the case, the mapping update, whether it is taken)
    let mappings: Vec<(&str, Value, bool)> = vec![
        (
            "a field the mappings do not map yet",
            json!({ "properties": { "label": { "type": "keyword" } } }),
            true,
        ),
        (
            "a sub-field added to a mapped field",
            json!({ "properties": { "title": { "type": "text",
                "fields": { "raw": { "type": "keyword" } } } } }),
            true,
        ),
        (
            "a mapped field re-typed",
            json!({ "properties": { "title": { "type": "keyword" } } }),
            false,
        ),
        (
            "a mapped field of another type re-typed",
            json!({ "properties": { "label": { "type": "integer" } } }),
            false,
        ),
    ];
    for (case, update, accepted) in &mappings {
        let answered = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "PUT",
            "/shaped/_mapping",
            Some(update.clone()),
        )
        .await;
        if *accepted {
            assert_eq!(
                (answered.status, answered.body["acknowledged"].as_bool()),
                (200, Some(true)),
                "case {case:?}: expected the mapping update to be acknowledged, got {} with \
                 {:?}",
                answered.status,
                answered.body
            );
        } else {
            assert_eq!(
                (answered.status, error_type(&answered)),
                (400, ILLEGAL_ARGUMENT.to_string()),
                "case {case:?}: expected a re-typed field to be refused, got {} with {:?}",
                answered.status,
                answered.body
            );
            assert!(
                answered.body["error"]["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("cannot be changed from type")),
                "case {case:?}: expected the engine's own sentence, got {:?}",
                answered.body
            );
        }
    }
    let evolved = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/shaped/_mapping",
        None,
    )
    .await;
    assert_eq!(
        (
            evolved.body["shaped"]["mappings"]["properties"]["label"]["type"].as_str(),
            evolved.body["shaped"]["mappings"]["properties"]["title"]["type"].as_str(),
            evolved.body["shaped"]["mappings"]["properties"]["title"]["fields"]["raw"]["type"]
                .as_str(),
        ),
        (Some("keyword"), Some("text"), Some("keyword")),
        "case \"the mappings after the accepted updates\": expected the added field and the \
         added sub-field with the original type kept, got {:?}",
        evolved.body
    );

    // An index the domain does not hold is the engine's own not-found on both halves.
    for path in ["/absent/_settings", "/absent/_mapping"] {
        let answered = rest(&emulator, DEFAULT_ACCESS_KEY_ID, &host, "GET", path, None).await;
        assert_eq!(
            (answered.status, error_type(&answered)),
            (404, "index_not_found_exception".to_string()),
            "case {path:?}: expected the missing index, got {} with {:?}",
            answered.status,
            answered.body
        );
    }
}

/// RULE: the `_cat` APIs list what the domain holds — one row per index, out of that
/// index's own settings and documents, and one row per node the cluster configuration
/// provisions.
#[tokio::test]
async fn the_cat_apis_list_the_indexes_and_the_nodes_the_domain_holds() {
    let emulator = Emulator::new();
    let host = created_cluster(&emulator, "catted", 2).await;

    // Nothing to list before an index exists: the empty LIST, not an absent body.
    let empty = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/_cat/indices?format=json",
        None,
    )
    .await;
    assert_eq!(
        (empty.status, empty.body.as_array().map(Vec::len)),
        (200, Some(0)),
        "case \"GET /_cat/indices with no index\": expected an empty array, got {} with {:?}",
        empty.status,
        empty.body
    );

    // (the index, its shard count, its replica count, the documents written into it)
    let rows: &[(&str, i64, i64, usize)] = &[("green-one", 1, 1, 2), ("yellow-one", 2, 3, 1)];
    for (index, shards, replicas, documents) in rows {
        created_index(
            &emulator,
            &host,
            index,
            json!({ "settings": {
                "number_of_shards": shards,
                "number_of_replicas": replicas,
            } }),
        )
        .await;
        for at in 0..*documents {
            let written = rest(
                &emulator,
                DEFAULT_ACCESS_KEY_ID,
                &host,
                "PUT",
                &format!("/{index}/_doc/{at}"),
                Some(json!({ "n": at })),
            )
            .await;
            assert_eq!(
                written.status, 201,
                "case \"a document of {index}\": expected 201, got {} with {:?}",
                written.status, written.body
            );
        }
    }

    let listed = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/_cat/indices?format=json",
        None,
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "case \"GET /_cat/indices\": expected 200, got {} with {:?}",
        listed.status, listed.body
    );
    let reported: Vec<(String, String, String, String, String)> = listed
        .body
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|row| {
            (
                row["index"].as_str().unwrap_or_default().to_string(),
                row["health"].as_str().unwrap_or_default().to_string(),
                row["status"].as_str().unwrap_or_default().to_string(),
                row["pri"].as_str().unwrap_or_default().to_string(),
                row["docs.count"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        reported,
        vec![
            (
                "green-one".to_string(),
                "green".to_string(),
                "open".to_string(),
                "1".to_string(),
                "2".to_string()
            ),
            (
                "yellow-one".to_string(),
                "yellow".to_string(),
                "open".to_string(),
                "2".to_string(),
                "1".to_string()
            ),
        ],
        "case \"GET /_cat/indices\": expected one row per index out of its own settings and \
         documents, got {:?}",
        listed.body
    );

    // The per-index form is the same listing, filtered to the index the path names.
    let one = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/_cat/indices/green-one?format=json",
        None,
    )
    .await;
    assert_eq!(
        (
            one.status,
            one.body.as_array().map(Vec::len),
            one.body[0]["index"].as_str()
        ),
        (200, Some(1), Some("green-one")),
        "case \"GET /_cat/indices/green-one\": expected the one row, got {} with {:?}",
        one.status,
        one.body
    );

    // One row per node the cluster configuration provisions, with exactly one of them the
    // elected cluster manager.
    let nodes = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/_cat/nodes?format=json",
        None,
    )
    .await;
    let census = nodes.body.as_array().cloned().unwrap_or_default();
    assert_eq!(
        (nodes.status, census.len()),
        (200, 2),
        "case \"GET /_cat/nodes\": expected one row per data node, got {} with {:?}",
        nodes.status,
        nodes.body
    );
    assert_eq!(
        census
            .iter()
            .filter(|row| row["cluster_manager"].as_str() == Some("*"))
            .count(),
        1,
        "case \"GET /_cat/nodes\": expected exactly one elected cluster manager, got {:?}",
        nodes.body
    );
    assert!(
        census
            .iter()
            .all(|row| row["node.role"].as_str() == Some("dim")),
        "case \"GET /_cat/nodes\": expected each node to report the letters of the roles a data \
         node of a domain with no dedicated master holds, got {:?}",
        nodes.body
    );

    // A request asking for no JSON is answered the aligned text a terminal reads, one line
    // per row, with the columns padded to the widest value in each.
    let (status, text) = rest_text(
        &emulator,
        &host,
        "GET",
        "/_cat/indices?v=true",
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"GET /_cat/indices?v\": expected 200, got {status} with {text:?}"
    );
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "case \"GET /_cat/indices?v\": expected a header and one line per index, got {text:?}"
    );
    assert!(
        lines[0].starts_with("health status index"),
        "case \"GET /_cat/indices?v\": expected the column names first, got {text:?}"
    );
    assert!(
        lines[1].starts_with("green  open   green-one")
            && lines[2].starts_with("yellow open   yellow-one"),
        "case \"GET /_cat/indices?v\": expected each column padded to its widest value, got \
         {text:?}"
    );
    let (bare_status, bare) = rest_text(&emulator, &host, "GET", "/_cat/indices").await;
    assert_eq!(
        (bare_status, bare.lines().count()),
        (200, 2),
        "case \"GET /_cat/indices without v\": expected the rows with no header, got {bare:?}"
    );
}

/// RULE: an alias is a virtual index name — a write addressed to one lands in the index it
/// is the write index of, and a read addressed to one reads the indexes it points at.
#[tokio::test]
async fn an_alias_stands_for_the_indexes_it_points_at() {
    let emulator = Emulator::new();
    created_domain(&emulator, "aliased").await;
    let host = endpoint_of(&emulator, "aliased");
    for index in ["logs-000001", "logs-000002"] {
        created_index(
            &emulator,
            &host,
            index,
            json!({ "settings": { "number_of_shards": 1, "number_of_replicas": 0 } }),
        )
        .await;
    }

    let added = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "POST",
        "/_aliases",
        Some(json!({ "actions": [
            { "add": { "index": "logs-000001", "alias": "logs-write", "is_write_index": true } },
            { "add": { "index": "logs-000001", "alias": "logs-read" } },
            { "add": { "index": "logs-000002", "alias": "logs-read" } },
        ] })),
    )
    .await;
    assert_eq!(
        (added.status, added.body["acknowledged"].as_bool()),
        (200, Some(true)),
        "case \"POST /_aliases\": expected the actions to be acknowledged, got {} with {:?}",
        added.status,
        added.body
    );

    // A write through the write alias lands in the backing index, and says so.
    let written = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "POST",
        "/logs-write/_doc/one",
        Some(json!({ "message": "first" })),
    )
    .await;
    assert_eq!(
        (written.status, written.body["_index"].as_str()),
        (201, Some("logs-000001")),
        "case \"a write through the write alias\": expected the backing index, got {} with \
         {:?}",
        written.status,
        written.body
    );
    // And so does one through the bulk API.
    let loaded = bulk(
        &emulator,
        &host,
        "/_bulk",
        &[
            json!({ "index": { "_index": "logs-write", "_id": "two" } }),
            json!({ "message": "second" }),
        ],
    )
    .await;
    assert_eq!(
        (
            loaded.status,
            loaded.body["items"][0]["index"]["_index"].as_str()
        ),
        (200, Some("logs-000001")),
        "case \"a bulk write through the write alias\": expected the backing index, got {} \
         with {:?}",
        loaded.status,
        loaded.body
    );

    // A read through an alias reads every index it points at.
    assert_eq!(
        counted(&emulator, &host, "logs-read").await,
        2,
        "case \"a count through the read alias\": expected the documents of both backing \
         indexes"
    );
    let through_alias = document(&emulator, &host, "logs-read", "one").await;
    assert_eq!(
        through_alias.as_ref().map(|found| found["_index"].clone()),
        Some(json!("logs-000001")),
        "case \"a document read through an alias\": expected the backing index, got \
         {through_alias:?}"
    );

    // Rotating the write alias to the next backing index routes new writes there, which is
    // what makes an alias the address a producer keeps.
    let rotated = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "POST",
        "/_aliases",
        Some(json!({ "actions": [
            { "remove": { "index": "logs-000001", "alias": "logs-write" } },
            { "add": { "index": "logs-000002", "alias": "logs-write", "is_write_index": true } },
        ] })),
    )
    .await;
    assert_eq!(
        (rotated.status, rotated.body["acknowledged"].as_bool()),
        (200, Some(true)),
        "case \"rotating the write alias\": got {} with {:?}",
        rotated.status,
        rotated.body
    );
    let after = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "POST",
        "/logs-write/_doc/three",
        Some(json!({ "message": "third" })),
    )
    .await;
    assert_eq!(
        (after.status, after.body["_index"].as_str()),
        (201, Some("logs-000002")),
        "case \"a write after the rotation\": expected the new backing index, got {} with \
         {:?}",
        after.status,
        after.body
    );

    // The aliases are reported back, per index and for the whole cluster.
    let reported = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/logs-000002/_alias",
        None,
    )
    .await;
    assert_eq!(
        (
            reported.status,
            reported.body["logs-000002"]["aliases"]["logs-write"]["is_write_index"].as_bool(),
            reported.body["logs-000002"]["aliases"]["logs-read"].is_object(),
        ),
        (200, Some(true), true),
        "case \"GET /logs-000002/_alias\": expected both aliases, got {} with {:?}",
        reported.status,
        reported.body
    );
    let index_read = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/logs-000001",
        None,
    )
    .await;
    assert!(
        index_read.body["logs-000001"]["aliases"]["logs-read"].is_object(),
        "case \"GET /logs-000001\": expected the index read to report its aliases, got {:?}",
        index_read.body
    );
    // The cluster-wide read reports every index's aliases, which is the listing the alias
    // API answers for a request that names no index.
    let cluster_wide = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/_alias",
        None,
    )
    .await;
    assert_eq!(
        (
            cluster_wide.status,
            cluster_wide.body["logs-000001"]["aliases"]["logs-read"].is_object(),
            cluster_wide.body["logs-000002"]["aliases"]["logs-write"]["is_write_index"].as_bool(),
        ),
        (200, true, Some(true)),
        "case \"GET /_alias\": expected every index's aliases, got {} with {:?}",
        cluster_wide.status,
        cluster_wide.body
    );

    // (the case, the actions, the status, the error type)
    let refusals: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an action naming an index the domain does not hold",
            json!({ "actions": [{ "add": { "index": "absent", "alias": "a" } }] }),
            404,
            "index_not_found_exception",
        ),
        (
            "an action naming no alias",
            json!({ "actions": [{ "add": { "index": "logs-000001" } }] }),
            400,
            ILLEGAL_ARGUMENT,
        ),
        (
            "an action nothing is registered for",
            json!({ "actions": [{ "rename": { "index": "logs-000001", "alias": "a" } }] }),
            400,
            ILLEGAL_ARGUMENT,
        ),
        (
            "a body carrying no actions at all",
            json!({ "actions": [] }),
            400,
            ILLEGAL_ARGUMENT,
        ),
        (
            "removing an alias the index does not carry",
            json!({ "actions": [{ "remove": { "index": "logs-000001", "alias": "absent" } }] }),
            404,
            "aliases_not_found_exception",
        ),
        (
            "an alias named after an index the domain holds",
            json!({ "actions": [
                { "add": { "index": "logs-000001", "alias": "logs-000002" } }
            ] }),
            400,
            "invalid_alias_name_exception",
        ),
    ];
    for (case, actions, status, error) in &refusals {
        let answered = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "POST",
            "/_aliases",
            Some(actions.clone()),
        )
        .await;
        assert_eq!(
            (answered.status, error_type(&answered)),
            (*status, (*error).to_string()),
            "case {case:?}: expected the action to be refused, got {} with {:?}",
            answered.status,
            answered.body
        );
    }
    // The refused actions changed nothing: the write alias still points where it did.
    let unchanged = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "POST",
        "/logs-write/_doc/four",
        Some(json!({ "message": "fourth" })),
    )
    .await;
    assert_eq!(
        unchanged.body["_index"].as_str(),
        Some("logs-000002"),
        "case \"a write after the refused actions\": expected the alias to be where the last \
         accepted action put it, got {:?}",
        unchanged.body
    );
}

/// RULE: a component template is what an index template is composed of — an index created
/// under a name the template matches carries every component's section with the template's
/// own merged on top — and two index templates may not match one name at equal priority.
#[tokio::test]
async fn a_component_template_composes_the_index_template_that_names_it() {
    let emulator = Emulator::new();
    created_domain(&emulator, "composed").await;
    let host = endpoint_of(&emulator, "composed");

    // (the case, the component's name, the section it declares)
    let components: Vec<(&str, &str, Value)> = vec![
        (
            "a component declaring settings",
            "shard-defaults",
            json!({ "template": { "settings": {
                "number_of_shards": 3,
                "number_of_replicas": 0,
            } } }),
        ),
        (
            "a component declaring mappings",
            "event-mappings",
            json!({ "template": { "mappings": { "properties": {
                "count": { "type": "integer" },
            } } } }),
        ),
    ];
    for (case, name, body) in &components {
        let written = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "PUT",
            &format!("/_component_template/{name}"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            (written.status, written.body["acknowledged"].as_bool()),
            (200, Some(true)),
            "case {case:?}: expected the component template to be acknowledged, got {} with \
             {:?}",
            written.status,
            written.body
        );
    }
    let read_back = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/_component_template/shard-defaults",
        None,
    )
    .await;
    assert_eq!(
        (
            read_back.status,
            read_back.body["component_templates"][0]["name"].as_str(),
            read_back.body["component_templates"][0]["component_template"]["template"]
                ["settings"]["number_of_shards"]
                .as_i64(),
        ),
        (200, Some("shard-defaults"), Some(3)),
        "case \"GET /_component_template/shard-defaults\": expected the component as written, \
         got {} with {:?}",
        read_back.status,
        read_back.body
    );
    let missing = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/_component_template/never-written",
        None,
    )
    .await;
    assert_eq!(
        (missing.status, error_type(&missing)),
        (404, "resource_not_found_exception".to_string()),
        "case \"a component template the cluster does not hold\": expected the engine's own \
         not-found, got {} with {:?}",
        missing.status,
        missing.body
    );

    // A template composed of both, with a section of its own that wins over them.
    let template = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/_index_template/events",
        Some(json!({
            "index_patterns": ["events-*"],
            "priority": 100,
            "composed_of": ["shard-defaults", "event-mappings"],
            "template": { "settings": { "number_of_replicas": 1 } },
        })),
    )
    .await;
    assert_eq!(
        (template.status, template.body["acknowledged"].as_bool()),
        (200, Some(true)),
        "case \"PUT /_index_template/events\": got {} with {:?}",
        template.status,
        template.body
    );

    // (the case, the index name, whether the template applies to it)
    let created: &[(&str, &str, bool)] = &[
        ("a name the template's pattern matches", "events-2026", true),
        ("a name it does not match", "audits-2026", false),
    ];
    for (case, index, applies) in created {
        created_index(&emulator, &host, index, json!({})).await;
        let settings = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "GET",
            &format!("/{index}/_settings"),
            None,
        )
        .await;
        let mapping = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "GET",
            &format!("/{index}/_mapping"),
            None,
        )
        .await;
        let expected = if *applies {
            // The component's shard count, the component's mapping, and the template's own
            // replica count over the component's.
            (Some("3"), Some("1"), Some("integer"))
        } else {
            // No template applied, so the index holds no setting of its own and maps no
            // field at all.
            (None, None, None)
        };
        assert_eq!(
            (
                settings.body[index]["settings"]["index"]["number_of_shards"].as_str(),
                settings.body[index]["settings"]["index"]["number_of_replicas"].as_str(),
                mapping.body[index]["mappings"]["properties"]["count"]["type"].as_str(),
            ),
            expected,
            "case {case:?}: expected the composition the template names, got {:?} and {:?}",
            settings.body,
            mapping.body
        );
    }

    // (the case, the template, the status it earns)
    let templates: Vec<(&str, &str, Value, u16)> = vec![
        (
            "another template matching the same pattern at the same priority",
            "events-again",
            json!({ "index_patterns": ["events-*"], "priority": 100 }),
            400,
        ),
        (
            "a template whose pattern overlaps at the same priority",
            "events-narrow",
            json!({ "index_patterns": ["events-2026-*"], "priority": 100 }),
            400,
        ),
        (
            "the same template at another priority",
            "events-higher",
            json!({ "index_patterns": ["events-*"], "priority": 200 }),
            200,
        ),
        (
            "a template matching other names at the same priority",
            "audits",
            json!({ "index_patterns": ["audits-*"], "priority": 100 }),
            200,
        ),
        (
            "a template composed of a component the cluster does not hold",
            "composed-of-nothing",
            json!({ "index_patterns": ["nothing-*"], "composed_of": ["absent"] }),
            400,
        ),
        (
            // The list and its entries are the caller's, and deciding an overlap compares
            // every entry against every entry each stored template carries, so a list past
            // what this cluster compares is refused rather than stored.
            "a template naming more patterns than the cluster takes",
            "too-many-patterns",
            json!({
                "index_patterns":
                    (0..101).map(|at| format!("many-{at}-*")).collect::<Vec<String>>(),
            }),
            400,
        ),
        (
            "a template naming a pattern longer than an index name may be",
            "oversized-pattern",
            json!({ "index_patterns": ["a".repeat(256)] }),
            400,
        ),
        (
            "a template composed of more components than the cluster takes",
            "too-many-components",
            json!({
                "index_patterns": ["components-*"],
                "composed_of":
                    (0..101).map(|at| format!("component-{at}")).collect::<Vec<String>>(),
            }),
            400,
        ),
    ];
    for (case, name, body, status) in &templates {
        let answered = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "PUT",
            &format!("/_index_template/{name}"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            answered.status, *status,
            "case {case:?}: expected {status}, got {} with {:?}",
            answered.status, answered.body
        );
        if *status == 400 {
            assert_eq!(
                error_type(&answered),
                ILLEGAL_ARGUMENT,
                "case {case:?}: expected the engine's own refusal, got {:?}",
                answered.body
            );
        }
    }
}

/// RULE: the cluster health follows the replica copies the TOPOLOGY can hold — a copy needs
/// a node other than the one holding its primary, so copies past that are unassigned and
/// the cluster is yellow until the count fits.
#[tokio::test]
async fn the_cluster_health_follows_the_replica_copies_the_topology_can_hold() {
    // (the case, the data nodes, the index's shards, its replica copies, the status, the
    //  copies that have nowhere to go)
    struct Row {
        case: &'static str,
        nodes: i64,
        shards: i64,
        replicas: i64,
        status: &'static str,
        unassigned: i64,
    }
    let rows: &[Row] = &[
        Row {
            case: "one node and no replica",
            nodes: 1,
            shards: 2,
            replicas: 0,
            status: "green",
            unassigned: 0,
        },
        Row {
            case: "one node and one replica, which has nowhere to go",
            nodes: 1,
            shards: 1,
            replicas: 1,
            status: "yellow",
            unassigned: 1,
        },
        Row {
            case: "two nodes and one replica",
            nodes: 2,
            shards: 2,
            replicas: 1,
            status: "green",
            unassigned: 0,
        },
        Row {
            case: "two nodes and more replicas than they can hold",
            nodes: 2,
            shards: 1,
            replicas: 2,
            status: "yellow",
            unassigned: 1,
        },
        Row {
            case: "three nodes and two replicas",
            nodes: 3,
            shards: 2,
            replicas: 2,
            status: "green",
            unassigned: 0,
        },
    ];
    for (at, row) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let domain = format!("health-{at}");
        let host = created_cluster(&emulator, &domain, row.nodes).await;
        created_index(
            &emulator,
            &host,
            "replicated",
            json!({ "settings": {
                "number_of_shards": row.shards,
                "number_of_replicas": row.replicas,
            } }),
        )
        .await;
        let health = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "GET",
            "/_cluster/health",
            None,
        )
        .await;
        assert_eq!(
            (
                health.status,
                health.body["status"].as_str(),
                health.body["unassigned_shards"].as_i64(),
                health.body["active_primary_shards"].as_i64(),
            ),
            (200, Some(row.status), Some(row.unassigned), Some(row.shards)),
            "case {:?}: expected the health the topology leaves, got {} with {:?}",
            row.case,
            health.status,
            health.body
        );
    }

    // And lowering an over-replicated index's copies to what the topology can hold turns
    // the cluster green, which is the transition a client waits for.
    let emulator = Emulator::new();
    let host = created_cluster(&emulator, "settling", 2).await;
    created_index(
        &emulator,
        &host,
        "over-replicated",
        json!({ "settings": { "number_of_shards": 1, "number_of_replicas": 2 } }),
    )
    .await;
    let before = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/_cluster/health/over-replicated?wait_for_status=green",
        None,
    )
    .await;
    assert_eq!(
        (
            before.status,
            before.body["status"].as_str(),
            before.body["timed_out"].as_bool()
        ),
        (200, Some("yellow"), Some(true)),
        "case \"an over-replicated index\": expected yellow and a wait that expired, got {} \
         with {:?}",
        before.status,
        before.body
    );
    let lowered = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/over-replicated/_settings",
        Some(json!({ "index": { "number_of_replicas": 1 } })),
    )
    .await;
    assert_eq!(
        (lowered.status, lowered.body["acknowledged"].as_bool()),
        (200, Some(true)),
        "case \"lowering the replica count\": got {} with {:?}",
        lowered.status,
        lowered.body
    );
    let after = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "GET",
        "/_cluster/health/over-replicated?wait_for_status=green",
        None,
    )
    .await;
    assert_eq!(
        (
            after.status,
            after.body["status"].as_str(),
            after.body["unassigned_shards"].as_i64(),
            after.body["timed_out"].as_bool(),
        ),
        (200, Some("green"), Some(0), Some(false)),
        "case \"after the replica count fits\": expected green with nothing unassigned, got \
         {} with {:?}",
        after.status,
        after.body
    );
}

/// RULE: a `match` on an analyzed `text` field finds every document whose field carries the
/// query's term, and the `_analyze` API reports the tokens the index's own chain produces —
/// while a field that holds its values whole is matched as a whole value.
///
/// The rows are the terms a text holds and the ones it does not; the index's keyword
/// sibling is the control, matched whole.
#[tokio::test]
async fn a_match_on_a_text_field_finds_every_document_carrying_the_term() {
    let emulator = Emulator::new();
    created_domain(&emulator, "searched").await;
    let host = endpoint_of(&emulator, "searched");
    created_index(
        &emulator,
        &host,
        "articles",
        json!({
            "settings": { "analysis": { "analyzer": { "titles": {
                "type": "custom",
                "tokenizer": "standard",
                "filter": ["lowercase"],
            } } } },
            "mappings": { "properties": { "title": {
                "type": "text",
                "analyzer": "titles",
                "fields": { "raw": { "type": "keyword" } },
            } } },
        }),
    )
    .await;
    let titles: &[(&str, &str)] = &[
        ("d1", "Gamma Release Notes"),
        ("d2", "Alpha Release Notes"),
        ("d3", "Beta Candidate"),
    ];
    for (id, title) in titles {
        let written = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "PUT",
            &format!("/articles/_doc/{id}?refresh=true"),
            Some(json!({ "title": title })),
        )
        .await;
        assert_eq!(
            written.status, 201,
            "case \"a document titled {title:?}\": expected 201, got {} with {:?}",
            written.status, written.body
        );
    }

    // (the case, the query, the ids it must select)
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "a term every document's analyzed field holds",
            json!({ "query": { "match": { "title": "release" } } }),
            vec!["d1", "d2"],
        ),
        (
            "the same term in another case, which the chain folds",
            json!({ "query": { "match": { "title": "RELEASE" } } }),
            vec!["d1", "d2"],
        ),
        (
            "a term only one document holds",
            json!({ "query": { "match": { "title": "beta" } } }),
            vec!["d3"],
        ),
        (
            "a term no document holds",
            json!({ "query": { "match": { "title": "omega" } } }),
            vec![],
        ),
        (
            "the keyword sibling, matched as a whole value",
            json!({ "query": { "term": { "title.raw": "Beta Candidate" } } }),
            vec!["d3"],
        ),
        (
            "the keyword sibling asked for one of its terms",
            json!({ "query": { "term": { "title.raw": "beta" } } }),
            vec![],
        ),
        (
            "a phrase the analyzed field holds in order",
            json!({ "query": { "match_phrase": { "title": "alpha release" } } }),
            vec!["d2"],
        ),
    ];
    for (case, query, expected) in &rows {
        let answered = rest(
            &emulator,
            DEFAULT_ACCESS_KEY_ID,
            &host,
            "POST",
            "/articles/_search",
            Some(query.clone()),
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            answered.status, answered.body
        );
        let selected: Vec<String> = answered.body["hits"]["hits"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|hit| hit["_id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            selected,
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the documents the rule selects, got {:?}",
            answered.body
        );
        assert_eq!(
            answered.body["hits"]["total"]["value"].as_i64(),
            Some(i64::try_from(expected.len()).unwrap_or_default()),
            "case {case:?}: expected the total to be the documents selected, got {:?}",
            answered.body
        );
    }

    // The tokens the index's own chain produces, which is what decides those matches.
    let analyzed = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "POST",
        "/articles/_analyze",
        Some(json!({ "analyzer": "titles", "text": "Gamma Release Notes" })),
    )
    .await;
    let tokens: Vec<String> = analyzed.body["tokens"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|token| token["token"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        (analyzed.status, tokens),
        (
            200,
            vec![
                "gamma".to_string(),
                "release".to_string(),
                "notes".to_string()
            ]
        ),
        "case \"POST /articles/_analyze\": expected the chain's own tokens, got {} with {:?}",
        analyzed.status,
        analyzed.body
    );
    let by_field = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "POST",
        "/articles/_analyze",
        Some(json!({ "field": "title", "text": "Beta Candidate" })),
    )
    .await;
    assert_eq!(
        by_field.status, 200,
        "case \"an analyze naming the field\": expected the field's own chain, got {} with \
         {:?}",
        by_field.status, by_field.body
    );
}

/// RULE: a RESERVED first path segment — one beginning with `_` — names an API of the
/// engine and never an index, so a reserved path this surface does not serve is answered as
/// a path with no handler rather than as a request about an index.
#[tokio::test]
async fn a_reserved_first_segment_is_never_read_as_an_index_name() {
    let emulator = Emulator::new();
    created_domain(&emulator, "reserved").await;
    let host = endpoint_of(&emulator, "reserved");

    // (the case, the method, the path)
    let rows: &[(&str, &str, &str)] = &[
        ("a reserved endpoint this surface serves nothing for", "POST", "/_msearch"),
        ("a reserved endpoint under a served prefix", "GET", "/_cat/shards"),
        ("a reserved two-segment path", "POST", "/_search/scroll"),
        ("a reserved endpoint named like a document path", "PUT", "/_reindex/_doc/one"),
        ("a reserved endpoint named like an index's settings", "GET", "/_bogus/_settings"),
    ];
    for (case, method, path) in rows {
        let answered = rest(&emulator, DEFAULT_ACCESS_KEY_ID, &host, method, path, None).await;
        assert_eq!(
            answered.status, 400,
            "case {case:?}: expected the engine's unserved-path refusal, got {} with {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.body["error"].as_str(),
            Some(format!("no handler found for uri [{path}] and method [{method}]").as_str()),
            "case {case:?}: expected the unserved-path sentence, got {:?}",
            answered.body
        );
    }

    // A reserved endpoint this surface DOES serve, reached with a method it does not take,
    // is the other refusal — and it names the methods THAT route takes, not an index
    // route's.
    let wrong_method = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "DELETE",
        "/_bulk",
        None,
    )
    .await;
    assert_eq!(
        (wrong_method.status, wrong_method.body["error"].as_str()),
        (
            405,
            Some("Incorrect HTTP method for uri [/_bulk] and method [DELETE], allowed: [POST, PUT]")
        ),
        "case \"a served reserved endpoint reached with another method\": expected that \
         route's own method list, got {} with {:?}",
        wrong_method.status,
        wrong_method.body
    );

    // An index name may not begin with the reserved character at all, so a create that
    // tries is the engine's own invalid-name refusal.
    let named = rest(
        &emulator,
        DEFAULT_ACCESS_KEY_ID,
        &host,
        "PUT",
        "/_underscored",
        None,
    )
    .await;
    assert_eq!(
        named.status, 400,
        "case \"an index name beginning with the reserved character\": expected a refusal, \
         got {} with {:?}",
        named.status, named.body
    );
}
