// SPDX-License-Identifier: CC0-1.0

//! What a collection's OWN policies decide about a request to its endpoint, end to end over
//! the wire.
//!
//! The rule under test, which is AWS's own order for the two policies a collection carries:
//!
//! * a `network` security policy is what admits traffic to a collection's endpoints — "you
//!   must have at least one network policy that applies to a collection in order to access
//!   its endpoint" (AWS's "Network access for Amazon OpenSearch Serverless") — so a request
//!   to a collection no network policy covers is refused, whichever principal signed it and
//!   whatever route it asks for;
//! * a `data` access policy is what grants a PRINCIPAL a permission on the collection and
//!   its indexes (AWS's "Data access control for Amazon OpenSearch Serverless"), so a
//!   request a policy grants no covering permission to is refused even where the endpoint is
//!   reachable.
//!
//! Each table spans what the rule generalises over rather than one recorded request: the
//! first over which resources a network policy names (none at all, another collection,
//! the collection itself, every collection, a wildcarded prefix), the second over which
//! principal and which permissions a data access policy grants, crossed with the route the
//! request asks for — an index create, a search and a document write take three different
//! permissions.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{at, call, text};

/// This service's SigV4 signing name, which is what a client of a collection endpoint signs
/// with (`smithy_query(query="service_info")` -> `sigv4Name`).
const SIGNING_NAME: &str = "aoss";

/// The region every call of this suite is made in.
const REGION: &str = "us-east-1";

/// How many times a read looks again for a transition the clock owes. Generous: the suite
/// waits for the CONDITION and asserts nothing about how long it took.
const SETTLE_ATTEMPTS: usize = 200;

/// How long one look waits before the next.
const SETTLE_PAUSE: std::time::Duration = std::time::Duration::from_millis(50);

/// This service and IAM on ONE engine — the shape `core/router/src/lib.rs` mounts them in.
///
/// IAM is here because a data access policy names IAM principals: the suite creates a user
/// and an access key through IAM's own API and signs the data-plane request with it, which
/// is what makes the principal the data plane resolves a record `iam:CreateAccessKey`
/// really wrote.
fn emulator() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_opensearchserverless_opensearchserverless::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router
}

/// Percent-encode one form value the way an AWS SDK encodes an `awsQuery` body.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(*byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// One IAM call: IAM speaks `awsQuery`, so its parameters travel as a form body and its
/// answer comes back as XML.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("iam.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/iam/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed IAM request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The text of the first `<tag>` of an XML body.
fn xml_text(xml: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(start) = xml.find(&open).map(|at| at + open.len()) else {
        return String::new();
    };
    let Some(end) = xml[start..].find(&close).map(|at| at + start) else {
        return String::new();
    };
    xml[start..end].to_string()
}

/// An IAM user and a long-term access key of its own: the principal a data access policy
/// names, and the access key id a request of that principal signs with.
async fn iam_user(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, created) = iam(router, "CreateUser", &[("UserName", name)]).await;
    assert_eq!(status, 200, "case CreateUser({name}): got {status} {created}");
    let arn = xml_text(&created, "Arn");
    let (status, key) = iam(router, "CreateAccessKey", &[("UserName", name)]).await;
    assert_eq!(
        status, 200,
        "case CreateAccessKey({name}): got {status} {key}"
    );
    let access_key_id = xml_text(&key, "AccessKeyId");
    assert!(
        !arn.is_empty() && !access_key_id.is_empty(),
        "case {name}: expected an ARN and an access key id, got {arn:?} and {access_key_id:?}"
    );
    (arn, access_key_id)
}

