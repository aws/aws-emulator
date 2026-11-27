// SPDX-License-Identifier: CC0-1.0

//! UNIT resolvers over an `AMAZON_DYNAMODB` data source whose mapping templates are written
//! with the MARSHALLING helpers AppSync documents for that data source, over a table keyed by a
//! partition AND a sort key, driven over the API's own published `GRAPHQL` endpoint.
//!
//! The rules these cases assert, one level above the recorded ones:
//!
//! > 1. `$util.dynamodb.toDynamoDB(<value>)` renders a value as the ATTRIBUTE VALUE DynamoDB
//! >    holds it as — the one-member object naming the type the value itself has — and
//! >    `$util.dynamodb.toDynamoDBJson(<value>)` renders that same representation already
//! >    serialized, so a template can inline it into the request object it is writing. Each is
//! >    a VALUE the template renders; neither is a failure of the field, so a template written
//! >    with them resolves its field by making the call its request object names.
//! > 2. A `PutItem` request object REPLACES the item it addresses: the item the table holds
//! >    afterwards carries exactly the key and the `attributeValues` that request named, so an
//! >    attribute a later write omits is gone from the stored item rather than left behind.
//! > 3. A `Query` request object reads one partition in SORT-KEY order — descending where it
//! >    says so — `limit` bounds the page it answers and the opaque `nextToken` beside it
//! >    resumes the selection where that page ended, so walking the token reads every row of
//! >    the partition exactly once and stops. A `nextToken` the endpoint did not mint is
//! >    refused as an illegal ARGUMENT of the field, with the one fixed sentence stated for a
//! >    token that cannot be read and nothing of the value carried.
//! > 4. A filter stated as an OBJECT — one entry per attribute naming the operators it is
//! >    compared under, grouped by `and`, `or` and `not` — is transformed by
//! >    `$util.transform.toDynamoDBFilterExpression(<filter>)` into the filter expression the
//! >    request object's `filter` member carries, so the selection answers the rows of the
//! >    partition that filter holds over and no others.
//!
//! The rows span what those rules generalise over: the type of the value the general converter
//! is handed (the sort key is a string on one table and a number on the other, and a number
//! marshalled as text would not even be a key of that table), the attribute a write omits, the
//! page size a selection is cut at (one row, an uneven cut, exactly the count, more than the
//! partition holds, and none at all), the direction it is read in, the partition it selects,
//! which of a filter's grouped comparisons holds, and the forms a value that is no token of
//! this endpoint takes.
//!
//! Nothing here is asserted off this service's own store: the rows are read back both through
//! the API's own reader field and through Amazon DynamoDB's own `GetItem`, so a write that was
//! never made cannot pass.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::appsync;
use crate::graphql_api_iam_realtime_dynamodb_flow::{create_role, dynamodb};
use crate::graphql_api_mapping_template_resolvers::{install_schema, try_post_answer};

/// The Region every call of this suite is signed for, which is also the Region its API and its
/// tables are created in.
const REGION: &str = "us-west-2";

/// The path the API's GraphQL endpoint is served at.
const GRAPHQL_PATH: &str = "/graphql";

/// The two data sources of this suite, one per table.
const ITEMS_DATA_SOURCE: &str = "items";
const RANKED_DATA_SOURCE: &str = "ranked";

/// The partition every case reads, and a second one that holds a row of its own: a selection of
/// the one must never carry a row of the other.
const OWNER: &str = "ada";
const OTHER_OWNER: &str = "grace";

/// How long setup waits for a state the clock owes, and how often it looks again. Generous:
/// what is under test is what the endpoint answers, never how fast the host gets there.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(100);

/// The class AppSync reports an illegal ARGUMENT of a request under, which is what a field
/// carrying a pagination token that cannot be read answers with.
const ILLEGAL_ARGUMENT: &str = "IllegalArgument";

