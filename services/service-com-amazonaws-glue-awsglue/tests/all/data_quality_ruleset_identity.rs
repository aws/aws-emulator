// SPDX-License-Identifier: CC0-1.0

//! The identity of a Glue data quality ruleset, across every answer that reports one.
//!
//! RULE — the identifier a ruleset is assigned when it is created is the record's OWN
//! identity: it is minted once, by the create, and every later answer about that same record
//! reports it unchanged, so a client can take the identifier from whichever answer it holds
//! (the create's, a read's or an update's) and address the same ruleset with it. Two records
//! never share one, and a record created again under a name that was deleted is a new record
//! with a new identifier.
//!
//! The rows of the table below differ along what the rule generalises over: which members the
//! create registers (a target table or none, a catalog-qualified target, a description, a
//! security configuration, tags, a client token), the form and size of the names and the
//! definition, and which member a later patch names.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::wire::{call, emulator, error_code, ok, ACCOUNT_ID};

/// The members `CreateDataQualityRuleset` answers with: the one the published shape binds
/// (`smithy_query shape=CreateDataQualityRulesetResponse`) plus the record's identity and
/// definition the service reports beside it.
const CREATE_RESPONSE_MEMBERS: [&str; 7] = [
    "Name",
    "Identifier",
    "Description",
    "Ruleset",
    "TargetTable",
    "CreatedOn",
    "LastModifiedOn",
];

/// The longest a Glue `HashString` id may be, which bounds the identifier the service assigns
/// a data quality resource: `HashString`, "len 1..255"
/// (`smithy_query shape=HashString`).
const MAX_HASH_STRING_LENGTH: usize = 255;

/// The identifier an answer reports, or a panic naming the case when it reports none.
fn identifier_of(case: &str, operation: &str, body: &Value) -> String {
    body.get("Identifier")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case}: {operation} must report the ruleset's Identifier, got {body}")
        })
        .to_string()
}

/// The timestamp member `member` of an answer, as seconds since the epoch.
fn instant(case: &str, operation: &str, member: &str, body: &Value) -> f64 {
    body[member].as_f64().unwrap_or_else(|| {
        panic!("case {case}: {operation} must report {member} as a timestamp, got {body}")
    })
}

