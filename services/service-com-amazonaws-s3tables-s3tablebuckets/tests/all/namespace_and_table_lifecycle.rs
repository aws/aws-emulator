// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the S3 Tables namespace and table control plane:
//! `CreateNamespace` -> `GetNamespace` -> `CreateTable` -> `GetTable` ->
//! `ListTables` -> `ListTableBuckets` -> `DeleteTable` -> `DeleteNamespace` ->
//! `DeleteTableBucket`.
//!
//! The rule under test: every one of these operations computes its answer from the
//! records the creates PERSISTED — a namespace is stored under the bucket its ARN
//! resolves to and a table under the namespace it names, each with the server-minted
//! identity (namespace id, table ARN, warehouse location, version token) the create
//! wrote; a read reports that record whichever way it is addressed, a list reports
//! the stored order filtered and paged, a delete removes exactly the one record so a
//! later read answers 404, and a delete of a container that still holds records is
//! refused instead.
//!
//! Each table below spans the class the rule generalises over — name form and
//! length, how many namespaces and tables are resident, which optional members the
//! request carries, which filter narrows a list, how large a page is — rather than
//! one recorded request.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use service_com_amazonaws_s3tables_s3tablebuckets::state::{
    OpenTableFormat, TableBucketType, TableType,
};

use crate::harness::{
    expected_bucket_arn, now_seconds, timestamp_seconds, Emulator, Reply, ACCOUNT,
};

/// The region most cases work in; the ARNs a create mints carry it.
const REGION: &str = "us-east-1";

/// One `CreateNamespace` + `CreateTable` pair, as a row of the table below.
#[derive(Debug, Clone)]
struct CreateCase {
    label: &'static str,
    region: &'static str,
    bucket: &'static str,
    /// The namespace name, which is also the key the record is stored under.
    namespace: String,
    table: String,
    /// Whether the create describes the table's metadata, which is what gives
    /// the table a metadata file to report.
    metadata: bool,
    /// `encryptionConfiguration`, as `(sseAlgorithm, kmsKeyArn)`.
    encryption: Option<(&'static str, Option<&'static str>)>,
    storage_class: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// A KMS key ARN of the form the modeled `kmsKeyArn` pattern accepts
/// (`^(arn:aws[-a-z0-9]*:kms:[-a-z0-9]*:[0-9]{12}:key/.+)$`).
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

impl CreateCase {
    fn table_request(&self) -> Value {
        let mut request = Map::new();
        request.insert("name".to_string(), json!(self.table));
        // `format` is required and `ICEBERG` is the only value the modeled
        // `OpenTableFormat` enum declares.
        request.insert(
            "format".to_string(),
            json!(OpenTableFormat::Iceberg.as_wire()),
        );
        if self.metadata {
            request.insert(
                "metadata".to_string(),
                json!({
                    "iceberg": {
                        "schema": {
                            "fields": [
                                { "name": "id", "type": "long", "required": true },
                                { "name": "label", "type": "string" },
                            ]
                        }
                    }
                }),
            );
        }
        if let Some((algorithm, key)) = self.encryption {
            let mut configuration = Map::new();
            configuration.insert("sseAlgorithm".to_string(), json!(algorithm));
            if let Some(key) = key {
                configuration.insert("kmsKeyArn".to_string(), json!(key));
            }
            request.insert(
                "encryptionConfiguration".to_string(),
                Value::Object(configuration),
            );
        }
        if let Some(storage_class) = self.storage_class {
            request.insert(
                "storageClassConfiguration".to_string(),
                json!({ "storageClass": storage_class }),
            );
        }
        if !self.tags.is_empty() {
            let tags: Map<String, Value> = self
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect();
            request.insert("tags".to_string(), Value::Object(tags));
        }
        Value::Object(request)
    }
}

/// The table: it spans namespace and table name length (the modeled minimum of
/// one character, middling names, the modeled maximum of 255), name form (letters,
/// digits, interior underscores), whether the create describes table metadata,
/// both encryption algorithms, both storage classes, tag counts from none to
/// several, and two regions.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "the shortest names the model allows, no optional members",
            region: REGION,
            bucket: "shortest-names",
            namespace: "n".to_string(),
            table: "t".to_string(),
            metadata: false,
            encryption: None,
            storage_class: None,
            tags: &[],
        },
        CreateCase {
            label: "interior underscores, metadata, SSE-S3, standard storage, one tag",
            region: REGION,
            bucket: "sales-lakehouse",
            namespace: "sales_eu".to_string(),
            table: "daily_orders".to_string(),
            metadata: true,
            encryption: Some(("AES256", None)),
            storage_class: Some("STANDARD"),
            tags: &[("team", "analytics")],
        },
        CreateCase {
            label: "digits only, SSE-KMS with a key, intelligent tiering, several tags",
            region: "eu-west-1",
            bucket: "finance-lakehouse",
            namespace: "20260101".to_string(),
            table: "20260102".to_string(),
            metadata: true,
            encryption: Some(("aws:kms", Some(KMS_KEY_ARN))),
            storage_class: Some("INTELLIGENT_TIERING"),
            tags: &[("env", "prod"), ("owner", "finance"), ("cost-center", "4711")],
        },
        CreateCase {
            label: "names at the modeled maximum of 255 characters",
            region: "us-west-2",
            bucket: "longest-names",
            // 255 characters, the modeled maximum for both names.
            namespace: format!("n{}", "_a".repeat(127)),
            table: format!("t{}", "_b".repeat(127)),
            metadata: false,
            encryption: Some(("AES256", None)),
            storage_class: None,
            tags: &[("snapshot", "")],
        },
    ]
}