/// The schema the API of this suite is created with: one type per table, a page type per
/// selection, and the fields the resolvers are attached to.
const SCHEMA: &str = "schema { query: Query mutation: Mutation }\n\
     type Item {\n  owner: String\n  sk: String\n  name: String\n  note: String\n}\n\
     type Ranked {\n  owner: String\n  rank: Int\n  name: String\n  note: String\n}\n\
     type ItemPage {\n  items: [Item]\n  nextToken: String\n}\n\
     type RankedPage {\n  items: [Ranked]\n  nextToken: String\n}\n\
     type Query {\n  \
     getItem(owner: String!, sk: String!): Item\n  \
     getRanked(owner: String!, rank: Int!): Ranked\n  \
     listItems(owner: String!, limit: Int, nextToken: String, forward: Boolean): ItemPage\n  \
     listRanked(owner: String!, limit: Int, nextToken: String, forward: Boolean): RankedPage\n  \
     findItems(owner: String!, named: String!, prefix: String!): ItemPage\n  \
     findRanked(owner: String!, named: String!, prefix: String!): RankedPage\n}\n\
     type Mutation {\n  \
     putItem(owner: String!, sk: String!, name: String, note: String): Item\n  \
     putRanked(owner: String!, rank: Int!, name: String, note: String): Ranked\n}\n";

/// The sort keys the pagination cases write, in an order that is NOT their sort-key order, so a
/// page in sort-key order cannot be the order the rows were written in.
const WRITTEN_SORT_KEYS: [&str; 5] = ["s-3", "s-1", "s-5", "s-2", "s-4"];

/// The ranks the numeric table's cases write. Their sort-key order is NUMERIC (`1, 2, 4, 10,
/// 33`) and not the order their text sorts in (`1, 10, 2, 33, 4`), so a page in sort-key order
/// also pins that the general converter marshalled each of them as the NUMBER it is.
const WRITTEN_RANKS: [i64; 5] = [10, 2, 33, 1, 4];

/// The prefix every seeded row's `name` carries, which is what a filter selects a row, every
/// row or no row of the partition by.
const NAME_PREFIX: &str = "n-";

/// RULE 1 and 2: a `PutItem` request object written with the marshalling helpers resolves its
/// field, and what it writes REPLACES the item it addresses — the attribute a later write omits
/// is gone from the stored row.
#[tokio::test]
async fn a_put_item_resolver_written_with_the_marshalling_helpers_replaces_the_item() {
    let suite = setup("replace").await;

    for kind in kinds() {
        let case = format!("a full replacement under {}", kind.case);
        let key = kind.first_key();

        // The first write names both attributes, and answers the row it wrote.
        let written = post(
            &suite,
            &case,
            &kind.write_document(true),
            kind.arguments(&key, "first", Some("kept")),
        )
        .await;
        assert_eq!(
            (written.get("errors"), written.pointer(&kind.write_pointer())),
            (None, Some(&kind.row(&key, "first", Some("kept")))),
            "case {case:?}: the mutation must answer the row it wrote, got {written}"
        );

        // The API's own reader reads it back, each value as the value it holds rather than as
        // the attribute value the table holds it as.
        let read = post(&suite, &case, &kind.read_document(), kind.key_arguments(&key)).await;
        assert_eq!(
            (read.get("errors"), read.pointer(&kind.read_pointer())),
            (None, Some(&kind.row(&key, "first", Some("kept")))),
            "case {case:?}: the query must read back the row that was written, got {read}"
        );

        // And so does DynamoDB's own reader, which is where the row really is.
        let held = suite.stored(&kind, &key).await;
        assert_eq!(
            held.get("note"),
            Some(&json!({"S": "kept"})),
            "case {case:?}: the table must hold the attribute the write named, got {held:?}"
        );

        // The second write OMITS one attribute: the row it leaves behind carries exactly what
        // it named.
        let replaced = post(
            &suite,
            &case,
            &kind.write_document(false),
            kind.arguments(&key, "second", None),
        )
        .await;
        assert_eq!(
            (replaced.get("errors"), replaced.pointer(&kind.write_pointer())),
            (None, Some(&kind.row(&key, "second", None))),
            "case {case:?}: the replacing mutation must answer the row it wrote, got {replaced}"
        );
        let read = post(&suite, &case, &kind.read_document(), kind.key_arguments(&key)).await;
        assert_eq!(
            (read.get("errors"), read.pointer(&kind.read_pointer())),
            (None, Some(&kind.row(&key, "second", None))),
            "case {case:?}: the omitted attribute must be gone from the row that is read back, \
             got {read}"
        );
        let held = suite.stored(&kind, &key).await;
        assert_eq!(
            (held.get("name"), held.get("note")),
            (Some(&json!({"S": "second"})), None),
            "case {case:?}: the stored item must carry exactly the attributes the last write \
             named, got {held:?}"
        );
    }
}