/// Every answer about one ruleset — the create that registered it, a read of it and an update
/// of it — reports the identifier the create assigned that record, and no two records share
/// one.
#[tokio::test]
async fn one_rulesets_identifier_is_reported_by_its_create_its_reads_and_its_updates() {
    let router = emulator();

    // (case, the create, the patch applied afterwards)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            // The shape a lifecycle client sends: a ruleset created against a table, then
            // patched in both of its optional members.
            "a ruleset targeting a table, patched in both members",
            json!({
                "Name": "orders-rules",
                "Ruleset": "Rules = [ ColumnExists \"order_id\" ]",
                "Description": "the first description",
                "TargetTable": { "DatabaseName": "sales", "TableName": "orders" },
            }),
            json!({
                "Name": "orders-rules",
                "Ruleset": "Rules = [ ColumnExists \"order_id\", RowCount > 0 ]",
                "Description": "the second description",
            }),
        ),
        (
            // `DataQualityTargetTable` carries an optional `CatalogId` beside its two
            // required names, so a catalog-qualified target is reported with all three.
            "a ruleset targeting a table in a named catalog",
            json!({
                "Name": "catalogued-rules",
                "Ruleset": "Rules = [ IsComplete \"id\" ]",
                "TargetTable": {
                    "DatabaseName": "sales",
                    "TableName": "customers",
                    "CatalogId": ACCOUNT_ID,
                },
                "DataQualitySecurityConfiguration": "dq-security-config",
                "Tags": { "owner": "data-governance" },
            }),
            json!({ "Name": "catalogued-rules", "Description": "added by the update" }),
        ),
        (
            // No target table at all: the member the create never set stays absent, and the
            // identity is reported all the same.
            "a ruleset targeting no table, patched in the ruleset alone",
            json!({
                "Name": "untargeted-rules",
                "Ruleset": "Rules = [ ColumnExists \"a\" ]",
            }),
            json!({ "Name": "untargeted-rules", "Ruleset": "Rules = [ RowCount > 1 ]" }),
        ),
        (
            // A create carrying a `ClientToken`, which is the idempotency key rather than a
            // second address: the record still gets one identifier of its own.
            "a ruleset created under a client token",
            json!({
                "Name": "tokened-rules",
                "Ruleset": "Rules = [ ColumnExists \"b\" ]",
                "TargetTable": { "DatabaseName": "db", "TableName": "t" },
                "ClientToken": "a-client-token",
            }),
            json!({ "Name": "tokened-rules", "Description": "patched once" }),
        ),
        (
            // `NameString`'s pattern admits a space and a tab, so a name carrying them
            // addresses its ruleset like any other.
            "a name carrying a space and a tab",
            json!({
                "Name": "rule set one\ttwo",
                "Ruleset": "Rules = [ ColumnExists \"c\" ]",
                "TargetTable": { "DatabaseName": "db two", "TableName": "table\tthree" },
            }),
            json!({ "Name": "rule set one\ttwo", "Ruleset": "Rules = [ RowCount > 2 ]" }),
        ),
        (
            // `NameString` has "len 1..255": a name on the published upper bound is a name
            // like any other, and so is a one-character one.
            "a name at the published upper bound",
            json!({
                "Name": "n".repeat(255),
                "Ruleset": "Rules = [ ColumnExists \"d\" ]",
            }),
            json!({ "Name": "n".repeat(255), "Description": "patched at the bound" }),
        ),
        (
            "a one-character name and a patch naming no member to change",
            json!({
                "Name": "r",
                "Ruleset": "Rules = [ ColumnExists \"e\" ]",
                "Description": "untouched",
                "TargetTable": { "DatabaseName": "db", "TableName": "t" },
            }),
            json!({ "Name": "r" }),
        ),
    ];

    let mut identifiers: BTreeSet<String> = BTreeSet::new();

    for (case, create, patch) in &rows {
        let name = create["Name"].as_str().expect("the row carries a Name");

        let created = ok(&router, "CreateDataQualityRuleset", create).await;
        let identifier = identifier_of(case, "CreateDataQualityRuleset", &created);
        assert!(
            !identifier.is_empty() && identifier.len() <= MAX_HASH_STRING_LENGTH,
            "case {case}: expected an identifier of 1..{MAX_HASH_STRING_LENGTH} characters, got \
             {identifier:?}"
        );
        assert!(
            identifiers.insert(identifier.clone()),
            "case {case}: two rulesets must not share an identifier, got {identifier:?} again"
        );

        // The create answers with the record it stored, not with the request it was given:
        // the definition it registered, the association only it could set, and the two
        // instants of a record nothing has modified yet.
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: expected the stored name, got {created}"
        );
        assert_eq!(
            created.get("Ruleset"),
            create.get("Ruleset"),
            "case {case}: expected the registered definition, got {created}"
        );
        assert_eq!(
            created.get("TargetTable").cloned().unwrap_or(Value::Null),
            create.get("TargetTable").cloned().unwrap_or(Value::Null),
            "case {case}: expected the TargetTable the create named, got {created}"
        );
        assert_eq!(
            created.get("Description").cloned().unwrap_or(Value::Null),
            create.get("Description").cloned().unwrap_or(Value::Null),
            "case {case}: expected the description the create registered, got {created}"
        );
        assert_eq!(
            instant(case, "CreateDataQualityRuleset", "CreatedOn", &created),
            instant(case, "CreateDataQualityRuleset", "LastModifiedOn", &created),
            "case {case}: a record nothing has modified was last modified when it was \
             created, got {created}"
        );
        for reported in created
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: the answer is a structure, got {created}"))
            .keys()
        {
            assert!(
                CREATE_RESPONSE_MEMBERS.contains(&reported.as_str()),
                "case {case}: the create must carry only {CREATE_RESPONSE_MEMBERS:?}, got \
                 {reported} in {created}"
            );
        }

        // The read reports the same identity, and so does the update — so the value is the
        // record's and not one minted per call.
        let read = ok(&router, "GetDataQualityRuleset", &json!({ "Name": name })).await;
        assert_eq!(
            identifier_of(case, "GetDataQualityRuleset", &read),
            identifier,
            "case {case}: a read must report the identifier the create assigned; expected \
             {identifier:?}, got {read}"
        );
        assert_eq!(
            instant(case, "GetDataQualityRuleset", "CreatedOn", &read),
            instant(case, "CreateDataQualityRuleset", "CreatedOn", &created),
            "case {case}: the read must report the creation instant the create reported, got \
             {read}"
        );

        let updated = ok(&router, "UpdateDataQualityRuleset", patch).await;
        assert_eq!(
            identifier_of(case, "UpdateDataQualityRuleset", &updated),
            identifier,
            "case {case}: an update must report the identifier the create assigned; expected \
             {identifier:?}, got {updated}"
        );

        // And the record keeps it after the write: the identifier is not a property of one
        // answer but of the ruleset.
        let reread = ok(&router, "GetDataQualityRuleset", &json!({ "Name": name })).await;
        assert_eq!(
            identifier_of(case, "GetDataQualityRuleset after the update", &reread),
            identifier,
            "case {case}: the identifier must survive the update; expected {identifier:?}, got \
             {reread}"
        );
    }
}

