// SPDX-License-Identifier: CC0-1.0

//! What a WRITE and a DELTA SYNC through a VERSIONED (conflict-detecting) `AMAZON_DYNAMODB`
//! data source do, driven over the socket the API publishes and read back through the table's
//! own service.
//!
//! The rules these cases assert, one level above the recorded cases (one create that must come
//! back stamped `_version` 1, one stale write that must be refused, one merged write, one
//! refused delete and one delta page):
//!
//! > 1. Every write through a data source configured for versioned items stamps the item's own
//! >    `_version` — 1 on the write that creates it, one more on each write accepted after that
//! >    — and `_lastChangedAt`, the moment of that write in epoch milliseconds, onto the ROW the
//! >    table holds, and answers the row carrying both; a delete leaves the row behind as a
//! >    tombstone marked `_deleted`. The count belongs to the ITEM, so another item starts its
//! >    own at 1.
//! > 2. Where the resolver asks for version detection, a write is applied only against the row
//! >    it was built from: a put CREATES the item, so a key that already holds a row is a
//! >    conflict whatever version the request states — unless the request stated grounds of its
//! >    own to be refused on, which are the grounds it is then weighed on, by the service whose
//! >    language they are written in — and an update or a delete conflicts when the version it
//! >    states is not the stored one. A conflict is handed to the resolver's `conflictHandler` —
//! >    `AUTOMERGE` composes the write into the stored row and writes it as the next version
//! >    (keeping the server's value for a scalar both carry), except a delete, which has nothing
//! >    to compose and is refused — and every other configuration rejects it: exactly one
//! >    `ConflictUnhandled` field error carrying the row as the server holds it, with nothing
//! >    written. A resolver asking for no detection applies its write as it stands.
//! > 3. Each ACCEPTED write advances the version exactly once, so writes of one item made at the
//! >    same time are handed consecutive versions, one each, and none of them is lost. Each of
//! >    them is answered the row ITS own turn committed — the writes accepted before it and its
//! >    own change, and nothing of the turns that followed.
//! > 4. A delta sync answers the rows changed AFTER the moment it is given — the tombstones among
//! >    them, so a removal can be observed — beside the `startedAt` a client carries into its
//! >    next sync and the `nextToken` that continues the page it read.
//!
//! The rows span what those rules generalise over: the kind of write (put, update, delete), the
//! conflict configuration (rejecting, merging, detecting nothing, and no configuration at all),
//! how many writes of one item are in flight at once, and the moment a sync is given (none, the
//! moment of a known write, and one past every write).
//!
//! Two services have to agree for any of it to hold — DynamoDB holds the table and IAM the
//! data source's role — so every write is also read back through DynamoDB's OWN reader: a
//! version that did not reach the row cannot pass.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::appsync;
use crate::graphql_api_iam_realtime_dynamodb_flow::{create_role, dynamodb};
use crate::graphql_api_mapping_template_resolvers::{
    install_schema, post_graphql, GraphqlApi, REGION,
};

/// The table every data source of this suite resolves against, and its one key attribute.
const TABLE: &str = "appsync-versioned-posts";
const KEY_ATTRIBUTE: &str = "id";

/// The name of the versioned `AMAZON_DYNAMODB` data source every resolver is bound to.
const DATA_SOURCE: &str = "Posts";

/// How long a row a versioned delete leaves behind is retained, in minutes
/// (`DeltaSyncConfig$baseTableTTL`, "the number of minutes that an Item is stored in the data
/// source"): the suite's own choice, long enough that no tombstone it writes is reapable while
/// a case reads it.
const BASE_TABLE_TTL_MINUTES: i64 = 60;

/// How long a case waits for a state the clock owes, and how often it looks again. Generous:
/// what is under test is what the endpoint answers, never how fast the host gets there.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(50);

/// The selection every case makes of a post: the attributes a case writes, and the three
/// system attributes a versioned data source owns.
const SELECTION: &str =
    "{ id title alpha beta gamma delta _version _deleted _lastChangedAt }";

/// The schema every API of this suite is created with.
const SCHEMA: &str = "schema { query: Query mutation: Mutation }\n\
     \n\
     type Post {\n  id: ID!\n  title: String\n  alpha: String\n  beta: String\n  \
     gamma: String\n  delta: String\n  _version: Int\n  _deleted: Boolean\n  \
     _lastChangedAt: Float\n}\n\
     \n\
     type SyncPage {\n  items: [Post]\n  nextToken: String\n  startedAt: Float\n}\n\
     \n\
     type Query {\n  getPost(id: ID!): Post\n  \
     syncPosts(lastSync: Float, limit: Int, nextToken: String): SyncPage\n}\n\
     \n\
     type Mutation {\n  \
     createPost(id: ID!, title: String!): Post\n  \
     putPost(id: ID!, title: String!, expectedVersion: Int!): Post\n  \
     putPostIfAbsent(id: ID!, title: String!, expectedVersion: Int!): Post\n  \
     putPostMerging(id: ID!, title: String!, field: String!, value: String!, \
     expectedVersion: Int!): Post\n  \
     updatePost(id: ID!, title: String!, expectedVersion: Int!): Post\n  \
     mergePost(id: ID!, field: String!, value: String!, expectedVersion: Int!): Post\n  \
     updatePostUndetected(id: ID!, title: String!, expectedVersion: Int!): Post\n  \
     updatePostUnconfigured(id: ID!, title: String!, expectedVersion: Int!): Post\n  \
     deletePost(id: ID!, expectedVersion: Int!): Post\n  \
     deletePostMerging(id: ID!, expectedVersion: Int!): Post\n}\n";

/// The `errorType` AppSync reports a conflict that was not resolved under, and the one it
/// reports a write the TABLE refused under: the service the call was made to and the class of
/// the exception its client raised.
const CONFLICT_UNHANDLED: &str = "ConflictUnhandled";
const CONDITION_REFUSED: &str = "DynamoDB:ConditionalCheckFailedException";

/// The sentence a conflict that was not resolved is reported with: fixed text, carrying nothing
/// of the request or of the row — the row the server holds travels beside it, in the error's own
/// `data`.
const CONFLICT_REJECTED: &str = "Conflict resolver rejects mutation.";