/// RULE 3: a `Query` request object reads ONE partition in sort-key order, `limit` cuts the page
/// and `nextToken` resumes it — whatever page size the selection is cut at.
#[tokio::test]
async fn a_query_resolver_pages_one_partition_in_sort_key_order() {
    let suite = setup("pages").await;

    for kind in kinds() {
        suite.seed(&kind).await;
        let sorted = kind.sorted_keys();
        // The page sizes span a page per row, an uneven cut, exactly the count the partition
        // holds, more than it holds, and no limit at all.
        for limit in [Some(1), Some(2), Some(3), Some(5), Some(9), None] {
            let case = format!("{} read at a limit of {limit:?}", kind.case);
            let (read, pages) = suite.walk(&case, &kind, OWNER, limit, None).await;
            assert_eq!(
                read, sorted,
                "case {case:?}: expected every row of the partition once, in sort-key order, \
                 got {read:?}"
            );
            // The limit CUT the selection: the rows took as many pages as that size needs —
            // one more where the last page ended exactly on the limit, which is a page the
            // selection is continued into before it is known to be empty.
            let cut = limit.map_or(1, |limit| read.len().div_ceil(limit));
            assert!(
                (cut..=cut + 1).contains(&pages),
                "case {case:?}: expected the selection to take about {cut} pages, took {pages}"
            );
        }

        // Read the other way, the page carries the same rows in the reverse of that order.
        let case = format!("{} read in the other direction", kind.case);
        let mut descending = sorted.clone();
        descending.reverse();
        let (read, _) = suite.walk(&case, &kind, OWNER, Some(2), Some(false)).await;
        assert_eq!(
            read, descending,
            "case {case:?}: expected the rows in descending sort-key order, got {read:?}"
        );

        // The selection is of ONE partition: the row another partition holds is never in it.
        let case = format!("{} of another partition", kind.case);
        let (read, _) = suite.walk(&case, &kind, OTHER_OWNER, None, None).await;
        assert_eq!(
            read,
            vec![kind.other_key()],
            "case {case:?}: expected only the row that partition holds, got {read:?}"
        );
    }
}

/// RULE 1 over the FILTER a selection applies: a filter stated as an object is transformed into
/// the filter expression the request object's own member carries, so the selection answers the
/// rows of the partition that filter holds over — in sort-key order, as every selection is read.
#[tokio::test]
async fn a_query_resolver_filters_the_partition_by_the_transformed_filter_object() {
    let suite = setup("filters").await;

    for kind in kinds() {
        suite.seed(&kind).await;
        let sorted = kind.sorted_keys();
        let name_of = |at: usize| kind.row_name(&sorted[at]);
        // The rows span what the one filter generalises over: which of its grouped comparisons
        // holds — the equality alone, the prefix alone over every row, and both at once — so a
        // filter that was dropped, inverted or applied to one comparison only cannot pass.
        let rows: Vec<(String, String, String, Vec<Value>)> = vec![
            (
                "the equality alone".to_string(),
                name_of(0),
                "zzz".to_string(),
                vec![sorted[0].clone()],
            ),
            (
                "the prefix over every row".to_string(),
                "zzz".to_string(),
                NAME_PREFIX.to_string(),
                sorted.clone(),
            ),
            (
                "both comparisons at once".to_string(),
                name_of(0),
                name_of(4),
                vec![sorted[0].clone(), sorted[4].clone()],
            ),
            (
                "neither comparison".to_string(),
                "zzz".to_string(),
                "zzz".to_string(),
                Vec::new(),
            ),
        ];

        for (form, named, prefix, expected) in rows {
            let case = format!("{} filtered by {form}", kind.case);
            let answered = post(
                &suite,
                &case,
                &kind.find_document(),
                json!({"owner": OWNER, "named": named, "prefix": prefix}),
            )
            .await;
            let page = answered
                .pointer(&kind.find_pointer())
                .cloned()
                .unwrap_or(Value::Null);
            let read: Vec<Value> = page["items"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|item| item[kind.key_member].clone())
                .collect();
            assert_eq!(
                (answered.get("errors"), read),
                (None, expected.clone()),
                "case {case:?}: expected the rows the filter holds over, in sort-key order, got \
                 {answered}"
            );
        }
    }
}