/// Create a security policy of `kind`, asserting it was accepted.
async fn create_security_policy(
    router: &MultiServiceRouter,
    kind: &str,
    name: &str,
    policy: &Value,
) {
    let (status, code, answer) = call(
        router,
        "CreateSecurityPolicy",
        json!({ "type": kind, "name": name, "policy": policy.to_string() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateSecurityPolicy({kind}/{name}): got {status} {code} with {answer}"
    );
}

/// Create a `data` access policy, asserting it was accepted.
async fn create_access_policy(router: &MultiServiceRouter, name: &str, policy: &Value) {
    let (status, code, answer) = call(
        router,
        "CreateAccessPolicy",
        json!({ "type": "data", "name": name, "policy": policy.to_string() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateAccessPolicy(data/{name}): got {status} {code} with {answer}"
    );
}

/// A `network` security policy document naming `resources`, with public access — the shape
/// AWS's "Network access for Amazon OpenSearch Serverless" documents.
fn network_policy(resources: &[&str]) -> Value {
    json!({
        "Rules": [{ "ResourceType": "collection", "Resource": resources }],
        "AllowFromPublic": true,
    })
}

/// A `data` access policy document granting `permissions` on `resources` of `resource_type`
/// to `principals` — the shape AWS's "Data access control for Amazon OpenSearch Serverless"
/// documents.
fn data_policy(
    resource_type: &str,
    resources: &[&str],
    permissions: &[&str],
    principals: &[&str],
) -> Value {
    json!([{
        "Rules": [{
            "ResourceType": resource_type,
            "Resource": resources,
            "Permission": permissions,
        }],
        "Principal": principals,
    }])
}

/// Create a collection and poll until it reports ACTIVE, answering its `CollectionDetail`.
async fn active_collection(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, code, created) = call(
        router,
        "CreateCollection",
        json!({ "name": name, "type": "VECTORSEARCH", "encryptionConfig": { "aWSOwnedKey": true } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateCollection({name}): got {status} {code} with {created}"
    );
    let id = text(&created, "createCollectionDetail.id");
    for _ in 0..SETTLE_ATTEMPTS {
        let (status, code, read) = call(router, "BatchGetCollection", json!({ "ids": [id] })).await;
        assert_eq!(
            status, 200,
            "case BatchGetCollection({id}): got {status} {code} with {read}"
        );
        let detail = at(&read, "collectionDetails")
            .as_array()
            .and_then(|details| details.first())
            .cloned()
            .unwrap_or(Value::Null);
        if text(&detail, "status") == "ACTIVE" {
            return detail;
        }
        tokio::time::sleep(SETTLE_PAUSE).await;
    }
    panic!("case {name}: the collection never reached ACTIVE");
}

/// The host a published endpoint names, which is what a client of it sends as `Host`.
fn endpoint_host(detail: &Value) -> String {
    let endpoint = text(detail, "collectionEndpoint");
    let after_scheme = endpoint.strip_prefix("https://").unwrap_or(&endpoint);
    after_scheme
        .split('/')
        .next()
        .unwrap_or(after_scheme)
        .to_string()
}

/// One OpenSearch REST request to a collection endpoint, as `opensearch-py` sends it:
/// addressed to the published host, signed for the `aoss` signing name presenting `akid`,
/// and carrying NO `x-amz-target`, because the REST API has no RPC target.
async fn rest(
    router: &MultiServiceRouter,
    host: &str,
    akid: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(
            body.map(|body| body.to_string()).unwrap_or_default(),
        ))
        .expect("a well-formed data-plane request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// A `knn_vector` index body of `dimension` values — the body the recorded `indices.create`
/// of a knowledge base's vector index carries.
fn knn_index_body(dimension: usize) -> Value {
    json!({
        "settings": { "index": { "knn": true } },
        "mappings": {
            "properties": {
                "vector": { "type": "knn_vector", "dimension": dimension },
            }
        },
    })
}

/// The access key id a request of the account's own published credential presents: one no
/// IAM record names, which is the position an account root credential is in.
const AMBIENT_ACCESS_KEY_ID: &str = "AKIDAMBIENT";

/// RULE: a collection's endpoints are reachable only through a `network` security policy
/// whose rules name that collection; a request to a collection no such policy covers is
/// refused 401 and nothing it asks for is performed.
///
/// The rows span which resources the stored policy names.
#[tokio::test]
async fn an_endpoint_answers_only_through_a_network_policy_that_names_its_collection() {
    /// One case: what the network policy names, and whether the endpoint then answers.
    struct Row {
        case: &'static str,
        /// The `Resource` patterns the policy's rules name, or `None` for no policy at all.
        names: Option<&'static [&'static str]>,
        reachable: bool,
    }
    let rows: &[Row] = &[
        Row {
            case: "no network policy at all",
            names: None,
            reachable: false,
        },
        Row {
            case: "a policy naming another collection",
            names: Some(&["collection/some-other-collection"]),
            reachable: false,
        },
        Row {
            case: "a policy naming this collection",
            names: Some(&["collection/net-gated"]),
            reachable: true,
        },
        Row {
            case: "a policy naming every collection",
            names: Some(&["collection/*"]),
            reachable: true,
        },
        Row {
            case: "a policy naming a prefix of this collection",
            names: Some(&["collection/net-*"]),
            reachable: true,
        },
        Row {
            case: "a policy naming another collection beside this one",
            names: Some(&["collection/elsewhere", "collection/net-gated"]),
            reachable: true,
        },
    ];
    for Row {
        case,
        names,
        reachable,
    } in rows
    {
        let router = emulator();
        if let Some(names) = names {
            create_security_policy(&router, "network", "net-policy", &network_policy(names)).await;
        }
        let collection = active_collection(&router, "net-gated").await;
        let host = endpoint_host(&collection);
        let (status, answer) = rest(
            &router,
            &host,
            AMBIENT_ACCESS_KEY_ID,
            "PUT",
            "/gated-index",
            Some(&knn_index_body(8)),
        )
        .await;
        let expected = if *reachable { 200 } else { 401 };
        assert_eq!(
            status, expected,
            "case {case}: expected PUT /gated-index to answer {expected}, got {status} with \
             {answer}"
        );

        // Nothing the refused request asked for was performed: the index is not there.
        if !*reachable {
            let (held, body) = rest(
                &router,
                &host,
                AMBIENT_ACCESS_KEY_ID,
                "HEAD",
                "/gated-index",
                None,
            )
            .await;
            assert_eq!(
                held, 401,
                "case {case}: every route of an unreachable endpoint is refused alike, got \
                 {held} with {body}"
            );
        }
    }
}

/// RULE: a `data` access policy is what grants a principal a permission on a collection's
/// indexes; a request whose principal no stored policy grants the permission its route takes
/// is refused 403, and the same request is served once a policy grants it.
///
/// The rows span the principal a policy names, the permissions it grants and the route the
/// request asks for — three routes that take three different permissions.
#[tokio::test]
async fn an_index_request_is_served_only_where_a_data_access_policy_grants_its_permission() {
    /// Which route the case asks for.
    #[derive(Debug, Clone, Copy)]
    enum Route {
        /// `PUT /<index>`, which takes `aoss:CreateIndex`.
        CreateIndex,
        /// `GET /<index>/_search`, which takes `aoss:ReadDocument`.
        Search,
        /// `POST /<index>/_doc`, which takes `aoss:WriteDocument`.
        WriteDocument,
    }

    /// One case: what the data access policy grants, which route is asked for, and the
    /// status the data plane answers.
    struct Row {
        case: &'static str,
        /// The permissions the policy grants, or `None` for no data access policy at all.
        granted: Option<&'static [&'static str]>,
        /// Whether the policy names the signing principal; `false` names another one.
        names_the_caller: bool,
        route: Route,
        expected: u16,
    }
    let rows: &[Row] = &[
        Row {
            case: "no data access policy at all",
            granted: None,
            names_the_caller: true,
            route: Route::CreateIndex,
            expected: 403,
        },
        Row {
            case: "a policy naming another principal",
            granted: Some(&["aoss:CreateIndex"]),
            names_the_caller: false,
            route: Route::CreateIndex,
            expected: 403,
        },
        Row {
            case: "a policy granting the create to this principal",
            granted: Some(&["aoss:CreateIndex"]),
            names_the_caller: true,
            route: Route::CreateIndex,
            expected: 200,
        },
        Row {
            case: "a policy granting every aoss permission",
            granted: Some(&["aoss:*"]),
            names_the_caller: true,
            route: Route::CreateIndex,
            expected: 200,
        },
        Row {
            case: "a policy granting only the document read, asked for the index create",
            granted: Some(&["aoss:ReadDocument"]),
            names_the_caller: true,
            route: Route::CreateIndex,
            expected: 403,
        },
        Row {
            case: "a policy granting the document read, asked for a search",
            granted: Some(&["aoss:CreateIndex", "aoss:ReadDocument"]),
            names_the_caller: true,
            route: Route::Search,
            expected: 200,
        },
        Row {
            case: "a policy withholding the document read, asked for a search",
            granted: Some(&["aoss:CreateIndex", "aoss:WriteDocument"]),
            names_the_caller: true,
            route: Route::Search,
            expected: 403,
        },
        Row {
            case: "a policy withholding the document write, asked for a write",
            granted: Some(&["aoss:CreateIndex", "aoss:ReadDocument"]),
            names_the_caller: true,
            route: Route::WriteDocument,
            expected: 403,
        },
        Row {
            case: "a policy granting the document write, asked for a write",
            granted: Some(&["aoss:CreateIndex", "aoss:WriteDocument"]),
            names_the_caller: true,
            route: Route::WriteDocument,
            expected: 201,
        },
    ];
    for Row {
        case,
        granted,
        names_the_caller,
        route,
        expected,
    } in rows
    {
        let router = emulator();
        // The endpoint is reachable in every row, so what each asserts is the data-access
        // decision alone.
        create_security_policy(
            &router,
            "network",
            "net-policy",
            &network_policy(&["collection/*"]),
        )
        .await;
        let (user_arn, access_key_id) = iam_user(&router, "index-writer").await;
        let (other_arn, _) = iam_user(&router, "someone-else").await;
        if let Some(granted) = granted {
            let principal = if *names_the_caller {
                user_arn.as_str()
            } else {
                other_arn.as_str()
            };
            create_access_policy(
                &router,
                "data-policy",
                &data_policy(
                    "index",
                    &["index/data-gated/*"],
                    granted,
                    &[principal],
                ),
            )
            .await;
        }
        let collection = active_collection(&router, "data-gated").await;
        let host = endpoint_host(&collection);

        // The index itself is created by the account's own published credential, which the
        // data access policies do not bound — so a row about a READ or a WRITE is about that
        // permission and not about whether the index is there.
        if !matches!(route, Route::CreateIndex) {
            let (status, answer) = rest(
                &router,
                &host,
                AMBIENT_ACCESS_KEY_ID,
                "PUT",
                "/data-gated-index",
                Some(&knn_index_body(4)),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: setup expected the index create accepted, got {status} with \
                 {answer}"
            );
        }

        let (method, path, body) = match route {
            Route::CreateIndex => ("PUT", "/data-gated-index".to_string(), Some(knn_index_body(4))),
            Route::Search => (
                "GET",
                "/data-gated-index/_search".to_string(),
                Some(json!({ "query": { "match_all": {} } })),
            ),
            Route::WriteDocument => (
                "POST",
                "/data-gated-index/_doc".to_string(),
                Some(json!({ "vector": [1.0, 0.0, 0.0, 0.0] })),
            ),
        };
        let (status, answer) =
            rest(&router, &host, &access_key_id, method, &path, body.as_ref()).await;
        assert_eq!(
            status, *expected,
            "case {case}: expected {method} {path} to answer {expected}, got {status} with \
             {answer}"
        );
    }
}

/// RULE: the account's own published credential is not bounded by the data access policies —
/// the position an AWS account root credential is in — so a collection with no data access
/// policy at all still serves it, while the network policy still decides whether its
/// endpoint answers at all.
#[tokio::test]
async fn the_accounts_own_credential_is_bounded_by_the_network_policy_alone() {
    let router = emulator();
    create_security_policy(
        &router,
        "network",
        "net-policy",
        &network_policy(&["collection/*"]),
    )
    .await;
    let collection = active_collection(&router, "root-reachable").await;
    let host = endpoint_host(&collection);
    let (status, answer) = rest(
        &router,
        &host,
        AMBIENT_ACCESS_KEY_ID,
        "PUT",
        "/root-index",
        Some(&knn_index_body(4)),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the account's own credential with no data access policy: expected 200, got \
         {status} with {answer}"
    );
    let (status, answer) = rest(
        &router,
        &host,
        AMBIENT_ACCESS_KEY_ID,
        "HEAD",
        "/root-index",
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case reading the created index back: expected 200, got {status} with {answer}"
    );
}