#[tokio::test]
async fn every_write_stamps_the_items_own_version_and_the_moment_it_changed() {
    let suite = setup("stamping").await;
    let id = "post-stamped";

    /// One write of the walk: what it is called, the field it drives, the document and
    /// variables it is driven with, and the version and deletion marker the row then carries.
    struct Step<'a> {
        case: &'a str,
        field: &'a str,
        query: String,
        variables: Value,
        version: i64,
        deleted: bool,
    }
    // The steps walk ONE item through every write such a data source serves, in order, so each
    // one's expectation is the version the write before it left plus one.
    let steps = vec![
        Step {
            case: "the write that creates the item",
            field: "createPost",
            query: format!(
                "mutation M($id: ID!, $title: String!) \
                 {{ createPost(id: $id, title: $title) {SELECTION} }}"
            ),
            variables: json!({"id": id, "title": "first"}),
            version: 1,
            deleted: false,
        },
        Step {
            case: "an update carrying the version the row stands at",
            field: "updatePost",
            query: format!(
                "mutation M($id: ID!, $title: String!, $v: Int!) \
                 {{ updatePost(id: $id, title: $title, expectedVersion: $v) {SELECTION} }}"
            ),
            variables: json!({"id": id, "title": "second", "v": 1}),
            version: 2,
            deleted: false,
        },
        Step {
            case: "a merging update over the version that update left",
            field: "mergePost",
            query: format!(
                "mutation M($id: ID!, $field: String!, $value: String!, $v: Int!) \
                 {{ mergePost(id: $id, field: $field, value: $value, expectedVersion: $v) \
                 {SELECTION} }}"
            ),
            variables: json!({"id": id, "field": "alpha", "value": "a", "v": 2}),
            version: 3,
            deleted: false,
        },
        Step {
            case: "the delete that leaves the tombstone",
            field: "deletePost",
            query: format!(
                "mutation M($id: ID!, $v: Int!) \
                 {{ deletePost(id: $id, expectedVersion: $v) {SELECTION} }}"
            ),
            variables: json!({"id": id, "v": 3}),
            version: 4,
            deleted: true,
        },
    ];
    let mut changed_before = 0_i64;
    for step in &steps {
        let case = step.case;
        let body = posted(&suite.api, case, &step.query, step.variables.clone()).await;
        assert_eq!(
            errors(&body),
            Vec::<Value>::new(),
            "case {case:?}: an accepted write carries no errors, got {body}"
        );
        let answered = &body["data"][step.field];
        assert_eq!(
            answered["_version"].as_i64(),
            Some(step.version),
            "case {case:?}: expected the row to answer version {}, got {answered}",
            step.version
        );
        assert_eq!(
            answered["_deleted"].as_bool().unwrap_or(false),
            step.deleted,
            "case {case:?}: expected the deletion marker to be {}, got {answered}",
            step.deleted
        );
        let changed_at = answered["_lastChangedAt"].as_i64().unwrap_or_else(|| {
            panic!("case {case:?}: expected the moment of the change, got {answered}")
        });
        assert!(
            changed_at >= changed_before && changed_at > 0,
            "case {case:?}: expected a moment at or after {changed_before}, got {changed_at}"
        );
        changed_before = changed_at;

        // And the ROW the table holds carries both, which is what a client that reads it back
        // through any reader sees.
        let held = stored_row(&suite.router, id).await;
        assert_eq!(
            held["_version"]["N"].as_str(),
            Some(step.version.to_string().as_str()),
            "case {case:?}: expected the stored row at version {}, got {held}",
            step.version
        );
        assert_eq!(
            held["_lastChangedAt"]["N"].as_str(),
            Some(changed_at.to_string().as_str()),
            "case {case:?}: expected the stored row to carry the moment answered, got {held}"
        );
        assert_eq!(
            held["_deleted"]["BOOL"].as_bool().unwrap_or(false),
            step.deleted,
            "case {case:?}: expected the stored row's deletion marker to be {}, got {held}",
            step.deleted
        );
    }
    // The tombstone the delete left says when it may be reaped, which is the retention window
    // the data source states past the moment of that write.
    let tombstone = stored_row(&suite.router, id).await;
    assert_eq!(
        tombstone["_ttl"]["N"].as_str().and_then(|at| at.parse().ok()),
        Some(changed_before / 1_000 + BASE_TABLE_TTL_MINUTES * 60),
        "case the tombstone a delete left: expected the row to be reapable a retention window \
         after the write, got {tombstone}"
    );

    // The count belongs to the ITEM: another key starts its own at 1, whatever the first one
    // reached.
    let other = posted(
        &suite.api,
        "another item of the same table",
        &format!(
            "mutation M($id: ID!, $title: String!) \
             {{ createPost(id: $id, title: $title) {SELECTION} }}"
        ),
        json!({"id": "post-other", "title": "first"}),
    )
    .await;
    assert_eq!(
        other["data"]["createPost"]["_version"].as_i64(),
        Some(1),
        "case another item of the same table: expected its own count to start at 1, got {other}"
    );
}