/// RULE 3: a `nextToken` this endpoint did not mint is refused as an illegal ARGUMENT of the
/// field — the field resolves to nothing, the response is still `200`, and the one error names
/// the field's path, that class and the fixed sentence, with nothing of the value carried.
#[tokio::test]
async fn a_query_resolver_refuses_a_pagination_token_it_did_not_mint() {
    use base64::Engine as _;

    let suite = setup("tokens").await;
    let encode = |payload: &str| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.as_bytes())
    };
    // The forms a value that is no token of this endpoint takes.
    let carried: Vec<(&str, String)> = vec![
        ("text of no payload at all", "not-a-token".to_string()),
        ("the empty token", String::new()),
        ("a payload that decodes to no document", encode("{not json")),
        ("a payload that decodes to a document that is no key", encode("[1, 2]")),
        ("a payload that decodes to text", encode("\"s-1\"")),
    ];

    for kind in kinds() {
        suite.seed(&kind).await;
        // The same selection WITHOUT a token is served first, so what the refusal turns on is
        // the value carried and not the field.
        let case = format!("{} warmed up", kind.case);
        let (warmed, _) = suite.walk(&case, &kind, OWNER, Some(2), None).await;
        assert_eq!(
            warmed,
            kind.sorted_keys(),
            "case {case:?}: the selection must be served before a token is carried, got \
             {warmed:?}"
        );

        for (form, token) in &carried {
            let case = format!("{} carrying {form}", kind.case);
            let answered = post(
                &suite,
                &case,
                &kind.list_document(),
                json!({"owner": OWNER, "limit": 2, "token": token, "forward": Value::Null}),
            )
            .await;
            let reported = answered["errors"].as_array().cloned().unwrap_or_default();
            assert_eq!(
                (
                    answered.pointer(&kind.list_pointer()),
                    reported.len(),
                    reported.first().and_then(|error| error["errorType"].as_str()),
                    reported.first().and_then(|error| error["message"].as_str()),
                    reported.first().map(|error| error["path"].clone()),
                ),
                (
                    Some(&Value::Null),
                    1,
                    Some(ILLEGAL_ARGUMENT),
                    Some(state::INVALID_PAGINATION_TOKEN_MESSAGE),
                    Some(json!([kind.list])),
                ),
                "case {case:?}: expected one illegal-argument error of the field, got {answered}"
            );
            let stated = reported
                .first()
                .and_then(|error| error["message"].as_str())
                .unwrap_or_default()
                .to_string();
            assert!(
                token.is_empty() || !stated.contains(token.as_str()),
                "case {case:?}: the refusal must carry nothing of the value, got {stated:?}"
            );
        }
    }
}

/// One field family of this suite: the fields over one table, the member its sort key is
/// carried under, and the GraphQL type of that member.
struct Kind {
    /// What this family's rows are called in a case name.
    case: &'static str,
    put: &'static str,
    get: &'static str,
    list: &'static str,
    find: &'static str,
    /// The member the sort key is carried under, which is also the table's sort-key attribute.
    key_member: &'static str,
    /// The GraphQL type of that member, as the operation declares its variable.
    key_type: &'static str,
    /// The attribute type DynamoDB holds that key under.
    key_attribute: &'static str,
    /// The table this family resolves against.
    table: &'static str,
}

