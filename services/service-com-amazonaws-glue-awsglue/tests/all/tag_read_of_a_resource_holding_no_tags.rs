// SPDX-License-Identifier: CC0-1.0

//! `GetTags` for a resource this endpoint HOLDS that carries no tags.
//!
//! RULE: `GetTags` reports the tag set of the resource its `ResourceArn` names, and a resource
//! that carries no tags — because its create recorded none, or because its KIND records none
//! at all (a catalog table: `CreateTable` models no `Tags` map) — has no tags rather than no
//! readable tag set, so the read of one this endpoint holds is a success carrying the EMPTY
//! map. The modelled `ResourceArn` (`GlueResourceArn`, len 1..10240, pattern
//! `^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue:`) restricts no resource TYPE and the modelled
//! output's `Tags` (`TagsMap`, len 0..50) admits length 0, so `EntityNotFoundException` is
//! reserved for an ARN naming a resource this endpoint does not hold — a name nothing created,
//! a resource that was deleted, another Region's estate, another account's.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: whether the kind records a tag set at
//! all (a catalog table against a database), whether the create sent no `Tags` member or an
//! empty one, how the ARN spells the names the catalog folds, which database of several holds
//! the table, how much else the record carries, and — for the misses — which step of the
//! address is the one this endpoint holds nothing for.
//!
//! The absent side's wording is the tagging surface's own
//! ([`crate::resource_tagging::UNPLACEABLE_RESOURCE_ARN_NOT_FOUND`]); the forms that address no
//! Glue resource at all are tabled by [`crate::tagging_resource_arn_form`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tagging::{glue_arn, map_of, UNPLACEABLE_RESOURCE_ARN_NOT_FOUND};
use crate::wire::{call, call_in_region, emulator, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// An account other than the one this endpoint serves, so an ARN carrying it names another
/// estate's resource.
const OTHER_ACCOUNT: &str = "999999999999";

/// A Region other than the one the harness serves, for the two ways an ARN can belong to
/// another Region: the ARN naming it, or the endpoint serving it.
const OTHER_REGION: &str = "eu-west-1";

/// Create the database `name`, sending whatever `tags` says about the create's `Tags` member:
/// `None` sends no member at all.
async fn given_database(router: &MultiServiceRouter, name: &str, tags: Option<Value>) {
    let mut body = json!({ "DatabaseInput": { "Name": name } });
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    ok(router, "CreateDatabase", &body).await;
}

/// Create the table `table` in `database` with the `TableInput` `input` describes, and read it
/// back through `GetTable` so every row below is about a record this endpoint really holds.
async fn given_table(router: &MultiServiceRouter, database: &str, table: &str, input: Value) {
    let mut table_input = json!({ "Name": table });
    for (member, value) in input.as_object().into_iter().flatten() {
        table_input[member] = value.clone();
    }
    ok(
        router,
        "CreateTable",
        &json!({ "DatabaseName": database, "TableInput": table_input }),
    )
    .await;
    let read = ok(
        router,
        "GetTable",
        &json!({ "DatabaseName": database, "Name": table }),
    )
    .await;
    // The catalog folds a table's name when it stores it ("For Hive compatibility, this is
    // folded to lowercase when it is stored", `TableInput$Name`), so the read reports the
    // folded name whatever case the create sent.
    assert_eq!(
        read["Table"]["Name"].as_str(),
        Some(table.to_lowercase().as_str()),
        "case {database}/{table}: the create must be readable back through GetTable, got {read}"
    );
}

/// The ARN of the table `table` of `database`, as served in this harness's Region and account.
fn table_arn(database: &str, table: &str) -> String {
    glue_arn(REGION, ACCOUNT_ID, &format!("table/{database}/{table}"))
}

/// Assert that the tag read of `arn` is a success reporting the EMPTY map — `Tags` PRESENT and
/// carrying nothing — and that reading it again reports the same, so the read leaves nothing
/// behind.
async fn assert_tags_read_empty(router: &MultiServiceRouter, case: &str, arn: &str) {
    for pass in ["first read", "read again"] {
        let (status, body) = call(router, "GetTags", &json!({ "ResourceArn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?} ({pass}): a resource this endpoint holds is read, not refused; \
             expected 200, got {status} with {body}"
        );
        let tags = body.get("Tags").unwrap_or_else(|| {
            panic!("case {case:?} ({pass}): the read must report a Tags map, got {body}")
        });
        assert!(
            tags.is_object(),
            "case {case:?} ({pass}): expected Tags to be a map, got {tags}"
        );
        assert!(
            map_of(tags).is_empty(),
            "case {case:?} ({pass}): expected the empty map, got {tags}"
        );
    }
}

/// Assert that the tag read of `arn`, served in `region`, is the absence the tagging surface
/// words: 400 `EntityNotFoundException` carrying that one sentence.
async fn assert_tags_read_absent(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    arn: &str,
) {
    let (status, body) =
        call_in_region(router, region, "GetTags", &json!({ "ResourceArn": arn })).await;
    assert_eq!(
        status, 400,
        "case {case:?}: an ARN naming no resource this endpoint holds is refused with 400, got \
         {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with("EntityNotFoundException"),
        "case {case:?}: expected EntityNotFoundException, got {:?} in {body}",
        error_code(&body)
    );
    assert_eq!(
        error_message(&body),
        UNPLACEABLE_RESOURCE_ARN_NOT_FOUND,
        "case {case:?}: expected the absence worded {UNPLACEABLE_RESOURCE_ARN_NOT_FOUND:?}, got \
         {:?}",
        error_message(&body)
    );
}

/// A resource this endpoint holds that carries no tags reads back as the empty map, whether the
/// tags are absent because its create recorded none or because its kind records none at all.
#[tokio::test]
async fn a_held_resource_carrying_no_tags_reads_back_the_empty_map() {
    let router = emulator();
    given_database(&router, "tag_read_db", None).await;
    given_database(&router, "tag_read_other_db", Some(json!({}))).await;
    given_table(&router, "tag_read_db", "orders", json!({})).await;
    given_table(
        &router,
        "tag_read_db",
        "shipments",
        json!({
            "Description": "one shipment per row",
            "Parameters": { "classification": "parquet" },
            "StorageDescriptor": {
                "Columns": [{ "Name": "id", "Type": "string" }],
                "Location": "s3://warehouse/shipments/",
            },
        }),
    )
    .await;
    given_table(&router, "tag_read_other_db", "invoices", json!({})).await;

    // (case, the ARN read). Each names a resource this endpoint holds whose record carries no
    // tag at all.
    let held: Vec<(&str, String)> = vec![
        (
            "a catalog table, a kind whose create models no Tags map",
            table_arn("tag_read_db", "orders"),
        ),
        (
            "a table whose record carries a description, parameters and a schema",
            table_arn("tag_read_db", "shipments"),
        ),
        (
            "the second database's table, addressed under its own database",
            table_arn("tag_read_other_db", "invoices"),
        ),
        (
            "a table ARN spelling both names in another case, which the catalog folds",
            table_arn("TAG_READ_OTHER_DB", "INVOICES"),
        ),
        (
            "a database whose create sent no Tags member at all",
            glue_arn(REGION, ACCOUNT_ID, "database/tag_read_db"),
        ),
        (
            "a database whose create sent an empty Tags map",
            glue_arn(REGION, ACCOUNT_ID, "database/tag_read_other_db"),
        ),
    ];

    for (case, arn) in &held {
        assert_tags_read_empty(&router, case, arn).await;
    }
}

/// An ARN naming a resource this endpoint does NOT hold stays the tagging surface's absence,
/// whichever step of the address is the one it holds nothing for.
#[tokio::test]
async fn an_arn_naming_no_held_resource_is_still_a_miss() {
    let router = emulator();
    given_database(&router, "tag_miss_db", None).await;
    given_table(&router, "tag_miss_db", "orders", json!({})).await;
    given_table(&router, "tag_miss_db", "retired", json!({})).await;
    ok(
        &router,
        "DeleteTable",
        &json!({ "DatabaseName": "tag_miss_db", "Name": "retired" }),
    )
    .await;

    // (case, the Region the request is served in, the ARN read).
    let absent: Vec<(&str, &str, String)> = vec![
        (
            "a table name nothing created, in a database this endpoint holds",
            REGION,
            table_arn("tag_miss_db", "absent_table"),
        ),
        (
            "a table of a database nothing created",
            REGION,
            table_arn("absent_db", "orders"),
        ),
        (
            "a table that was deleted after being created",
            REGION,
            table_arn("tag_miss_db", "retired"),
        ),
        (
            "a held table addressed under another account",
            REGION,
            glue_arn(REGION, OTHER_ACCOUNT, "table/tag_miss_db/orders"),
        ),
        (
            "a held table addressed in another Region",
            REGION,
            glue_arn(OTHER_REGION, ACCOUNT_ID, "table/tag_miss_db/orders"),
        ),
        (
            "this Region's table read through another Region's endpoint",
            OTHER_REGION,
            table_arn("tag_miss_db", "orders"),
        ),
        (
            "a table ARN carrying no table name after its database",
            REGION,
            glue_arn(REGION, ACCOUNT_ID, "table/tag_miss_db"),
        ),
        (
            "a table ARN carrying a segment past the table name",
            REGION,
            glue_arn(REGION, ACCOUNT_ID, "table/tag_miss_db/orders/extra"),
        ),
        (
            "a table version, a kind this endpoint holds no record of",
            REGION,
            glue_arn(REGION, ACCOUNT_ID, "tableVersion/tag_miss_db/orders"),
        ),
        (
            "a user defined function, a kind this endpoint holds no record of",
            REGION,
            glue_arn(REGION, ACCOUNT_ID, "userDefinedFunction/tag_miss_db/to_upper"),
        ),
    ];

    for (case, region, arn) in &absent {
        assert_tags_read_absent(&router, case, region, arn).await;
    }

    // The table the misses were addressed around is untouched: it still reads as held.
    assert_tags_read_empty(
        &router,
        "the held table, after every miss above",
        &table_arn("tag_miss_db", "orders"),
    )
    .await;
}

/// A kind that records no tag set has nowhere for a tag WRITE to land, so the writes keep
/// answering the tagging surface's absence for a table's ARN — and the read of that table
/// still reports the empty map afterwards, so a refused write leaves it exactly as it was.
#[tokio::test]
async fn a_kind_recording_no_tag_set_takes_no_tag_write() {
    let router = emulator();
    given_database(&router, "tag_write_db", None).await;
    given_table(&router, "tag_write_db", "orders", json!({})).await;
    let arn = table_arn("tag_write_db", "orders");

    for (operation, body) in [
        (
            "TagResource",
            json!({ "ResourceArn": arn, "TagsToAdd": { "written": "no" } }),
        ),
        (
            "UntagResource",
            json!({ "ResourceArn": arn, "TagsToRemove": ["written"] }),
        ),
    ] {
        let (status, answer) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {operation}: a kind recording no tag set takes no tag write; expected 400, got \
             {status} with {answer}"
        );
        assert!(
            error_code(&answer).ends_with("EntityNotFoundException"),
            "case {operation}: expected EntityNotFoundException, got {:?} in {answer}",
            error_code(&answer)
        );
    }

    assert_tags_read_empty(&router, "the table after both refused writes", &arn).await;
}