#[tokio::test]
async fn a_write_the_stored_row_did_not_come_from_is_handed_to_the_configured_handler() {
    let suite = setup("conflicts").await;

    /// One case: the write it makes against an item that stands at version 2 under the title
    /// `v2`, whether that write is refused, the version the row carries afterwards, and the
    /// members the answered row — or the row the refusal carries — has to hold.
    struct Row<'a> {
        case: &'a str,
        field: &'a str,
        query: String,
        variables: Value,
        /// The `errorType` the field reports, or `None` for a write that is applied.
        refused: Option<&'a str>,
        version: i64,
        expected: Value,
    }
    let update = |field: &str| {
        format!(
            "mutation M($id: ID!, $title: String!, $v: Int!) \
             {{ {field}(id: $id, title: $title, expectedVersion: $v) {SELECTION} }}"
        )
    };
    let delete = |field: &str| {
        format!(
            "mutation M($id: ID!, $v: Int!) \
             {{ {field}(id: $id, expectedVersion: $v) {SELECTION} }}"
        )
    };
    // The rows differ along the kind of write (put, update, delete), the conflict
    // configuration the resolver carries (rejecting, merging, detecting nothing, none at all),
    // whether the version the request states is the stored one, and whether the request states
    // grounds of its own to be refused on.
    let rows = [
        Row {
            case: "an update stating a version the row has moved past",
            field: "updatePost",
            query: update("updatePost"),
            variables: json!({"title": "stale", "v": 1}),
            refused: Some(CONFLICT_UNHANDLED),
            version: 2,
            expected: json!({"title": "v2"}),
        },
        Row {
            case: "an update of a resolver that asks for no detection",
            field: "updatePostUndetected",
            query: update("updatePostUndetected"),
            variables: json!({"title": "undetected", "v": 1}),
            refused: None,
            version: 3,
            expected: json!({"title": "undetected"}),
        },
        Row {
            case: "an update of a resolver carrying no sync configuration at all",
            field: "updatePostUnconfigured",
            query: update("updatePostUnconfigured"),
            variables: json!({"title": "unconfigured", "v": 1}),
            refused: None,
            version: 3,
            expected: json!({"title": "unconfigured"}),
        },
        Row {
            case: "a stale update AUTOMERGE composes into the stored row",
            field: "mergePost",
            query: format!(
                "mutation M($id: ID!, $field: String!, $value: String!, $v: Int!) \
                 {{ mergePost(id: $id, field: $field, value: $value, expectedVersion: $v) \
                 {SELECTION} }}"
            ),
            variables: json!({"field": "alpha", "value": "merged", "v": 1}),
            refused: None,
            version: 3,
            // The attribute the write adds lands, and the title both rows carry keeps the
            // value at the server.
            expected: json!({"alpha": "merged", "title": "v2"}),
        },
        Row {
            case: "a stale delete AUTOMERGE has nothing to compose",
            field: "deletePostMerging",
            query: delete("deletePostMerging"),
            variables: json!({"v": 1}),
            refused: Some(CONFLICT_UNHANDLED),
            version: 2,
            expected: json!({"title": "v2"}),
        },
        Row {
            case: "a stale delete of a rejecting resolver",
            field: "deletePost",
            query: delete("deletePost"),
            variables: json!({"v": 1}),
            refused: Some(CONFLICT_UNHANDLED),
            version: 2,
            expected: json!({"title": "v2"}),
        },
        Row {
            case: "a put over a key that already holds a row, stating the stored version",
            field: "putPost",
            query: update("putPost"),
            variables: json!({"title": "put", "v": 2}),
            refused: Some(CONFLICT_UNHANDLED),
            version: 2,
            expected: json!({"title": "v2"}),
        },
        Row {
            // The boundary of the put reading: a request that states the grounds of its own
            // refusal is weighed on exactly those grounds, by the service whose language they
            // are written in, so what the field reports is that service's own class.
            case: "a conditional put over a key that already holds a row",
            field: "putPostIfAbsent",
            query: update("putPostIfAbsent"),
            variables: json!({"title": "put", "v": 2}),
            refused: Some(CONDITION_REFUSED),
            version: 2,
            expected: json!({}),
        },
        Row {
            case: "the same put where the resolver merges instead of rejecting",
            field: "putPostMerging",
            query: format!(
                "mutation M($id: ID!, $title: String!, $field: String!, $value: String!, \
                 $v: Int!) \
                 {{ putPostMerging(id: $id, title: $title, field: $field, value: $value, \
                 expectedVersion: $v) {SELECTION} }}"
            ),
            variables: json!({"title": "put", "field": "beta", "value": "merged", "v": 2}),
            refused: None,
            version: 3,
            expected: json!({"beta": "merged", "title": "v2"}),
        },
    ];
    for (at, row) in rows.iter().enumerate() {
        let case = row.case;
        // Each row gets its own item, taken to version 2 under the title `v2` so that version
        // 1 is a version the row has moved past and 2 is the one it stands at.
        let id = format!("post-conflict-{at}");
        seeded(&suite, case, &id).await;

        let mut variables = row.variables.clone();
        variables["id"] = json!(id);
        let body = posted(&suite.api, case, &row.query, variables).await;
        let reported = errors(&body);
        if let Some(error_type) = row.refused {
            assert_eq!(
                reported.len(),
                1,
                "case {case:?}: expected exactly one field error, got {body}"
            );
            let error = &reported[0];
            assert_eq!(
                (
                    error["errorType"].as_str(),
                    error["path"].clone(),
                    body["data"][row.field].clone(),
                ),
                (Some(error_type), json!([row.field]), Value::Null),
                "case {case:?}: expected one {error_type} error on the refused field, got {body}"
            );
            if error_type == CONFLICT_UNHANDLED {
                // A conflict that was not resolved carries the row as the SERVER holds it,
                // which is what the caller needs to build the write it retries with.
                carries(case, "the refusal's own data", &error["data"], &row.expected);
                assert_eq!(
                    error["data"]["_version"].as_i64(),
                    Some(row.version),
                    "case {case:?}: expected the refusal to carry the stored version, got {body}"
                );
            }
        } else {
            assert_eq!(
                reported,
                Vec::<Value>::new(),
                "case {case:?}: expected the write to be applied, got {body}"
            );
            let answered = &body["data"][row.field];
            carries(case, "the answered row", answered, &row.expected);
            assert_eq!(
                answered["_version"].as_i64(),
                Some(row.version),
                "case {case:?}: expected the accepted write to advance the version once, got \
                 {answered}"
            );
        }

        // What the TABLE holds either way: a refused write leaves the row exactly as it was —
        // its version and no deletion marker — and an accepted one leaves the version it
        // answered.
        let held = stored_row(&suite.router, &id).await;
        assert_eq!(
            (
                held["_version"]["N"].as_str(),
                held["_deleted"]["BOOL"].as_bool().unwrap_or(false),
            ),
            (Some(row.version.to_string().as_str()), false),
            "case {case:?}: expected the stored row at version {} and no tombstone, got {held}",
            row.version
        );
    }
}

#[tokio::test]
async fn writes_of_one_item_made_at_the_same_time_are_each_advanced_once() {
    let suite = setup("concurrent").await;
    let id = "post-concurrent";
    let case = "four writes of one item in flight at once";
    seeded(&suite, case, id).await;

    // Each writer composes a different attribute and all four state the version the row has
    // already moved past, so every one of them is a conflict the resolver's AUTOMERGE
    // composes: four accepted writes after version 2 are versions 3, 4, 5 and 6, one each.
    let query = format!(
        "mutation M($id: ID!, $field: String!, $value: String!, $v: Int!) \
         {{ mergePost(id: $id, field: $field, value: $value, expectedVersion: $v) \
         {SELECTION} }}"
    );
    let writer = |field: &'static str| {
        let query = query.clone();
        let api = &suite.api;
        async move {
            posted(
                api,
                case,
                &query,
                json!({"id": id, "field": field, "value": field, "v": 1}),
            )
            .await
        }
    };
    let (first, second, third, fourth) = tokio::join!(
        writer("alpha"),
        writer("beta"),
        writer("gamma"),
        writer("delta"),
    );

    let answered = [first, second, third, fourth];
    let mut versions: Vec<i64> = answered
        .iter()
        .map(|body| {
            assert_eq!(
                errors(body),
                Vec::<Value>::new(),
                "case {case:?}: every merged write is accepted, got {body}"
            );
            body["data"]["mergePost"]["_version"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case:?}: expected a version, got {body}"))
        })
        .collect();
    versions.sort_unstable();
    assert_eq!(
        versions,
        vec![3, 4, 5, 6],
        "case {case:?}: expected each accepted write to advance the version once, got \
         {versions:?} from {answered:?}"
    );

    // And none of the four is lost: the stored row carries every attribute they composed, at
    // the version the last of them stamped.
    let held = stored_row(&suite.router, id).await;
    assert_eq!(
        (
            held["_version"]["N"].as_str(),
            held["alpha"]["S"].as_str(),
            held["beta"]["S"].as_str(),
            held["gamma"]["S"].as_str(),
            held["delta"]["S"].as_str(),
            held["title"]["S"].as_str(),
        ),
        (
            Some("6"),
            Some("alpha"),
            Some("beta"),
            Some("gamma"),
            Some("delta"),
            Some("v2"),
        ),
        "case {case:?}: expected every merged attribute at the last version, got {held}"
    );
}