/// The two field families, one per table: the sort key is a string on one and a number on the
/// other, which is what the general converter has to pick the attribute type from.
fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            case: "a string sort key",
            put: "putItem",
            get: "getItem",
            list: "listItems",
            find: "findItems",
            key_member: "sk",
            key_type: "String!",
            key_attribute: "S",
            table: ITEMS_TABLE,
        },
        Kind {
            case: "a numeric sort key",
            put: "putRanked",
            get: "getRanked",
            list: "listRanked",
            find: "findRanked",
            key_member: "rank",
            key_type: "Int!",
            key_attribute: "N",
            table: RANKED_TABLE,
        },
    ]
}

impl Kind {
    /// Every sort key this family writes, in the order it writes them.
    fn written_keys(&self) -> Vec<Value> {
        if self.key_attribute == "N" {
            WRITTEN_RANKS.iter().map(|rank| json!(rank)).collect()
        } else {
            WRITTEN_SORT_KEYS.iter().map(|key| json!(key)).collect()
        }
    }

    /// Those same keys in the order the table holds them: ascending sort-key order.
    fn sorted_keys(&self) -> Vec<Value> {
        if self.key_attribute == "N" {
            let mut ranks = WRITTEN_RANKS;
            ranks.sort_unstable();
            ranks.iter().map(|rank| json!(rank)).collect()
        } else {
            let mut keys = WRITTEN_SORT_KEYS;
            keys.sort_unstable();
            keys.iter().map(|key| json!(key)).collect()
        }
    }

    /// The one key the replacement case addresses.
    fn first_key(&self) -> Value {
        self.written_keys().remove(0)
    }

    /// The key of the row the OTHER partition holds.
    fn other_key(&self) -> Value {
        if self.key_attribute == "N" {
            json!(77)
        } else {
            json!("s-other")
        }
    }