/// A create that does not store a new record does not mint a new identity either: the
/// idempotent retry of a create answers with the stored ruleset's identifier, and a create
/// refused for a name already taken leaves that identifier where it was.
#[tokio::test]
async fn a_create_that_stores_no_new_record_mints_no_new_identifier() {
    let router = emulator();
    let create = json!({
        "Name": "retried-rules",
        "Ruleset": "Rules = [ ColumnExists \"id\" ]",
        "Description": "the registered description",
        "TargetTable": { "DatabaseName": "sales", "TableName": "orders" },
        "ClientToken": "the-same-token",
    });

    let first = ok(&router, "CreateDataQualityRuleset", &create).await;
    let identifier = identifier_of("the first create", "CreateDataQualityRuleset", &first);

    // The SAME create arriving twice: the retry is answered with the stored record, identity
    // and all.
    let retried = ok(&router, "CreateDataQualityRuleset", &create).await;
    assert_eq!(
        identifier_of("the idempotent retry", "CreateDataQualityRuleset", &retried),
        identifier,
        "case the idempotent retry: expected the stored identifier {identifier:?}, got {retried}"
    );

    // A create for a name already taken is refused, and the refusal writes nothing: the
    // ruleset keeps the identifier it had.
    for (case, refused) in [
        (
            "the same name under no token",
            json!({ "Name": "retried-rules", "Ruleset": "Rules = [ RowCount > 0 ]" }),
        ),
        (
            "the same name and token with another definition",
            json!({
                "Name": "retried-rules",
                "Ruleset": "Rules = [ RowCount > 5 ]",
                "ClientToken": "the-same-token",
            }),
        ),
    ] {
        let (status, body) = call(&router, "CreateDataQualityRuleset", &refused).await;
        assert_eq!(
            status, 400,
            "case {case}: a name already taken is refused, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {body}"
        );
        let read = ok(
            &router,
            "GetDataQualityRuleset",
            &json!({ "Name": "retried-rules" }),
        )
        .await;
        assert_eq!(
            identifier_of(case, "GetDataQualityRuleset", &read),
            identifier,
            "case {case}: the refusal must leave the identifier at {identifier:?}, got {read}"
        );
    }

    // A record created again under a name that was deleted is a NEW record, so it carries a
    // new identifier.
    ok(
        &router,
        "DeleteDataQualityRuleset",
        &json!({ "Name": "retried-rules" }),
    )
    .await;
    let recreated = ok(&router, "CreateDataQualityRuleset", &create).await;
    assert_ne!(
        identifier_of("recreated after a delete", "CreateDataQualityRuleset", &recreated),
        identifier,
        "case recreated after a delete: a new record must carry a new identifier, got {recreated}"
    );
}