#[tokio::test]
async fn a_delta_sync_answers_the_rows_changed_after_the_moment_it_is_given() {
    let suite = setup("sync").await;
    let page = |arguments: &str, parameters: &str| {
        // A field given no argument is selected bare: a parameter list and an argument list
        // are written only where the case states one.
        let selected = if arguments.is_empty() {
            "syncPosts".to_string()
        } else {
            format!("syncPosts({arguments})")
        };
        format!(
            "query S{parameters} {{ {selected} \
             {{ items {SELECTION} nextToken startedAt }} }}"
        )
    };
    let whole = page("", "");
    let since = page("lastSync: $since", "($since: Float!)");

    // Three items, created in order: the first is the one no later write touches, so the
    // moment of ITS change is a moment the other two changed after.
    let unchanged = created(&suite, "a row no later write touches", "post-a").await;
    created(&suite, "a row a later write changes", "post-b").await;
    created(&suite, "a row a later write removes", "post-c").await;

    // The later writes have to land strictly after that moment for the delta to be able to
    // tell them apart, and the clock this emulator reads has a millisecond of resolution: the
    // change is made again until its own moment is past the one the sync will be given, which
    // is a condition rather than a wait.
    let changed = changed_after(&suite, "post-b", unchanged).await;
    let removed = posted(
        &suite.api,
        "the row a later write removes",
        &format!(
            "mutation M($id: ID!, $v: Int!) \
             {{ deletePost(id: $id, expectedVersion: $v) {SELECTION} }}"
        ),
        json!({"id": "post-c", "v": 1}),
    )
    .await;
    let removed_at = removed["data"]["deletePost"]["_lastChangedAt"]
        .as_i64()
        .unwrap_or_else(|| panic!("the removal must report the moment it happened: {removed}"));
    assert!(
        removed_at >= changed,
        "the removal must happen at or after the change before it, got {removed_at} and \
         {changed}"
    );

    // The sync of a client that holds nothing: every row of the table, and the moment the
    // sync started, which is what that client carries into its next one.
    let case = "a sync given no moment at all";
    let body = posted(&suite.api, case, &whole, json!({})).await;
    assert_eq!(
        errors(&body),
        Vec::<Value>::new(),
        "case {case:?}: a sync carries no errors, got {body}"
    );
    let answered = &body["data"]["syncPosts"];
    assert_eq!(
        ids(answered),
        vec!["post-a", "post-b", "post-c"],
        "case {case:?}: expected every row of the table, got {answered}"
    );
    let started_at = answered["startedAt"].as_i64().unwrap_or_else(|| {
        panic!("case {case:?}: expected the moment the sync started, got {answered}")
    });
    assert!(
        started_at >= removed_at,
        "case {case:?}: expected the sync to start at or after the last write ({removed_at}), \
         got {started_at}"
    );

    // The delta: the rows changed after the moment of the first row's own write — the
    // tombstone among them, marked deleted — and not the row that has not changed since.
    let case = "a sync given the moment of a known write";
    let body = posted(&suite.api, case, &since, json!({"since": unchanged})).await;
    let answered = &body["data"]["syncPosts"];
    assert_eq!(
        (errors(&body), ids(answered)),
        (Vec::<Value>::new(), vec!["post-b".to_string(), "post-c".to_string()]),
        "case {case:?}: expected only the rows changed after that moment, got {body}"
    );
    let tombstone = answered["items"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["id"] == json!("post-c")))
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        tombstone["_deleted"].as_bool(),
        Some(true),
        "case {case:?}: expected the removed row to travel in the delta marked deleted, got \
         {answered}"
    );

    // A moment past every write there is: nothing changed after it.
    let case = "a sync given a moment past every write";
    let body = posted(&suite.api, case, &since, json!({"since": started_at + 60_000})).await;
    let answered = &body["data"]["syncPosts"];
    assert_eq!(
        (errors(&body), ids(answered), answered["nextToken"].clone()),
        (Vec::<Value>::new(), Vec::<String>::new(), Value::Null),
        "case {case:?}: expected an empty page that continues nowhere, got {body}"
    );

    // And the page the sync answers is CONTINUED by the token it reports: one row at a time
    // reads every row of the table and no more.
    let case = "a sync read one row at a time";
    let paged = page(
        "limit: $limit, nextToken: $token",
        "($limit: Int!, $token: String)",
    );
    let mut token = Value::Null;
    let mut read: Vec<String> = Vec::new();
    for _ in 0..=3 {
        let body = posted(
            &suite.api,
            case,
            &paged,
            json!({"limit": 1, "token": token.clone()}),
        )
        .await;
        let answered = &body["data"]["syncPosts"];
        assert_eq!(
            errors(&body),
            Vec::<Value>::new(),
            "case {case:?}: every page carries no errors, got {body}"
        );
        read.extend(ids(answered));
        token = answered["nextToken"].clone();
        if token.is_null() {
            break;
        }
    }
    read.sort();
    assert_eq!(
        (read, token),
        (
            vec![
                "post-a".to_string(),
                "post-b".to_string(),
                "post-c".to_string()
            ],
            Value::Null
        ),
        "case {case:?}: expected the token to continue the sync to the last row and stop there"
    );
}