    /// The key as DynamoDB's own reader is handed it.
    fn wire_key(&self, owner: &str, key: &Value) -> Value {
        let held = match key {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        json!({
            "owner": {"S": owner},
            self.key_member: {self.key_attribute: held},
        })
    }

    /// The row this family's fields answer for one key: every member the selection asks for.
    fn row(&self, key: &Value, name: &str, note: Option<&str>) -> Value {
        json!({
            "owner": OWNER,
            self.key_member: key.clone(),
            "name": name,
            "note": note.map_or(Value::Null, |note| json!(note)),
        })
    }

    /// The mutation document, with the `note` argument where the case names one — a document
    /// that OMITS it is how a write names fewer attributes than the row already carries.
    fn write_document(&self, with_note: bool) -> String {
        let (parameters, arguments) = if with_note {
            (", $note: String", ", note: $note")
        } else {
            ("", "")
        };
        format!(
            "mutation W($owner: String!, $key: {key_type}, $name: String{parameters}) \
             {{ {put}(owner: $owner, {key_member}: $key, name: $name{arguments}) \
             {{ owner {key_member} name note }} }}",
            key_type = self.key_type,
            put = self.put,
            key_member = self.key_member,
        )
    }

    /// The reader document of this family.
    fn read_document(&self) -> String {
        format!(
            "query R($owner: String!, $key: {key_type}) \
             {{ {get}(owner: $owner, {key_member}: $key) {{ owner {key_member} name note }} }}",
            key_type = self.key_type,
            get = self.get,
            key_member = self.key_member,
        )
    }

    /// The selection document of this family: the page it answers and the token beside it.
    fn list_document(&self) -> String {
        format!(
            "query L($owner: String!, $limit: Int, $token: String, $forward: Boolean) \
             {{ {list}(owner: $owner, limit: $limit, nextToken: $token, forward: $forward) \
             {{ items {{ owner {key_member} name }} nextToken }} }}",
            list = self.list,
            key_member = self.key_member,
        )
    }

    /// The FILTERED selection document of this family.
    fn find_document(&self) -> String {
        format!(
            "query F($owner: String!, $named: String!, $prefix: String!) \
             {{ {find}(owner: $owner, named: $named, prefix: $prefix) \
             {{ items {{ owner {key_member} name }} nextToken }} }}",
            find = self.find,
            key_member = self.key_member,
        )
    }

    /// The name the row under `key` is seeded with: one name per row, each carrying the same
    /// prefix, so a filter can select one of them, every one of them or none.
    fn row_name(&self, key: &Value) -> String {
        let text = match key {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        format!("{NAME_PREFIX}{text}")
    }

    /// The variables one write carries.
    fn arguments(&self, key: &Value, name: &str, note: Option<&str>) -> Value {
        let mut variables = json!({"owner": OWNER, "key": key.clone(), "name": name});
        if let Some(note) = note {
            variables["note"] = json!(note);
        }
        variables
    }

    /// The variables one read carries.
    fn key_arguments(&self, key: &Value) -> Value {
        json!({"owner": OWNER, "key": key.clone()})
    }

    fn write_pointer(&self) -> String {
        format!("/data/{}", self.put)
    }

    fn read_pointer(&self) -> String {
        format!("/data/{}", self.get)
    }

    fn list_pointer(&self) -> String {
        format!("/data/{}", self.list)
    }

    fn find_pointer(&self) -> String {
        format!("/data/{}", self.find)
    }
}

/// One created API and the tables its data sources resolve against.
struct Suite {
    router: MultiServiceRouter,
    graphql: String,
    api_key: String,
}

impl Suite {
    /// Write the rows a pagination case reads: the partition's own rows, in an order that is not
    /// their sort-key order, and one row of another partition.
    async fn seed(&self, kind: &Kind) {
        for key in kind.written_keys() {
            let case = format!("seeding {} with {key}", kind.case);
            let name = kind.row_name(&key);
            let written = post(
                self,
                &case,
                &kind.write_document(true),
                kind.arguments(&key, &name, Some("kept")),
            )
            .await;
            assert_eq!(
                written.get("errors"),
                None,
                "case {case:?}: the seeding write must carry no errors, got {written}"
            );
        }
        let other = kind.other_key();
        let case = format!("seeding the other partition of {}", kind.case);
        let mut variables = kind.arguments(&other, "elsewhere", None);
        variables["owner"] = json!(OTHER_OWNER);
        let written = post(self, &case, &kind.write_document(false), variables).await;
        assert_eq!(
            written.get("errors"),
            None,
            "case {case:?}: the seeding write must carry no errors, got {written}"
        );
    }

    /// Walk the whole selection, following the token each page reports, and answer the sort keys
    /// it read in the order it read them, beside how many PAGES that took.
    ///
    /// The walk is bounded by the rows the partition can hold, so a token that never stops is a
    /// failure rather than a loop.
    async fn walk(
        &self,
        case: &str,
        kind: &Kind,
        owner: &str,
        limit: Option<usize>,
        forward: Option<bool>,
    ) -> (Vec<Value>, usize) {
        let document = kind.list_document();
        let mut token = Value::Null;
        let mut read = Vec::new();
        for page in 1..=WRITTEN_SORT_KEYS.len() + 2 {
            let answered = post(
                self,
                case,
                &document,
                json!({
                    "owner": owner,
                    "limit": limit.map_or(Value::Null, |limit| json!(limit)),
                    "token": token.clone(),
                    "forward": forward.map_or(Value::Null, |forward| json!(forward)),
                }),
            )
            .await;
            assert_eq!(
                answered.get("errors"),
                None,
                "case {case:?}: page {page} must carry no errors, got {answered}"
            );
            let answered = answered
                .pointer(&kind.list_pointer())
                .cloned()
                .unwrap_or(Value::Null);
            let items = answered["items"].as_array().cloned().unwrap_or_default();
            if let Some(limit) = limit {
                assert!(
                    items.len() <= limit,
                    "case {case:?}: page {page} must carry at most the {limit} rows the \
                     selection was cut at, carried {}",
                    items.len()
                );
            }
            for item in items {
                assert_eq!(
                    item["owner"],
                    json!(owner),
                    "case {case:?}: every row of the page is of the partition selected, got \
                     {item}"
                );
                read.push(item[kind.key_member].clone());
            }
            token = answered["nextToken"].clone();
            if token.is_null() {
                return (read, page);
            }
        }
        panic!("case {case:?}: the token must stop once the selection is read, got {read:?}");
    }

    /// The item Amazon DynamoDB's own reader holds under one key, as the attribute values it
    /// holds them as.
    async fn stored(&self, kind: &Kind, key: &Value) -> serde_json::Map<String, Value> {
        let read = dynamodb(
            &self.router,
            "GetItem",
            json!({
                "TableName": kind.table,
                "Key": kind.wire_key(OWNER, key),
                "ConsistentRead": true,
            }),
        )
        .await;
        read["Item"]
            .as_object()
            .cloned()
            .unwrap_or_else(|| panic!("the table must hold the row written under {key}: {read}"))
    }
}

/// POST one document to the API's published endpoint, as a client does, answering the document
/// it was served.
async fn post(suite: &Suite, case: &str, document: &str, variables: Value) -> Value {
    let body = json!({"query": document, "variables": variables});
    let (status, _, answered) = try_post_answer(
        case,
        &suite.graphql,
        GRAPHQL_PATH,
        "POST",
        &body,
        &[("x-api-key", suite.api_key.as_str())],
    )
    .await
    .unwrap_or_else(|| panic!("case {case:?}: the published endpoint must answer"));
    assert_eq!(
        status, 200,
        "case {case:?}: a keyed GraphQL request is answered 200, got {status} and {answered}"
    );
    answered
}

/// The tables the two data sources of this suite resolve against: one keyed by a string sort
/// key and one by a numeric one.
const ITEMS_TABLE: &str = "marshalled-items";
const RANKED_TABLE: &str = "marshalled-ranked";

/// A router with the services this topology spans, over ONE engine: IAM and STS hold the data
/// sources' service roles, DynamoDB the tables, and AppSync the API that resolves against them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // What a case pins is what the endpoint ANSWERS for the material a call presents, never
    // signature verification, which the emulator's front door owns.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(EmulatedService::new(engine));
    router
}

/// Create one table keyed by a partition AND a sort key of `key_type`, and wait until DynamoDB
/// first reports it `ACTIVE`.
async fn create_table(
    router: &MultiServiceRouter,
    name: &str,
    key_member: &str,
    key_type: &str,
) -> String {
    let created = dynamodb(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [
                {"AttributeName": "owner", "KeyType": "HASH"},
                {"AttributeName": key_member, "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "owner", "AttributeType": "S"},
                {"AttributeName": key_member, "AttributeType": key_type},
            ],
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
        let described = dynamodb(router, "DescribeTable", json!({"TableName": name})).await;
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

/// The request template of one field, written the way the documented templates of this data
/// source are: every value it carries is MARSHALLED by a `$util.dynamodb` helper.
///
/// The write builds its `attributeValues` out of the arguments it was given, so a write that
/// names fewer attributes writes fewer — which is what a full replacement is observed through.
fn request_template(field: &str, key_member: &str) -> String {
    match field {
        "putItem" | "putRanked" => format!(
            "#set($attributes = {{}})\n\
             #if($ctx.args.name)\
             $util.qr($attributes.put(\"name\", $util.dynamodb.toDynamoDB($ctx.args.name)))\
             #end\n\
             #if($ctx.args.note)\
             $util.qr($attributes.put(\"note\", $util.dynamodb.toDynamoDB($ctx.args.note)))\
             #end\n\
             {{\"version\": \"2018-05-29\", \"operation\": \"PutItem\", \
             \"key\": {{\"owner\": $util.dynamodb.toDynamoDBJson($ctx.args.owner), \
             \"{key_member}\": $util.dynamodb.toDynamoDBJson($ctx.args.{key_member})}}, \
             \"attributeValues\": $util.toJson($attributes)}}"
        ),
        "getItem" | "getRanked" => format!(
            "{{\"version\": \"2018-05-29\", \"operation\": \"GetItem\", \
             \"key\": {{\"owner\": $util.dynamodb.toDynamoDBJson($ctx.args.owner), \
             \"{key_member}\": $util.dynamodb.toDynamoDBJson($ctx.args.{key_member})}}}}"
        ),
        // The FILTERED selection: the filter is stated as an OBJECT — two comparisons of one
        // attribute, grouped so either of them selects a row — and transformed into the filter
        // expression the request object's member carries.
        "findItems" | "findRanked" => "{\"version\": \"2018-05-29\", \"operation\": \"Query\", \
              \"query\": {\"expression\": \"#owner = :owner\", \
              \"expressionNames\": {\"#owner\": \"owner\"}, \
              \"expressionValues\": \
              {\":owner\": $util.dynamodb.toDynamoDBJson($ctx.args.owner)}}, \
              \"filter\": $util.transform.toDynamoDBFilterExpression(\
              {\"or\": [{\"name\": {\"eq\": $ctx.args.named}}, \
              {\"name\": {\"beginsWith\": $ctx.args.prefix}}]})}"
            .to_string(),
        // The selection of one partition, cut where the field's own arguments say and resumed
        // from the token they carry.
        _ => "{\"version\": \"2018-05-29\", \"operation\": \"Query\", \
              \"query\": {\"expression\": \"#owner = :owner\", \
              \"expressionNames\": {\"#owner\": \"owner\"}, \
              \"expressionValues\": \
              {\":owner\": $util.dynamodb.toDynamoDBJson($ctx.args.owner)}}, \
              \"limit\": $util.toJson($ctx.args.limit), \
              \"nextToken\": $util.toJson($ctx.args.nextToken), \
              \"scanIndexForward\": $util.toJson($ctx.args.forward)}"
            .to_string(),
    }
}

/// The response template every resolver of this suite carries: it re-raises what the data
/// source refused — which is how a refusal reaches the client as the class it was worded with —
/// and answers the result otherwise.
const RESPONSE_TEMPLATE: &str =
    "#if($ctx.error)\n  $util.error($ctx.error.message, $ctx.error.type, $ctx.result)\n#end\n\
     $util.toJson($ctx.result)\n";

/// Create the two tables, the API with [`SCHEMA`], one data source per table reached under its
/// own service role, and one resolver per field, and answer what a client needs to reach it.
async fn setup(name: &str) -> Suite {
    let router = emulator();
    let tables = [
        (ITEMS_TABLE, ITEMS_DATA_SOURCE, "sk", "S"),
        (RANKED_TABLE, RANKED_DATA_SOURCE, "rank", "N"),
    ];
    let (status, created) = appsync(
        &router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("marshalled-{name}"),
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
    let (status, key) = appsync(
        &router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    let api_key = key["apiKey"]["id"].as_str().unwrap_or_default().to_string();
    install_schema(&router, &api_id, SCHEMA).await;

    for (table, data_source, key_member, key_type) in tables {
        let table_arn = create_table(&router, table, key_member, key_type).await;
        let role = create_role(&router, &format!("{table}-{name}-role"), &table_arn).await;
        let (status, source) = appsync(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}/datasources"),
            Some(json!({
                "name": data_source,
                "type": "AMAZON_DYNAMODB",
                "serviceRoleArn": role,
                "dynamodbConfig": {"tableName": table, "awsRegion": REGION},
            })),
        )
        .await;
        assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    }

    for kind in kinds() {
        for (type_name, field) in [
            ("Mutation", kind.put),
            ("Query", kind.get),
            ("Query", kind.list),
            ("Query", kind.find),
        ] {
            let data_source = if kind.table == ITEMS_TABLE {
                ITEMS_DATA_SOURCE
            } else {
                RANKED_DATA_SOURCE
            };
            let (status, resolver) = appsync(
                &router,
                "POST",
                &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
                Some(json!({
                    "fieldName": field,
                    "kind": "UNIT",
                    "dataSourceName": data_source,
                    "requestMappingTemplate": request_template(field, kind.key_member),
                    "responseMappingTemplate": RESPONSE_TEMPLATE,
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "CreateResolver for {type_name}.{field} must answer 200, got {resolver}"
            );
        }
    }

    Suite {
        router,
        graphql,
        api_key,
    }
}
