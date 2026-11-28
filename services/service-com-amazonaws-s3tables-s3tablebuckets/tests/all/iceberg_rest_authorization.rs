// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AUTHORIZATION decision the Iceberg REST
//! catalog is served behind.
//!
//! RULE: the catalog is a data plane this service serves out of the SAME records
//! its control plane reads and writes, so every endpoint of it is decided against
//! the policies in force on the calling principal and on what the request
//! addresses — exactly as every modelled operation is
//! (`request_authorization.rs`). The model binds no operation to the catalog's
//! paths, so the decision cannot ride on the generated routing table: it is taken
//! inside the catalog, under the control-plane action whose records the endpoint
//! reaches (`s3tables:GetTableBucket` for the warehouse's configuration,
//! `s3tables:ListNamespaces`, `s3tables:GetNamespace`, `s3tables:ListTables` for
//! the discovery endpoints, `s3tables:GetTableMetadataLocation` for a load,
//! `s3tables:GetTable` for an existence check and
//! `s3tables:UpdateTableMetadataLocation` for a commit) and against the resource
//! it acts on — the table when the request addresses one, else the table bucket.
//! A refusal is HTTP 403 `AccessDeniedException` in the catalog's own error
//! envelope, carrying IAM's sentence, and it reports nothing of the table.
//!
//! The table spans what the rule generalises over: every endpoint of the catalog
//! (its three table methods and its four discovery endpoints) against a principal
//! carrying no grant, and the same endpoints against one granted every action of
//! this service.

use serde_json::{json, Value};

use super::harness::encode;
use super::table_encryption_authorization::{
    assumed_role, statement, Emulator, Principal, ACCOUNT_KEY,
};

/// The error type a refusal of the catalog names the class by: the name of the
/// shape this service's model declares for an authorization refusal, which is
/// what the same caller reads from the control plane for this class.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The namespace and the table the catalog is addressed for.
const NAMESPACE: &str = "catalog_authz";
const TABLE: &str = "events";

/// One endpoint of the catalog: its case, the method and path it is addressed at,
/// the action it is performed under, and whether it acts on the TABLE (rather
/// than on the table bucket).
struct Endpoint {
    case: &'static str,
    method: &'static str,
    path: String,
    action: &'static str,
    on_the_table: bool,
    body: Option<Value>,
}

/// The path of the catalog's table endpoint for one table.
fn table_path(bucket_arn: &str) -> String {
    format!(
        "/iceberg/v1/{}/namespaces/{}/tables/{}",
        encode(bucket_arn),
        encode(NAMESPACE),
        encode(TABLE)
    )
}

/// Every endpoint of the catalog, addressed for `bucket_arn`.
fn endpoints(bucket_arn: &str) -> Vec<Endpoint> {
    let prefix = format!("/iceberg/v1/{}", encode(bucket_arn));
    vec![
        Endpoint {
            case: "the warehouse's configuration",
            method: "GET",
            path: format!(
                "/iceberg/v1/config?warehouse={}",
                encode(bucket_arn)
            ),
            action: "s3tables:GetTableBucket",
            on_the_table: false,
            body: None,
        },
        Endpoint {
            case: "the warehouse's namespaces",
            method: "GET",
            path: format!("{prefix}/namespaces"),
            action: "s3tables:ListNamespaces",
            on_the_table: false,
            body: None,
        },
        Endpoint {
            case: "one namespace",
            method: "GET",
            path: format!("{prefix}/namespaces/{}", encode(NAMESPACE)),
            action: "s3tables:GetNamespace",
            on_the_table: false,
            body: None,
        },
        Endpoint {
            case: "a namespace's tables",
            method: "GET",
            path: format!("{prefix}/namespaces/{}/tables", encode(NAMESPACE)),
            action: "s3tables:ListTables",
            on_the_table: false,
            body: None,
        },
        Endpoint {
            case: "a table's metadata",
            method: "GET",
            path: table_path(bucket_arn),
            action: "s3tables:GetTableMetadataLocation",
            on_the_table: true,
            body: None,
        },
        Endpoint {
            case: "whether a table exists",
            method: "HEAD",
            path: table_path(bucket_arn),
            action: "s3tables:GetTable",
            on_the_table: true,
            body: None,
        },
        Endpoint {
            case: "a commit to a table",
            method: "POST",
            path: table_path(bucket_arn),
            action: "s3tables:UpdateTableMetadataLocation",
            on_the_table: true,
            body: Some(json!({ "requirements": [], "updates": [] })),
        },
    ]
}