/// Each refusal is read from the row as it stands WHEN that refusal is made, so a caller that
/// retries a stale write after the row has moved on again is told what it has moved on to.
///
/// The rule the cases above assert over one refusal, asserted here over a SEQUENCE of them: the
/// row the conflict carries is not the row of the first refusal, nor a value the endpoint kept
/// from it, but the one the table holds at that moment.
#[tokio::test]
async fn every_refusal_carries_the_row_as_it_stands_when_it_is_made() {
    let suite = setup("live-refusals").await;
    let case = "a stale write refused twice, with an accepted write in between";
    let id = "post-live-refusal";
    seeded(&suite, case, id).await;

    let stale = format!(
        "mutation M($id: ID!, $title: String!, $v: Int!) \
         {{ updatePost(id: $id, title: $title, expectedVersion: $v) {SELECTION} }}"
    );
    // The row stands at version 2 under `v2`, and every one of these writes states version 1 —
    // a version the row has moved past — while the accepted writes between them move it on. So
    // each refusal has its own answer to carry, and a refusal carrying the row of an earlier
    // one is visible.
    let mut expected: Vec<(i64, String)> = Vec::new();
    for (title, version) in [("v3", 2), ("v4", 3)] {
        let refused = posted(
            &suite.api,
            case,
            &stale,
            json!({"id": id, "title": "stale", "v": 1}),
        )
        .await;
        let reported = errors(&refused);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly one field error, got {refused}"
        );
        let carried = &reported[0]["data"];
        expected.push((
            carried["_version"].as_i64().unwrap_or_default(),
            carried["title"].as_str().unwrap_or_default().to_string(),
        ));

        // The accepted write that moves the row on, stating the version it stands at.
        let moved = posted(
            &suite.api,
            case,
            &stale,
            json!({"id": id, "title": title, "v": version}),
        )
        .await;
        assert_eq!(
            (
                errors(&moved),
                moved["data"]["updatePost"]["_version"].as_i64()
            ),
            (Vec::<Value>::new(), Some(version + 1)),
            "case {case:?}: the write between the refusals is applied, got {moved}"
        );
    }
    assert_eq!(
        expected,
        vec![(2, "v2".to_string()), (3, "v3".to_string())],
        "case {case:?}: expected each refusal to carry the row as it stood when it was made"
    );
}

/// A version OTHER than the stored one is what makes a write a conflict, whichever side of the
/// stored one it falls on, and a conflict no handler resolves writes NOTHING.
///
/// The cases above assert the rule over a version the row has moved past by one; the rows here
/// span what the comparison generalises over — a version behind the stored one, one the item has
/// never reached, and the version no count ever holds — across the three writes that detection
/// covers and both configurations that reject one. Every row is made against the SAME item,
/// because a rejected write leaves the row untouched: what each refusal reads is the row the case
/// started from, down to the moment of its last accepted write.
#[tokio::test]
async fn any_version_other_than_the_stored_one_is_a_conflict_that_writes_nothing() {
    let suite = setup("other-versions").await;
    let case = "the row every rejected write is made against";
    let id = "post-other-version";
    seeded(&suite, case, id).await;
    // One accepted write over the version the row stands at, so the stored row carries an
    // attribute beside its title — a refusal has to carry the whole row, not the one member the
    // write was built around — and stands at a version the rows below state on either side of.
    let moved = posted(
        &suite.api,
        case,
        &format!(
            "mutation M($id: ID!, $field: String!, $value: String!, $v: Int!) \
             {{ mergePost(id: $id, field: $field, value: $value, expectedVersion: $v) \
             {SELECTION} }}"
        ),
        json!({"id": id, "field": "delta", "value": "server", "v": 2}),
    )
    .await;
    assert_eq!(
        (
            errors(&moved),
            moved["data"]["mergePost"]["_version"].as_i64()
        ),
        (Vec::<Value>::new(), Some(3)),
        "case {case:?}: the write the rows are made against must take the row to version 3, got \
         {moved}"
    );
    let before = stored_row(&suite.router, id).await;

    /// One stale write: what it is called, the field it drives, and the document and variables
    /// it is driven with.
    struct Stale<'a> {
        case: &'a str,
        field: &'a str,
        query: String,
        variables: Value,
    }
    let update = |field: &str| {
        format!(
            "mutation M($id: ID!, $title: String!, $v: Int!) \
             {{ {field}(id: $id, title: $title, expectedVersion: $v) {SELECTION} }}"
        )
    };
    let delete = |field: &str| {
        format!(
            "mutation M($id: ID!, $v: Int!) \
             {{ {field}(id: $id, expectedVersion: $v) {SELECTION} }}"
        )
    };
    let rows = [
        Stale {
            case: "an update stating the version just before the stored one",
            field: "updatePost",
            query: update("updatePost"),
            variables: json!({"title": "behind", "v": 2}),
        },
        Stale {
            case: "an update stating a version the item has never reached",
            field: "updatePost",
            query: update("updatePost"),
            variables: json!({"title": "ahead", "v": 9}),
        },
        Stale {
            case: "an update stating the version no count ever holds",
            field: "updatePost",
            query: update("updatePost"),
            variables: json!({"title": "none", "v": 0}),
        },
        Stale {
            case: "a put over a key that holds a row, stating a version it has moved past",
            field: "putPost",
            query: update("putPost"),
            variables: json!({"title": "put", "v": 1}),
        },
        Stale {
            case: "a delete stating the version just before the stored one",
            field: "deletePost",
            query: delete("deletePost"),
            variables: json!({"v": 2}),
        },
        Stale {
            case: "a delete AUTOMERGE cannot compose, stating a version ahead of the stored one",
            field: "deletePostMerging",
            query: delete("deletePostMerging"),
            variables: json!({"v": 9}),
        },
    ];
    for row in &rows {
        let case = row.case;
        let mut variables = row.variables.clone();
        variables["id"] = json!(id);
        let body = posted(&suite.api, case, &row.query, variables).await;
        let reported = errors(&body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly one field error, got {body}"
        );
        let error = &reported[0];
        assert_eq!(
            (
                error["errorType"].as_str(),
                error["message"].as_str(),
                error["path"].clone(),
                body["data"][row.field].clone(),
            ),
            (
                Some(CONFLICT_UNHANDLED),
                Some(CONFLICT_REJECTED),
                json!([row.field]),
                Value::Null,
            ),
            "case {case:?}: expected one {CONFLICT_UNHANDLED} error on the refused field, got \
             {body}"
        );
        // The refusal carries the row as the SERVER holds it: every attribute of it, at the
        // version actually stored.
        carries(
            case,
            "the refusal's own data",
            &error["data"],
            &json!({"id": id, "title": "v2", "delta": "server", "_version": 3}),
        );
        // And nothing was written: the row is the one the case started from, the moment of its
        // last accepted write included, with no deletion marker and no reaping marker.
        let held = stored_row(&suite.router, id).await;
        assert_eq!(
            held, before,
            "case {case:?}: expected the stored row left exactly as it was, got {held}"
        );
    }
}