/// Create the bucket `case` names and answer its ARN.
async fn bucket_for(emulator: &Emulator, case: &CreateCase) -> String {
    let created = emulator
        .create_table_bucket(case.region, &json!({ "name": case.bucket }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {label:?}: CreateTableBucket must succeed, got {status} {body}",
        label = case.label,
        status = created.status,
        body = created.body
    );
    created.text("arn")
}

/// Every string member `GetTable` must answer with, as `(member, value)`.
fn table_members(reply: &Reply) -> Vec<(&'static str, String)> {
    [
        "name",
        "type",
        "tableARN",
        "versionToken",
        "warehouseLocation",
        "createdBy",
        "modifiedBy",
        "ownerAccountId",
        "format",
        "namespaceId",
        "tableBucketId",
    ]
    .into_iter()
    .map(|member| (member, reply.text(member)))
    .collect()
}

#[tokio::test]
async fn a_created_namespace_and_table_are_readable_field_for_field() {
    let emulator = Emulator::new();
    let cases = create_cases();
    let mut seen_namespace_ids: BTreeSet<String> = BTreeSet::new();
    let mut seen_table_arns: BTreeSet<String> = BTreeSet::new();
    let mut seen_version_tokens: BTreeSet<String> = BTreeSet::new();

    for case in &cases {
        for (member, name) in [("namespace", &case.namespace), ("table", &case.table)] {
            assert!(
                (1..=255).contains(&name.chars().count()),
                "case {label:?}: the row's own {member} name must satisfy the modeled length, \
                 got {len}",
                label = case.label,
                len = name.chars().count()
            );
        }

        let bucket_arn = bucket_for(&emulator, case).await;
        assert_eq!(
            bucket_arn,
            expected_bucket_arn(case.region, case.bucket),
            "case {label:?}: the bucket ARN must carry the request's region and name",
            label = case.label
        );

        // CreateNamespace answers with the stored namespace, under the bucket
        // the ARN resolved to.
        let before = now_seconds();
        let created_namespace = emulator
            .create_namespace(
                case.region,
                &bucket_arn,
                &json!({ "namespace": [case.namespace] }),
            )
            .await;
        assert_eq!(
            created_namespace.status, 200,
            "case {label:?}: CreateNamespace must succeed, got {status} {body}",
            label = case.label,
            status = created_namespace.status,
            body = created_namespace.body
        );
        assert_eq!(
            created_namespace.text("tableBucketARN"),
            bucket_arn,
            "case {label:?}: the create must answer with the bucket it stored the namespace in",
            label = case.label
        );
        assert_eq!(
            created_namespace.list("namespace"),
            vec![json!(case.namespace)],
            "case {label:?}: the create must answer with the stored namespace name, got {actual:?}",
            label = case.label,
            actual = created_namespace.list("namespace")
        );

        // The round trip: GetNamespace reports the record the create wrote.
        let read_namespace = emulator
            .get_namespace(case.region, &bucket_arn, &case.namespace)
            .await;
        assert_eq!(
            read_namespace.status, 200,
            "case {label:?}: GetNamespace must succeed, got {status} {body}",
            label = case.label,
            status = read_namespace.status,
            body = read_namespace.body
        );
        assert_eq!(
            read_namespace.list("namespace"),
            vec![json!(case.namespace)],
            "case {label:?}: the reader must answer with the stored namespace name",
            label = case.label
        );
        for member in ["createdBy", "ownerAccountId"] {
            assert_eq!(
                read_namespace.text(member),
                ACCOUNT,
                "case {label:?}: {member} must be the emulator's account, got {actual:?}",
                label = case.label,
                actual = read_namespace.text(member)
            );
        }
        let namespace_id = read_namespace.text("namespaceId");
        assert!(
            !namespace_id.is_empty() && !namespace_id.contains(&case.namespace),
            "case {label:?}: namespaceId must be a server-minted id, got {namespace_id:?}",
            label = case.label
        );
        assert!(
            seen_namespace_ids.insert(namespace_id.clone()),
            "case {label:?}: namespaceId {namespace_id:?} was already minted for another namespace",
            label = case.label
        );
        let namespace_created_at =
            timestamp_seconds(&read_namespace.body, "createdAt", case.label);
        assert!(
            namespace_created_at >= before - 1.0 && namespace_created_at <= now_seconds() + 1.0,
            "case {label:?}: the namespace's createdAt {namespace_created_at} must lie in the \
             window the create ran in",
            label = case.label
        );
        let bucket_read = emulator.get_table_bucket(case.region, &bucket_arn).await;
        assert_eq!(
            read_namespace.text("tableBucketId"),
            bucket_read.text("tableBucketId"),
            "case {label:?}: the namespace must report the id of the bucket it lives in",
            label = case.label
        );

        // CreateTable mints and persists the table's identity.
        let before = now_seconds();
        let created_table = emulator
            .create_table(
                case.region,
                &bucket_arn,
                &case.namespace,
                &case.table_request(),
            )
            .await;
        assert_eq!(
            created_table.status, 200,
            "case {label:?}: CreateTable must succeed, got {status} {body}",
            label = case.label,
            status = created_table.status,
            body = created_table.body
        );
        let table_arn = created_table.text("tableARN");
        let version_token = created_table.text("versionToken");
        assert!(
            table_arn.starts_with(&format!("{bucket_arn}/table/")),
            "case {label:?}: the table ARN must sit under its bucket's ARN, got {table_arn:?}",
            label = case.label
        );
        let table_id = table_arn
            .rsplit('/')
            .next()
            .expect("an ARN has a last segment")
            .to_string();
        assert!(
            !table_id.is_empty()
                && !table_id.contains(&case.table)
                && table_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "case {label:?}: the ARN's table segment must be a server-minted id the modeled \
             pattern accepts, got {table_id:?}",
            label = case.label
        );
        assert!(
            seen_table_arns.insert(table_arn.clone()),
            "case {label:?}: table ARN {table_arn:?} was already minted for another table",
            label = case.label
        );
        assert!(
            (1..=2048).contains(&version_token.chars().count())
                && !version_token.contains(&case.table),
            "case {label:?}: versionToken must be a server-minted token of the modeled length, \
             got {version_token:?}",
            label = case.label
        );
        assert!(
            seen_version_tokens.insert(version_token.clone()),
            "case {label:?}: versionToken {version_token:?} was already minted for another table",
            label = case.label
        );

        // The round trip, through BOTH addressing modes the model offers.
        let by_parts = emulator
            .get_table(case.region, &bucket_arn, &case.namespace, &case.table)
            .await;
        assert_eq!(
            by_parts.status, 200,
            "case {label:?}: GetTable by bucket, namespace and name must succeed, got {status} \
             {body}",
            label = case.label,
            status = by_parts.status,
            body = by_parts.body
        );
        let by_arn = emulator.get_table_by_arn(case.region, &table_arn).await;
        assert_eq!(
            by_arn.status, 200,
            "case {label:?}: GetTable by table ARN must succeed, got {status} {body}",
            label = case.label,
            status = by_arn.status,
            body = by_arn.body
        );
        assert_eq!(
            by_arn.body, by_parts.body,
            "case {label:?}: both addressing modes must resolve the same stored record",
            label = case.label
        );

        let read = by_parts;
        let expected: Vec<(&str, String)> = vec![
            ("name", case.table.clone()),
            ("type", TableType::Customer.as_wire().to_string()),
            ("tableARN", table_arn.clone()),
            ("versionToken", version_token.clone()),
            (
                "warehouseLocation",
                format!("s3://{table_id}--table-s3"),
            ),
            ("createdBy", ACCOUNT.to_string()),
            ("modifiedBy", ACCOUNT.to_string()),
            ("ownerAccountId", ACCOUNT.to_string()),
            ("format", OpenTableFormat::Iceberg.as_wire().to_string()),
            ("namespaceId", namespace_id.clone()),
            ("tableBucketId", bucket_read.text("tableBucketId")),
        ];
        assert_eq!(
            table_members(&read),
            expected,
            "case {label:?}: every member must come from the stored record",
            label = case.label
        );
        assert_eq!(
            read.list("namespace"),
            vec![json!(case.namespace)],
            "case {label:?}: the table must report the namespace it lives in",
            label = case.label
        );
        let created_at = timestamp_seconds(&read.body, "createdAt", case.label);
        let modified_at = timestamp_seconds(&read.body, "modifiedAt", case.label);
        assert!(
            created_at >= before - 1.0 && created_at <= now_seconds() + 1.0,
            "case {label:?}: the table's createdAt {created_at} must lie in the window the \
             create ran in",
            label = case.label
        );
        assert_eq!(
            modified_at, created_at,
            "case {label:?}: a table nothing has changed was last modified when it was created",
            label = case.label
        );

        // The metadata file exists only for a table whose create described its
        // metadata, and it sits under that table's own warehouse location.
        let metadata_location = read.optional_text("metadataLocation");
        match (case.metadata, &metadata_location) {
            (true, Some(location)) => assert!(
                location.starts_with(&format!("s3://{table_id}--table-s3/metadata/"))
                    && location.ends_with(".metadata.json"),
                "case {label:?}: the metadata file must sit under the table's warehouse \
                 location, got {location:?}",
                label = case.label
            ),
            (false, None) => {}
            (expected, actual) => panic!(
                "case {label:?}: a table whose create described metadata ({expected}) must \
                 report a metadata location, got {actual:?}",
                label = case.label
            ),
        }

        // Reading twice must answer identically: a read changes nothing.
        let again = emulator
            .get_table(case.region, &bucket_arn, &case.namespace, &case.table)
            .await;
        assert_eq!(
            again.body, read.body,
            "case {label:?}: two reads of one table must answer identically",
            label = case.label
        );
    }
}

/// One `ListTables` case: what narrows the listing, and which tables must come
/// back in which order.
#[derive(Debug)]
struct ListCase {
    label: &'static str,
    query: Vec<(&'static str, String)>,
    expected: Vec<&'static str>,
}

#[tokio::test]
async fn list_tables_reports_the_stored_order_filtered_and_paged() {
    let emulator = Emulator::new();
    let bucket_arn = emulator
        .create_table_bucket(REGION, &json!({ "name": "listing-bucket" }))
        .await
        .text("arn");

    // Three namespaces holding a different number of tables each, created in an
    // order that is NOT the stored one, so a listing that echoed the creates
    // rather than the store would come back in the wrong order.
    let resident = [
        ("sales", "weekly_totals"),
        ("ops", "runbooks"),
        ("sales", "daily_orders"),
        ("ops", "runs"),
        ("marketing", "campaigns"),
        ("ops", "alerts"),
    ];
    for (namespace, table) in resident {
        if emulator
            .get_namespace(REGION, &bucket_arn, namespace)
            .await
            .status
            != 200
        {
            let created = emulator
                .create_namespace(REGION, &bucket_arn, &json!({ "namespace": [namespace] }))
                .await;
            assert_eq!(
                created.status, 200,
                "CreateNamespace {namespace:?} must succeed, got {status} {body}",
                status = created.status,
                body = created.body
            );
        }
        let created = emulator
            .create_table(
                REGION,
                &bucket_arn,
                namespace,
                &json!({ "name": table, "format": "ICEBERG" }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "CreateTable {namespace:?}/{table:?} must succeed, got {status} {body}",
            status = created.status,
            body = created.body
        );
    }

    // The stored order is namespace, then table name.
    let every_table = vec![
        "marketing/campaigns",
        "ops/alerts",
        "ops/runbooks",
        "ops/runs",
        "sales/daily_orders",
        "sales/weekly_totals",
    ];
    let cases = vec![
        ListCase {
            label: "no filter: every table in the bucket, in the stored order",
            query: vec![],
            expected: every_table.clone(),
        },
        ListCase {
            label: "a namespace filter: only that namespace's tables",
            query: vec![("namespace", "ops".to_string())],
            expected: vec!["ops/alerts", "ops/runbooks", "ops/runs"],
        },
        ListCase {
            label: "a namespace holding exactly one table",
            query: vec![("namespace", "marketing".to_string())],
            expected: vec!["marketing/campaigns"],
        },
        ListCase {
            label: "a prefix every namespace is searched for",
            query: vec![("prefix", "run".to_string())],
            expected: vec!["ops/runbooks", "ops/runs"],
        },
        ListCase {
            label: "a prefix inside one namespace",
            query: vec![
                ("namespace", "sales".to_string()),
                ("prefix", "daily".to_string()),
            ],
            expected: vec!["sales/daily_orders"],
        },
        ListCase {
            label: "a prefix no table name starts with",
            query: vec![("prefix", "zzz".to_string())],
            expected: vec![],
        },
        ListCase {
            label: "a page size larger than the number of resident tables",
            query: vec![("maxTables", "1000".to_string())],
            expected: every_table.clone(),
        },
    ];

    for case in &cases {
        let listed = emulator.list_tables(REGION, &bucket_arn, &case.query).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: ListTables must succeed, got {status} {body}",
            label = case.label,
            status = listed.status,
            body = listed.body
        );
        let keys: Vec<String> = listed
            .list("tables")
            .iter()
            .map(|summary| {
                format!(
                    "{}/{}",
                    summary["namespace"][0].as_str().unwrap_or("<none>"),
                    summary["name"].as_str().unwrap_or("<none>")
                )
            })
            .collect();
        assert_eq!(
            keys, case.expected,
            "case {label:?}: the listing must report the stored tables in the stored order, got \
             {keys:?}",
            label = case.label
        );
        assert_eq!(
            listed.optional_text("continuationToken"),
            None,
            "case {label:?}: a listing that returned everything must carry no continuation \
             token, got {actual:?}",
            label = case.label,
            actual = listed.optional_text("continuationToken")
        );
    }

    // Every summary is built from the same record `GetTable` reports.
    let summaries = emulator
        .list_tables(REGION, &bucket_arn, &[])
        .await
        .list("tables");
    for summary in &summaries {
        let namespace = summary["namespace"][0].as_str().expect("a namespace name");
        let name = summary["name"].as_str().expect("a table name");
        let read = emulator
            .get_table(REGION, &bucket_arn, namespace, name)
            .await;
        for member in ["tableARN", "type", "createdAt", "modifiedAt"] {
            assert_eq!(
                summary.get(member),
                read.body.get(member),
                "case {namespace:?}/{name:?}: the summary's {member} must be the stored one"
            );
        }
        // A summary reports the table's own record; the ids of the namespace and the
        // table bucket it sits in are reported by the reads of those resources, which
        // is what `list_tables_summary_members.rs` pins for every listing.
        for container_id in ["namespaceId", "tableBucketId"] {
            assert!(
                summary.get(container_id).is_none(),
                "case {namespace:?}/{name:?}: the summary carries {container_id}: {summary}"
            );
            assert!(
                read.body.get(container_id).is_some(),
                "case {namespace:?}/{name:?}: GetTable must report {container_id}: {body}",
                body = read.body
            );
        }
    }

    // Paging: every page size from one table to one past the resident count
    // walks the same stored order exactly once.
    for page_size in 1..=every_table.len() + 1 {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = vec![("maxTables", page_size.to_string())];
            if let Some(token) = &token {
                query.push(("continuationToken", token.clone()));
            }
            let listed = emulator.list_tables(REGION, &bucket_arn, &query).await;
            assert_eq!(
                listed.status, 200,
                "page size {page_size}: ListTables must succeed, got {status} {body}",
                status = listed.status,
                body = listed.body
            );
            let page = listed.list("tables");
            assert!(
                page.len() <= page_size,
                "page size {page_size}: a page must carry at most the requested number of \
                 tables, got {actual}",
                actual = page.len()
            );
            walked.extend(page.iter().map(|summary| {
                format!(
                    "{}/{}",
                    summary["namespace"][0].as_str().unwrap_or("<none>"),
                    summary["name"].as_str().unwrap_or("<none>")
                )
            }));
            pages += 1;
            assert!(
                pages <= every_table.len() + 2,
                "page size {page_size}: the walk must terminate, got {pages} pages"
            );
            token = listed.optional_text("continuationToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, every_table,
            "page size {page_size}: the walk must visit every stored table exactly once, in the \
             stored order, got {walked:?}"
        );
    }

    // A token this service never minted names no place in the listing.
    let refused = emulator
        .list_tables(
            REGION,
            &bucket_arn,
            &[("continuationToken", "not-a-token".to_string())],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "a continuation token this service did not mint must be refused, got {status} {body}",
        status = refused.status,
        body = refused.body
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("BadRequestException"),
        "the refusal must carry the modeled error code, got {actual:?}",
        actual = refused.error_code
    );
}

#[tokio::test]
async fn list_table_buckets_reports_the_stored_buckets_filtered_and_paged() {
    let emulator = Emulator::new();
    // Created out of order, so a listing that echoed the creates rather than the
    // store would come back in the wrong one.
    for name in ["prod-sales", "dev-sales", "prod-finance"] {
        let created = emulator
            .create_table_bucket(REGION, &json!({ "name": name }))
            .await;
        assert_eq!(
            created.status, 200,
            "CreateTableBucket {name:?} must succeed, got {status} {body}",
            status = created.status,
            body = created.body
        );
    }
    // One bucket in another region: this endpoint is regional, so it is not part
    // of this region's listing.
    let elsewhere = emulator
        .create_table_bucket("eu-west-1", &json!({ "name": "prod-elsewhere" }))
        .await
        .text("arn");

    let stored_order = vec!["dev-sales", "prod-finance", "prod-sales"];
    let cases = vec![
        (
            "no filter: every bucket in this region, in the stored order",
            vec![],
            stored_order.clone(),
        ),
        (
            "a prefix several names share",
            vec![("prefix", "prod-".to_string())],
            vec!["prod-finance", "prod-sales"],
        ),
        (
            "a prefix one name has",
            vec![("prefix", "dev".to_string())],
            vec!["dev-sales"],
        ),
        (
            "a prefix no name starts with",
            vec![("prefix", "staging".to_string())],
            vec![],
        ),
        (
            "the type every bucket a caller creates carries",
            vec![("type", TableBucketType::Customer.as_wire().to_string())],
            stored_order.clone(),
        ),
        (
            "the type only Amazon Web Services creates buckets with",
            vec![("type", TableBucketType::Aws.as_wire().to_string())],
            vec![],
        ),
    ];

    for (label, query, expected) in &cases {
        let listed = emulator.list_table_buckets(REGION, query).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: ListTableBuckets must succeed, got {status} {body}",
            status = listed.status,
            body = listed.body
        );
        let names: Vec<String> = listed
            .list("tableBuckets")
            .iter()
            .map(|summary| summary["name"].as_str().unwrap_or("<none>").to_string())
            .collect();
        assert_eq!(
            &names, expected,
            "case {label:?}: the listing must report the stored buckets in the stored order, got \
             {names:?}"
        );
    }

    // Every summary is built from the same record `GetTableBucket` reports.
    for summary in emulator
        .list_table_buckets(REGION, &[])
        .await
        .list("tableBuckets")
    {
        let arn = summary["arn"].as_str().expect("a bucket ARN");
        let read = emulator.get_table_bucket(REGION, arn).await;
        for member in ["arn", "name", "ownerAccountId", "createdAt", "tableBucketId", "type"] {
            assert_eq!(
                summary.get(member),
                read.body.get(member),
                "bucket {arn:?}: the summary's {member} must be the stored one"
            );
        }
    }

    // Paging, over every page size from one bucket to one past the resident count.
    for page_size in 1..=stored_order.len() + 1 {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![("maxBuckets", page_size.to_string())];
            if let Some(token) = &token {
                query.push(("continuationToken", token.clone()));
            }
            let listed = emulator.list_table_buckets(REGION, &query).await;
            assert_eq!(
                listed.status, 200,
                "page size {page_size}: ListTableBuckets must succeed, got {status} {body}",
                status = listed.status,
                body = listed.body
            );
            let page = listed.list("tableBuckets");
            assert!(
                page.len() <= page_size,
                "page size {page_size}: a page must carry at most the requested number of \
                 buckets, got {actual}",
                actual = page.len()
            );
            walked.extend(
                page.iter()
                    .map(|summary| summary["name"].as_str().unwrap_or("<none>").to_string()),
            );
            token = listed.optional_text("continuationToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, stored_order,
            "page size {page_size}: the walk must visit every stored bucket exactly once, in the \
             stored order, got {walked:?}"
        );
    }

    // The bucket in the other region is listed there, and only there.
    let other = emulator.list_table_buckets("eu-west-1", &[]).await;
    let other_names: Vec<String> = other
        .list("tableBuckets")
        .iter()
        .map(|summary| summary["name"].as_str().unwrap_or("<none>").to_string())
        .collect();
    assert_eq!(
        other_names,
        vec!["prod-elsewhere".to_string()],
        "a listing answers for the region the request names, got {other_names:?}"
    );
    assert_eq!(
        emulator.get_table_bucket("eu-west-1", &elsewhere).await.status,
        200,
        "the bucket created in the other region must still be readable by its ARN"
    );
}

#[tokio::test]
async fn a_delete_removes_one_record_and_is_refused_while_children_remain() {
    let emulator = Emulator::new();
    let bucket_arn = emulator
        .create_table_bucket(REGION, &json!({ "name": "lifecycle-bucket" }))
        .await
        .text("arn");
    for namespace in ["keep", "remove"] {
        emulator
            .create_namespace(REGION, &bucket_arn, &json!({ "namespace": [namespace] }))
            .await;
    }
    let mut version_tokens = Vec::new();
    for (namespace, table) in [("keep", "kept"), ("remove", "doomed"), ("remove", "sibling")] {
        let created = emulator
            .create_table(
                REGION,
                &bucket_arn,
                namespace,
                &json!({ "name": table, "format": "ICEBERG" }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "CreateTable {namespace:?}/{table:?} must succeed, got {status} {body}",
            status = created.status,
            body = created.body
        );
        version_tokens.push((table, created.text("versionToken"), created.text("tableARN")));
    }

    // A container that still holds records cannot be deleted. A bucket that
    // still holds namespaces is the CLIENT's request error: emptying it, not
    // retrying, is what makes the delete possible.
    let bucket_in_use = emulator.delete_table_bucket(REGION, &bucket_arn).await;
    assert_eq!(
        bucket_in_use.status, 400,
        "a bucket that still holds namespaces must not be deleted, got {status} {body}",
        status = bucket_in_use.status,
        body = bucket_in_use.body
    );
    assert_eq!(
        bucket_in_use.error_code.as_deref(),
        Some("BadRequestException"),
        "the refusal must carry the modeled error code, got {actual:?}",
        actual = bucket_in_use.error_code
    );
    let namespace_in_use = emulator
        .delete_namespace(REGION, &bucket_arn, "remove")
        .await;
    assert_eq!(
        namespace_in_use.status, 409,
        "a namespace that still holds tables must not be deleted, got {status} {body}",
        status = namespace_in_use.status,
        body = namespace_in_use.body
    );

    // A version token that is not the table's own is a conflict, and the table
    // is still there afterwards.
    let (doomed, doomed_token, doomed_arn) = version_tokens
        .iter()
        .find(|(table, ..)| *table == "doomed")
        .cloned()
        .expect("the row was created above");
    let stale = emulator
        .delete_table(
            REGION,
            &bucket_arn,
            "remove",
            doomed,
            &[("versionToken", format!("{doomed_token}0"))],
        )
        .await;
    assert_eq!(
        stale.status, 409,
        "a delete presenting another version token must be refused, got {status} {body}",
        status = stale.status,
        body = stale.body
    );
    assert_eq!(
        emulator
            .get_table(REGION, &bucket_arn, "remove", doomed)
            .await
            .status,
        200,
        "a refused delete must leave the table in place"
    );

    // The delete the table's own token authorizes removes exactly that record.
    let deleted = emulator
        .delete_table(
            REGION,
            &bucket_arn,
            "remove",
            doomed,
            &[("versionToken", doomed_token.clone())],
        )
        .await;
    assert_eq!(
        deleted.status, 204,
        "DeleteTable must answer the modeled 204, got {status} {body}",
        status = deleted.status,
        body = deleted.body
    );
    let gone = emulator
        .get_table(REGION, &bucket_arn, "remove", doomed)
        .await;
    assert_eq!(
        gone.status, 404,
        "a deleted table must not be found, got {status} {body}",
        status = gone.status,
        body = gone.body
    );
    assert_eq!(
        gone.error_code.as_deref(),
        Some("NotFoundException"),
        "the refusal must carry the modeled error code, got {actual:?}",
        actual = gone.error_code
    );
    assert_eq!(
        emulator.get_table_by_arn(REGION, &doomed_arn).await.status,
        404,
        "the deleted table's ARN must resolve to nothing"
    );
    let remaining: Vec<String> = emulator
        .list_tables(REGION, &bucket_arn, &[])
        .await
        .list("tables")
        .iter()
        .map(|summary| summary["name"].as_str().unwrap_or("<none>").to_string())
        .collect();
    assert_eq!(
        remaining,
        vec!["kept".to_string(), "sibling".to_string()],
        "only the deleted table may be gone, got {remaining:?}"
    );

    // Emptying the namespace is what lets it go, and emptying the bucket is what
    // lets IT go.
    assert_eq!(
        emulator
            .delete_table(REGION, &bucket_arn, "remove", "sibling", &[])
            .await
            .status,
        204,
        "a delete with no version token must succeed"
    );
    let namespace_deleted = emulator
        .delete_namespace(REGION, &bucket_arn, "remove")
        .await;
    assert_eq!(
        namespace_deleted.status, 204,
        "DeleteNamespace must answer the modeled 204, got {status} {body}",
        status = namespace_deleted.status,
        body = namespace_deleted.body
    );
    assert_eq!(
        emulator
            .get_namespace(REGION, &bucket_arn, "remove")
            .await
            .status,
        404,
        "a deleted namespace must not be found"
    );
    assert_eq!(
        emulator
            .get_namespace(REGION, &bucket_arn, "keep")
            .await
            .status,
        200,
        "only the deleted namespace may be gone"
    );
    assert_eq!(
        emulator.delete_table_bucket(REGION, &bucket_arn).await.status,
        400,
        "the bucket still holds a namespace, so it still cannot be deleted"
    );

    assert_eq!(
        emulator
            .delete_table(REGION, &bucket_arn, "keep", "kept", &[])
            .await
            .status,
        204,
        "the last table must delete"
    );
    assert_eq!(
        emulator
            .delete_namespace(REGION, &bucket_arn, "keep")
            .await
            .status,
        204,
        "the last namespace must delete"
    );
    let bucket_deleted = emulator.delete_table_bucket(REGION, &bucket_arn).await;
    assert_eq!(
        bucket_deleted.status, 204,
        "DeleteTableBucket must answer the modeled 204 once the bucket is empty, got {status} \
         {body}",
        status = bucket_deleted.status,
        body = bucket_deleted.body
    );
    assert_eq!(
        emulator.get_table_bucket(REGION, &bucket_arn).await.status,
        404,
        "a deleted bucket must not be found"
    );
    assert!(
        emulator
            .list_table_buckets(REGION, &[])
            .await
            .list("tableBuckets")
            .is_empty(),
        "a deleted bucket must be absent from the listing"
    );
}

#[tokio::test]
async fn an_unknown_parent_is_not_found_and_a_resident_name_conflicts() {
    let emulator = Emulator::new();
    let bucket_arn = emulator
        .create_table_bucket(REGION, &json!({ "name": "refusals-bucket" }))
        .await
        .text("arn");
    let absent_bucket_arn = expected_bucket_arn(REGION, "no-such-bucket");
    emulator
        .create_namespace(REGION, &bucket_arn, &json!({ "namespace": ["resident"] }))
        .await;
    let table = emulator
        .create_table(
            REGION,
            &bucket_arn,
            "resident",
            &json!({ "name": "resident", "format": "ICEBERG" }),
        )
        .await;
    let table_arn = table.text("tableARN");

    // Each row: the call, and the status and error code it must answer.
    let not_found = "NotFoundException";
    let conflict = "ConflictException";

    let cases: Vec<(&str, Reply, u16, &str)> = vec![
        (
            "a namespace in a bucket no create minted",
            emulator
                .create_namespace(
                    REGION,
                    &absent_bucket_arn,
                    &json!({ "namespace": ["orphan"] }),
                )
                .await,
            404,
            not_found,
        ),
        (
            "a second namespace of a resident name",
            emulator
                .create_namespace(REGION, &bucket_arn, &json!({ "namespace": ["resident"] }))
                .await,
            409,
            conflict,
        ),
        (
            "a table in a namespace no create minted",
            emulator
                .create_table(
                    REGION,
                    &bucket_arn,
                    "absent",
                    &json!({ "name": "orphan", "format": "ICEBERG" }),
                )
                .await,
            404,
            not_found,
        ),
        (
            "a table in a bucket no create minted",
            emulator
                .create_table(
                    REGION,
                    &absent_bucket_arn,
                    "resident",
                    &json!({ "name": "orphan", "format": "ICEBERG" }),
                )
                .await,
            404,
            not_found,
        ),
        (
            "a second table of a resident name in one namespace",
            emulator
                .create_table(
                    REGION,
                    &bucket_arn,
                    "resident",
                    &json!({ "name": "resident", "format": "ICEBERG" }),
                )
                .await,
            409,
            conflict,
        ),
        (
            "a namespace read in a bucket no create minted",
            emulator
                .get_namespace(REGION, &absent_bucket_arn, "resident")
                .await,
            404,
            not_found,
        ),
        (
            "a namespace read no create minted",
            emulator.get_namespace(REGION, &bucket_arn, "absent").await,
            404,
            not_found,
        ),
        (
            "a table read no create minted",
            emulator
                .get_table(REGION, &bucket_arn, "resident", "absent")
                .await,
            404,
            not_found,
        ),
        (
            "a table read by an ARN no create minted",
            emulator
                .get_table_by_arn(REGION, &format!("{bucket_arn}/table/0123456789abcdef"))
                .await,
            404,
            not_found,
        ),
        (
            "a table read that names neither addressing mode whole",
            emulator
                .get_table_with(REGION, &[("namespace", "resident".to_string())])
                .await,
            400,
            "BadRequestException",
        ),
        (
            "a listing of a bucket no create minted",
            emulator.list_tables(REGION, &absent_bucket_arn, &[]).await,
            404,
            not_found,
        ),
        (
            "a listing of a namespace no create minted",
            emulator
                .list_tables(REGION, &bucket_arn, &[("namespace", "absent".to_string())])
                .await,
            404,
            not_found,
        ),
        (
            "a table deleted in a namespace no create minted",
            emulator
                .delete_table(REGION, &bucket_arn, "absent", "resident", &[])
                .await,
            404,
            not_found,
        ),
        (
            "a table deleted that no create minted",
            emulator
                .delete_table(REGION, &bucket_arn, "resident", "absent", &[])
                .await,
            404,
            not_found,
        ),
        (
            "a namespace deleted that no create minted",
            emulator
                .delete_namespace(REGION, &bucket_arn, "absent")
                .await,
            404,
            not_found,
        ),
        (
            "a bucket deleted that no create minted",
            emulator
                .delete_table_bucket(REGION, &absent_bucket_arn)
                .await,
            404,
            not_found,
        ),
    ];

    for (label, reply, status, code) in &cases {
        assert_eq!(
            &reply.status, status,
            "case {label:?}: expected {status}, got {actual} {body}",
            actual = reply.status,
            body = reply.body
        );
        assert_eq!(
            reply.error_code.as_deref(),
            Some(*code),
            "case {label:?}: the refusal must carry the modeled error code, got {actual:?}",
            actual = reply.error_code
        );
        assert!(
            !reply.message().is_empty() && reply.message() != "<no message>",
            "case {label:?}: the refusal must say what was wrong"
        );
    }

    // None of those refusals changed anything: the one namespace and the one
    // table are still exactly as they were created.
    assert_eq!(
        emulator
            .get_table_by_arn(REGION, &table_arn)
            .await
            .text("versionToken"),
        table.text("versionToken"),
        "a refused call must not have changed the resident table"
    );
    let tables: Vec<String> = emulator
        .list_tables(REGION, &bucket_arn, &[])
        .await
        .list("tables")
        .iter()
        .map(|summary| summary["name"].as_str().unwrap_or("<none>").to_string())
        .collect();
    assert_eq!(
        tables,
        vec!["resident".to_string()],
        "a refused call must not have stored anything, got {tables:?}"
    );
}