/// IAM's sentence for a request no readable identity-based document allows.
fn refusal_sentence(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// The catalog's error document, read out of its own envelope
/// (`error.message`, `error.type`).
fn catalog_error(body: &Value, case: &str) -> (String, String) {
    let error = body
        .get("error")
        .unwrap_or_else(|| panic!("case {case:?}: a refusal carries the catalog's error: {body}"));
    let field = |name: &str| {
        error
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the error names its {name}: {error}"))
            .to_string()
    };
    (field("message"), field("type"))
}

/// Create a table bucket, a namespace and one table carrying metadata, so every
/// endpoint below has something to answer for. Answers `(bucket ARN, table ARN)`.
async fn fixture(aws: &Emulator) -> (String, String) {
    let bucket_arn = aws
        .ok(
            "PUT",
            "/buckets",
            Some(&json!({ "name": "catalog-authz" })),
        )
        .await
        .text("arn");
    aws.ok(
        "PUT",
        &format!("/namespaces/{}", encode(&bucket_arn)),
        Some(&json!({ "namespace": [NAMESPACE] })),
    )
    .await;
    aws.ok(
        "PUT",
        &format!(
            "/tables/{}/{}",
            encode(&bucket_arn),
            encode(NAMESPACE)
        ),
        Some(&json!({
            "name": TABLE,
            "format": "ICEBERG",
            "metadata": { "iceberg": { "schema": { "fields": [
                { "name": "user_id", "type": "string", "required": true },
            ]}}},
        })),
    )
    .await;
    let read = aws
        .ok(
            "GET",
            &format!(
                "/get-table?tableBucketARN={}&namespace={}&name={}",
                encode(&bucket_arn),
                encode(NAMESPACE),
                encode(TABLE)
            ),
            None,
        )
        .await;
    (bucket_arn, read.text("tableARN"))
}

/// RULE: a principal carrying no grant is refused at EVERY endpoint of the
/// catalog, with IAM's sentence naming that endpoint's action and the resource it
/// acts on — and the refusal reports nothing of what it addressed.
#[tokio::test]
async fn every_catalog_endpoint_is_refused_to_a_principal_with_no_grant() {
    let aws = Emulator::new();
    let (bucket_arn, table_arn) = fixture(&aws).await;
    let principal: Principal = assumed_role(&aws, "catalog-authz-none", &[]).await;

    for endpoint in endpoints(&bucket_arn) {
        let case = endpoint.case;
        let refused = aws
            .call_as(
                &principal.access_key_id,
                endpoint.method,
                &endpoint.path,
                endpoint.body.as_ref(),
            )
            .await;
        assert_eq!(
            refused.status, 403,
            "case {case:?}: a principal with no grant is refused, got {status} {body}",
            status = refused.status,
            body = refused.body
        );
        let (message, kind) = catalog_error(&refused.body, case);
        assert_eq!(
            kind, ACCESS_DENIED,
            "case {case:?}: the refusal names the class this service states it with, got {kind:?}"
        );
        let resource = if endpoint.on_the_table {
            &table_arn
        } else {
            &bucket_arn
        };
        assert_eq!(
            message,
            refusal_sentence(&principal.arn, endpoint.action, resource),
            "case {case:?}: expected IAM's sentence for this endpoint's action and resource, \
             got {message:?}"
        );
        // A refusal reports nothing of what the request addressed.
        for member in ["metadata", "metadata-location", "namespaces", "identifiers"] {
            assert_eq!(
                refused.body.get(member),
                None,
                "case {case:?}: a refusal must report no {member}, got {body}",
                body = refused.body
            );
        }
    }
}

/// RULE: the same endpoints are SERVED to a principal the policies in force
/// allow — so the refusals above are the decision and not the catalog being
/// closed.
#[tokio::test]
async fn every_catalog_endpoint_is_served_to_a_principal_the_policies_allow() {
    let aws = Emulator::new();
    let (bucket_arn, _) = fixture(&aws).await;
    let granted: Principal = assumed_role(
        &aws,
        "catalog-authz-granted",
        &[statement("Allow", "s3tables:*", "*")],
    )
    .await;

    for endpoint in endpoints(&bucket_arn) {
        let case = endpoint.case;
        // What the ACCOUNT's own credential — which holds no IAM policies and is
        // therefore never refused — is answered for the same request, which is
        // the answer a grant must make the principal's call reach too.
        let unbounded = aws
            .call_as(
                ACCOUNT_KEY,
                endpoint.method,
                &endpoint.path,
                endpoint.body.as_ref(),
            )
            .await;
        let served = aws
            .call_as(
                &granted.access_key_id,
                endpoint.method,
                &endpoint.path,
                endpoint.body.as_ref(),
            )
            .await;
        assert_eq!(
            (served.status, &served.body),
            (unbounded.status, &unbounded.body),
            "case {case:?}: a granted principal is answered what the account's own credential \
             is, got {status} {body}",
            status = served.status,
            body = served.body
        );
        assert_ne!(
            served.status, 403,
            "case {case:?}: a granted principal must not be refused, got {body}",
            body = served.body
        );
    }

    // A grant for ANOTHER action of this service allows none of the catalog's
    // endpoints: the decision is read for the action the endpoint takes.
    let elsewhere: Principal = assumed_role(
        &aws,
        "catalog-authz-elsewhere",
        &[statement("Allow", "s3tables:CreateTableBucket", "*")],
    )
    .await;
    for endpoint in endpoints(&bucket_arn) {
        let case = endpoint.case;
        let refused = aws
            .call_as(
                &elsewhere.access_key_id,
                endpoint.method,
                &endpoint.path,
                endpoint.body.as_ref(),
            )
            .await;
        assert_eq!(
            refused.status, 403,
            "case {case:?}: a grant for another action allows no endpoint of the catalog, got \
             {status} {body}",
            status = refused.status,
            body = refused.body
        );
    }
}