/// Each write that merges is composed against the row as it stands when ITS turn comes, so what
/// it is answered is the row that turn committed.
///
/// The case above asserts the versions concurrent merges are handed; the rule here is what each
/// of them READS back: the writer stamped the Nth version after the one the item was taken to
/// carries exactly the N merged attributes committed up to and including its own — never a
/// partial view of a shared base, and never an attribute a later turn added. The rows differ in
/// how many merges are in flight at once.
#[tokio::test]
async fn every_merge_in_flight_answers_the_row_its_own_turn_committed() {
    let suite = setup("committed-merges").await;
    let query = format!(
        "mutation M($id: ID!, $field: String!, $value: String!, $v: Int!) \
         {{ mergePost(id: $id, field: $field, value: $value, expectedVersion: $v) \
         {SELECTION} }}"
    );
    // Every writer states the version the row has already moved past, so each of them is a
    // conflict the resolver's AUTOMERGE composes, and each composes a DIFFERENT attribute, so
    // which merges one answer carries is visible in it.
    let writer = |case: &'static str, id: &'static str, field: &'static str| {
        let query = query.clone();
        let api = &suite.api;
        async move {
            let body = posted(
                api,
                case,
                &query,
                json!({"id": id, "field": field, "value": field, "v": 1}),
            )
            .await;
            (field, body)
        }
    };

    let two = "two merges of one item in flight at once";
    let pair = "post-committed-pair";
    seeded(&suite, two, pair).await;
    let (first, second) = tokio::join!(writer(two, pair, "alpha"), writer(two, pair, "beta"));
    committed(&suite, two, pair, &[first, second]).await;

    let three = "three merges of one item in flight at once";
    let trio = "post-committed-trio";
    seeded(&suite, three, trio).await;
    let (first, second, third) = tokio::join!(
        writer(three, trio, "alpha"),
        writer(three, trio, "beta"),
        writer(three, trio, "gamma"),
    );
    committed(&suite, three, trio, &[first, second, third]).await;
}

/// Assert what the merges in flight at once over one item were answered: consecutive versions
/// after the one the item was seeded at, one each, and each answer the row that writer's own turn
/// committed — the merges accepted before it and its own, and nothing of the turns that followed.
///
/// `answered` pairs each writer's own attribute with the document it was served.
async fn committed(suite: &Suite, case: &str, id: &str, answered: &[(&str, Value)]) {
    /// The version [`seeded`] leaves the item at, which every merge here is counted from.
    const SEEDED_VERSION: i64 = 2;

    let composed: Vec<&str> = answered.iter().map(|(field, _)| *field).collect();
    let mut versions: Vec<i64> = Vec::new();
    for (field, body) in answered {
        assert_eq!(
            errors(body),
            Vec::<Value>::new(),
            "case {case:?}: every merged write is accepted, got {body}"
        );
        let row = &body["data"]["mergePost"];
        let version = row["_version"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a version, got {body}"));
        versions.push(version);
        let carried: Vec<&str> = composed
            .iter()
            .copied()
            .filter(|name| !row[*name].is_null())
            .collect();
        let turns = usize::try_from(version - SEEDED_VERSION).unwrap_or_default();
        assert_eq!(
            (row["title"].as_str(), row[*field].as_str(), carried.len()),
            (Some("v2"), Some(*field), turns),
            "case {case:?}: the writer of {field:?} stamped version {version} must be answered \
             the {turns} merges its own turn committed, got {row} carrying {carried:?}"
        );
    }
    versions.sort_unstable();
    let expected: Vec<i64> = (1..=i64::try_from(answered.len()).unwrap_or_default())
        .map(|at| SEEDED_VERSION + at)
        .collect();
    assert_eq!(
        versions, expected,
        "case {case:?}: expected each accepted merge to advance the version once, got \
         {versions:?} from {answered:?}"
    );

    // And the row the TABLE holds is where those turns left it: every attribute they composed,
    // the server's own title, and the last version handed out.
    let held = stored_row(&suite.router, id).await;
    for field in &composed {
        assert_eq!(
            held[*field]["S"].as_str(),
            Some(*field),
            "case {case:?}: expected the stored row to carry every merged attribute, got {held}"
        );
    }
    let last = expected.last().copied().unwrap_or_default().to_string();
    assert_eq!(
        (held["_version"]["N"].as_str(), held["title"]["S"].as_str()),
        (Some(last.as_str()), Some("v2")),
        "case {case:?}: expected the stored row at version {last} under the server's own title, \
         got {held}"
    );
}

/// The ids the page `answered` carries, in NAME order.
///
/// A delta sync reads the table, and a table read reports no order of its own, so what a case
/// asserts is WHICH rows a page carries: the ids are ordered here so the expectation does not
/// rest on an order no rule gives.
fn ids(answered: &Value) -> Vec<String> {
    let mut carried: Vec<String> = answered["items"]        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item["id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    carried.sort();
    carried
}

/// Assert that `answered` carries every member `expected` states, naming the case and what was
/// answered. The row carries its own system attributes beside them, so this is a containment.
fn carries(case: &str, what: &str, answered: &Value, expected: &Value) {
    for (member, value) in expected.as_object().into_iter().flatten() {
        assert_eq!(
            &answered[member], value,
            "case {case:?}: expected {what} to carry {member} as {value}, got {answered}"
        );
    }
}

/// Create the item `id` and answer the moment its creation was stamped with, asserting it came
/// back as the first version of that item.
async fn created(suite: &Suite, case: &str, id: &str) -> i64 {
    let body = posted(
        &suite.api,
        case,
        &format!(
            "mutation M($id: ID!, $title: String!) \
             {{ createPost(id: $id, title: $title) {SELECTION} }}"
        ),
        json!({"id": id, "title": "v1"}),
    )
    .await;
    let answered = &body["data"]["createPost"];
    assert_eq!(
        (errors(&body), answered["_version"].as_i64()),
        (Vec::<Value>::new(), Some(1)),
        "case {case:?}: the write that creates an item stamps its first version, got {body}"
    );
    answered["_lastChangedAt"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: expected the moment of the write, got {body}"))
}

/// Take the item `id` to version 2 under the title `v2`: the state every conflict case is
/// written against, where version 1 is a version the row has moved past.
async fn seeded(suite: &Suite, case: &str, id: &str) {
    created(suite, case, id).await;
    let body = posted(
        &suite.api,
        case,
        &format!(
            "mutation M($id: ID!, $title: String!, $v: Int!) \
             {{ updatePost(id: $id, title: $title, expectedVersion: $v) {SELECTION} }}"
        ),
        json!({"id": id, "title": "v2", "v": 1}),
    )
    .await;
    assert_eq!(
        (errors(&body), body["data"]["updatePost"]["_version"].as_i64()),
        (Vec::<Value>::new(), Some(2)),
        "case {case:?}: the seeding write must take the row to version 2, got {body}"
    );
}

/// Change the item `id` until the moment its change is stamped with is strictly past `moment`,
/// and answer that moment.
///
/// The emulator reads the clock in milliseconds, so a change made in the same millisecond as
/// `moment` carries that very moment and is not "after" it. The condition — not a wait — is
/// that the change is stamped later, so the change is made again until it is; the version it
/// leaves behind is read from what each write answers rather than assumed.
async fn changed_after(suite: &Suite, id: &str, moment: i64) -> i64 {
    let query = format!(
        "mutation M($id: ID!, $title: String!, $v: Int!) \
         {{ updatePost(id: $id, title: $title, expectedVersion: $v) {SELECTION} }}"
    );
    let case = "a change stamped after a known moment";
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    let mut version = 1;
    loop {
        let body = posted(
            &suite.api,
            case,
            &query,
            json!({"id": id, "title": "changed", "v": version}),
        )
        .await;
        let answered = &body["data"]["updatePost"];
        version = answered["_version"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a version, got {body}"));
        let changed_at = answered["_lastChangedAt"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a moment, got {body}"));
        if changed_at > moment {
            return changed_at;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: a change must eventually be stamped after {moment}, last {body}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// POST one GraphQL document to the address the API published, as a client does, and answer the
/// document it was served — asserting it was answered 200, whatever it carries.
async fn posted(api: &GraphqlApi, case: &str, query: &str, variables: Value) -> Value {
    let (status, body) = post_graphql(
        case,
        api,
        &json!({"query": query, "variables": variables}),
        &[("x-api-key", api.api_key.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a GraphQL request is answered 200, got {status} and {body}"
    );
    body
}

/// The errors one answer reports, which is the empty list for an answer that reports none.
fn errors(body: &Value) -> Vec<Value> {
    body["errors"].as_array().cloned().unwrap_or_default()
}

/// The row the TABLE holds for `id`, through DynamoDB's own reader — so a write that did not
/// reach it cannot pass.
async fn stored_row(router: &MultiServiceRouter, id: &str) -> Value {
    let read = dynamodb(
        router,
        "GetItem",
        json!({
            "TableName": TABLE,
            "Key": {KEY_ATTRIBUTE: {"S": id}},
            "ConsistentRead": true,
        }),
    )
    .await;
    read["Item"].clone()
}

/// Everything one case drives: the router the services are mounted on and the API whose socket
/// the cases POST to.
struct Suite {
    router: MultiServiceRouter,
    api: GraphqlApi,
}

/// Mount the services, create the table, the role and the API with its versioned data source
/// and every resolver, and answer what a case drives.
async fn setup(name: &str) -> Suite {
    let router = emulator();
    let table_arn = create_table(&router).await;
    let role = create_role(
        &router,
        &format!("appsync-versioned-{name}-role"),
        &table_arn,
    )
    .await;
    let api = create_api(&router, name, &role).await;
    Suite { router, api }
}

/// A router with the services the topology spans, over ONE engine: DynamoDB holds the table,
/// IAM and STS the data source's role, CloudWatch the API's own series.
fn emulator() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService::new(engine),
    );
    router
}

/// Create the table the data source resolves against through DynamoDB's own API, wait until
/// that service first reports it `ACTIVE`, and answer the ARN it holds it under.
async fn create_table(router: &MultiServiceRouter) -> String {
    let created = dynamodb(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": KEY_ATTRIBUTE, "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let arn = created["TableDescription"]["TableArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !arn.is_empty(),
        "setup: CreateTable must report the ARN the table is held under, got {created}"
    );
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = dynamodb(router, "DescribeTable", json!({"TableName": TABLE})).await;
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") {
            return arn;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "setup: the table must become ACTIVE, last read {described}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// Create the API with [`SCHEMA`], an API key, the VERSIONED data source reached under
/// `role_arn` and one resolver per field, and answer what a case drives it through.
async fn create_api(router: &MultiServiceRouter, name: &str, role_arn: &str) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("versioned-posts-{name}"),
            "authenticationType": "API_KEY",
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api_id = created["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(
        &created["graphqlApi"],
        "GRAPHQL",
    );
    assert!(
        graphql.starts_with(&format!("{LOOPBACK_HOST}:")),
        "uris.GRAPHQL must be published on an address this emulator serves, got {graphql:?}"
    );
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    install_schema(router, &api_id, SCHEMA).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({
            "name": DATA_SOURCE,
            "type": "AMAZON_DYNAMODB",
            "serviceRoleArn": role_arn,
            "dynamodbConfig": {
                "tableName": TABLE,
                "awsRegion": REGION,
                // "Set to TRUE to use Conflict Detection and Resolution with this data
                // source" (`DynamodbDataSourceConfig$versioned`).
                "versioned": true,
                "deltaSyncConfig": {
                    "baseTableTTL": BASE_TABLE_TTL_MINUTES,
                    "deltaSyncTableName": TABLE,
                    "deltaSyncTableTTL": BASE_TABLE_TTL_MINUTES,
                },
            },
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    for resolver in resolvers() {
        let mut request = json!({
            "fieldName": resolver.field,
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "requestMappingTemplate": resolver.request,
            // Every resolver shares the response half: it re-raises what the data source
            // answered, the row the server holds included, as the documented one does.
            "responseMappingTemplate": RESPONSE_TEMPLATE,
        });
        if let Some(sync_config) = resolver.sync_config {
            request["syncConfig"] = sync_config;
        }
        let (status, created) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/{}/resolvers", resolver.type_name),
            Some(request),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {}.{} must answer 200, got {created}",
            resolver.type_name, resolver.field
        );
    }
    GraphqlApi {
        api_id,
        graphql,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// One resolver of the suite: the field it is attached to, the request mapping template it is
/// defined by, and the `syncConfig` it carries — `None` for a resolver that asks for no
/// conflict handling at all.
struct Resolver {
    type_name: &'static str,
    field: &'static str,
    request: &'static str,
    sync_config: Option<Value>,
}

/// The response half every resolver of the suite shares: a step whose data source did not
/// answer is re-raised as the error of the field — its sentence, its class and the value it
/// carried, which for an unresolved conflict is the row as the server holds it — and every
/// other step answers what the call came to.
const RESPONSE_TEMPLATE: &str = "#if($ctx.error)\n\
     $util.error($ctx.error.message, $ctx.error.type, $ctx.result)\n\
     #end\n\
     $util.toJson($ctx.result)";

/// Every resolver the suite's API carries: one per field of [`SCHEMA`], each driving one
/// operation of the data source, and each with the conflict configuration its cases need.
fn resolvers() -> Vec<Resolver> {
    // "VERSION: Detect conflicts based on object versions" and "NONE", and the handler that
    // "resolve[s] conflicts by rejecting mutations when versions don't match the latest
    // version at the server" beside the one that merges
    // (`smithy_query(query="shape",shape="ConflictDetectionType,ConflictHandlerType")`).
    let rejecting = || {
        Some(json!({"conflictDetection": "VERSION", "conflictHandler": "OPTIMISTIC_CONCURRENCY"}))
    };
    let merging = || Some(json!({"conflictDetection": "VERSION", "conflictHandler": "AUTOMERGE"}));
    let undetected = || {
        Some(json!({"conflictDetection": "NONE", "conflictHandler": "OPTIMISTIC_CONCURRENCY"}))
    };
    vec![
        Resolver {
            type_name: "Query",
            field: "getPost",
            request: GET,
            sync_config: None,
        },
        Resolver {
            type_name: "Query",
            field: "syncPosts",
            request: SYNC,
            sync_config: None,
        },
        Resolver {
            type_name: "Mutation",
            field: "createPost",
            request: PUT,
            sync_config: rejecting(),
        },
        Resolver {
            type_name: "Mutation",
            field: "putPost",
            request: PUT_VERSIONED,
            sync_config: rejecting(),
        },
        Resolver {
            type_name: "Mutation",
            field: "putPostIfAbsent",
            request: PUT_CONDITIONAL,
            sync_config: rejecting(),
        },
        Resolver {
            type_name: "Mutation",
            field: "putPostMerging",
            request: PUT_FIELD,
            sync_config: merging(),
        },
        Resolver {
            type_name: "Mutation",
            field: "updatePost",
            request: UPDATE_TITLE,
            sync_config: rejecting(),
        },
        Resolver {
            type_name: "Mutation",
            field: "mergePost",
            request: UPDATE_FIELD,
            sync_config: merging(),
        },
        Resolver {
            type_name: "Mutation",
            field: "updatePostUndetected",
            request: UPDATE_TITLE,
            sync_config: undetected(),
        },
        Resolver {
            type_name: "Mutation",
            field: "updatePostUnconfigured",
            request: UPDATE_TITLE,
            sync_config: None,
        },
        Resolver {
            type_name: "Mutation",
            field: "deletePost",
            request: DELETE,
            sync_config: rejecting(),
        },
        Resolver {
            type_name: "Mutation",
            field: "deletePostMerging",
            request: DELETE,
            sync_config: merging(),
        },
    ]
}

/// The request object the write that CREATES an item returns: the item it writes, and no
/// version — the caller claims nothing about a key it believes holds nothing.
const PUT: &str = r#"{
  "version": "2018-05-29",
  "operation": "PutItem",
  "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)},
  "attributeValues": {"title": $util.dynamodb.toDynamoDBJson($ctx.args.title)}
}"#;

/// The same put, stating the version the caller believes the key stands at.
const PUT_VERSIONED: &str = r#"{
  "version": "2018-05-29",
  "operation": "PutItem",
  "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)},
  "attributeValues": {"title": $util.dynamodb.toDynamoDBJson($ctx.args.title)},
  "_version": $ctx.args.expectedVersion
}"#;

/// The same put, stating the grounds of its own refusal beside the version: the row it writes
/// must not be there, which is how a create that must not overwrite is written.
const PUT_CONDITIONAL: &str = r##"{
  "version": "2018-05-29",
  "operation": "PutItem",
  "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)},
  "attributeValues": {"title": $util.dynamodb.toDynamoDBJson($ctx.args.title)},
  "condition": {"expression": "attribute_not_exists(id)"},
  "_version": $ctx.args.expectedVersion
}"##;

/// The same put carrying one more attribute, which the field names: what a merge composes into
/// the stored row.
const PUT_FIELD: &str = r#"{
  "version": "2018-05-29",
  "operation": "PutItem",
  "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)},
  "attributeValues": {
    "title": $util.dynamodb.toDynamoDBJson($ctx.args.title),
    "$ctx.args.field": $util.dynamodb.toDynamoDBJson($ctx.args.value)
  },
  "_version": $ctx.args.expectedVersion
}"#;

/// The update that changes the title of the row the caller read.
const UPDATE_TITLE: &str = r##"{
  "version": "2018-05-29",
  "operation": "UpdateItem",
  "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)},
  "update": {
    "expression": "SET #t = :t",
    "expressionNames": {"#t": "title"},
    "expressionValues": {":t": $util.dynamodb.toDynamoDBJson($ctx.args.title)}
  },
  "_version": $ctx.args.expectedVersion
}"##;

/// The update that sets the ONE attribute the field names, so two writers can change disjoint
/// attributes of one row.
const UPDATE_FIELD: &str = r##"{
  "version": "2018-05-29",
  "operation": "UpdateItem",
  "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)},
  "update": {
    "expression": "SET #f = :v",
    "expressionNames": {"#f": "$ctx.args.field"},
    "expressionValues": {":v": $util.dynamodb.toDynamoDBJson($ctx.args.value)}
  },
  "_version": $ctx.args.expectedVersion
}"##;

/// The delete of the row the caller read.
const DELETE: &str = r#"{
  "version": "2018-05-29",
  "operation": "DeleteItem",
  "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)},
  "_version": $ctx.args.expectedVersion
}"#;

/// The read of one row.
const GET: &str = r#"{
  "version": "2018-05-29",
  "operation": "GetItem",
  "key": {"id": $util.dynamodb.toDynamoDBJson($ctx.args.id)}
}"#;

/// The DELTA SYNC: the moment the caller last synced at, how many rows it reads and where the
/// page before it left off. Each is rendered as the JSON of the argument the field was given,
/// so an argument the caller did not give is stated as `null` — which states nothing.
const SYNC: &str = r#"{
  "version": "2018-05-29",
  "operation": "Sync",
  "lastSync": $util.toJson($ctx.args.lastSync),
  "limit": $util.toJson($ctx.args.limit),
  "nextToken": $util.toJson($ctx.args.nextToken)
}"#;
