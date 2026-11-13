// SPDX-License-Identifier: CC0-1.0

//! Amazon OpenSearch Service and the legacy Amazon Elasticsearch Service are TWO emulated
//! services behind ONE endpoint.
//!
//! `smithy_query(query="service_info")` on either model gives sigv4Name `es`,
//! endpointPrefix `es` and arnNamespace `es`, protocol `restJson1`; they differ only in
//! `sdkId` (`OpenSearch` / `Elasticsearch Service`) and in the API version they declare
//! (`2021-01-01` / `2015-01-01`). AWS serves both APIs on the one
//! `es.<region>.amazonaws.com` endpoint and every SDK for either signs that one name, so
//! the router — which resolves a request's routing key from the SigV4 credential scope
//! first — handed BOTH services the key `es`. With the generated signing-name-only claim on
//! each, every request to the endpoint had two claimants and was refused `500
//! AmbiguousService`: a code no AWS model declares, and one botocore retries, so a client
//! burnt its whole retry budget before failing. Neither API was reachable at all.
//!
//! The rule this suite pins: **a signing name shared by several emulated services resolves
//! to EXACTLY ONE of them, decided by the path space that service's own model publishes.**
//! It is how AWS itself dispatches behind that one endpoint — every operation of the modern
//! model is bound under `/2021-01-01/…` and every operation of the legacy model under
//! `/2015-01-01/…` (`smithy_query(query="digest")` on either), so the two first URI path
//! segments are disjoint.
//!
//! Two layers assert it:
//!
//! * [`exactly_one_service_claims_each_request_for_the_es_key`] asks each service's
//!   [`smithy_server_runtime::Service::can_handle`] directly, so a row pins both that the
//!   right service claims a request and that NO other does — the condition the router turns
//!   into `500 AmbiguousService` when it fails. Its rows span the three signals a routing
//!   key is resolved from (a credential scope, a presigned query string, the `Host` label),
//!   both API versions, the path families both models bind under their own version
//!   (`tags`, `domain`) and paths no model publishes.
//! * [`the_es_endpoint_serves_both_api_versions`] drives real requests through the WHOLE
//!   router — the ambiguity existed only with both services mounted, and only the whole
//!   router proves no THIRD mounted service claims `es` either — and pins that each answer
//!   comes from the one model that owns the path: a success out of that model's own shapes,
//!   that model's own modelled refusal, or its scaffold `501 NotImplemented` naming the
//!   operation it resolved.
//!   Never a router-level refusal.
//!
//! The dispatch rows run IN ORDER against one router over one engine, so a later row reads
//! what an earlier one wrote: each API creates a domain and reads it back, which the
//! ambiguity refusal made impossible — the request never reached a handler at all.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every signed request below is signed for.
const REGION: &str = "us-east-1";

/// The shared SigV4 signing name / endpoint prefix: `es`.
const SHARED_KEY: &str = "es";

/// The SigV4 `Authorization` header of a request signed for `signing_name`, as every AWS
/// SDK sends it. The signature is never checked here: the claim predicate reads only the
/// credential scope.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// How a row names the service its request is for.
#[derive(Debug, Clone, Copy)]
enum Claiming {
    /// A SigV4 `Authorization` scope naming this routing key — the signal the router reads
    /// first, and what every AWS SDK sends.
    Signed(&'static str),
    /// A presigned URL's `X-Amz-Credential` query parameter naming [`SHARED_KEY`].
    Presigned,
    /// No credentials at all, so the `Host` label resolves the key.
    Anonymous,
}

/// The two emulated services that share the `es` key, by the short name the claim table
/// below names each one in.
const OPENSEARCH: &str = "opensearch";
const ELASTICSEARCH: &str = "elasticsearch";

#[test]
fn exactly_one_service_claims_each_request_for_the_es_key() {
    use smithy_server_runtime::Service;

    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let opensearch =
        service_com_amazonaws_opensearch_amazonopensearchservice::EmulatedService::new(Arc::clone(
            &engine,
        ));
    let elasticsearch =
        service_com_amazonaws_elasticsearchservice_amazonelasticsearchservice2015::EmulatedService::new(
            engine,
        );

    /// A presigned credential scope naming the shared signing name, with its `/`
    /// separators percent-encoded as a presigned URL carries them.
    const PRESIGNED: &str =
        "X-Amz-Credential=AKIDEXAMPLE%2F20260101%2Fus-east-1%2Fes%2Faws4_request";

    // (case, how the request is addressed, its Host, its URI path, the services that must
    // claim it)
    //
    // `can_handle` is handed the request's parts, and the claim reads the routing key and
    // the URI only — not the method — so a row names no method. The rows differ along what
    // the rule generalises over: the API version in the path, the path FAMILY under that
    // version (an operation-specific space, and the `tags`/`domain` families BOTH models
    // bind under their own version), and which of the three signals resolves the key.
    let rows: [(&str, Claiming, &str, &str, &[&str]); 14] = [
        (
            // The request the issue reports as `500 AmbiguousService`: CreateApplication.
            "CreateApplication on the shared signing name",
            Claiming::Signed(SHARED_KEY),
            "emulator.local",
            "/2021-01-01/opensearch/application",
            &[OPENSEARCH],
        ),
        (
            "StartMigration/ListMigrations, the app-migrations collection",
            Claiming::Signed(SHARED_KEY),
            "emulator.local",
            "/2021-01-01/opensearch/app-migrations",
            &[OPENSEARCH],
        ),
        (
            "GetMigration, one migration under that collection",
            Claiming::Signed(SHARED_KEY),
            "emulator.local",
            "/2021-01-01/opensearch/app-migrations/some-migration-id",
            &[OPENSEARCH],
        ),
        (
            // `tags` is bound by BOTH models — only the version segment separates them.
            "the tags family at the modern version",
            Claiming::Signed(SHARED_KEY),
            "emulator.local",
            "/2021-01-01/tags",
            &[OPENSEARCH],
        ),
        (
            "the tags family at the legacy version",
            Claiming::Signed(SHARED_KEY),
            "emulator.local",
            "/2015-01-01/tags",
            &[ELASTICSEARCH],
        ),
        (
            // …and so is `domain`: `ListDomainNames` is declared by both models, at the
            // same path under each one's own version.
            "the domain family at the modern version",
            Claiming::Signed(SHARED_KEY),
            "emulator.local",
            "/2021-01-01/domain",
            &[OPENSEARCH],
        ),
        (
            "the domain family at the legacy version",
            Claiming::Signed(SHARED_KEY),
            "emulator.local",
            "/2015-01-01/domain",
            &[ELASTICSEARCH],
        ),
        (
            "a legacy-model operation under its own es path space",
            Claiming::Signed(SHARED_KEY),
            "emulator.local",
            "/2015-01-01/es/domain/legacy-domain/config",
            &[ELASTICSEARCH],
        ),
        (
            "a presigned request at the modern version",
            Claiming::Presigned,
            "es.eu-west-2.amazonaws.com",
            "/2021-01-01/opensearch/domain",
            &[OPENSEARCH],
        ),
        (
            "a presigned request at the legacy version",
            Claiming::Presigned,
            "es.eu-west-2.amazonaws.com",
            "/2015-01-01/es/domain",
            &[ELASTICSEARCH],
        ),
        (
            // No credentials at all: the `Host` label resolves the key to `es`, and the
            // path still decides which of the two serves it.
            "an anonymous request to the es host at the modern version",
            Claiming::Anonymous,
            "es.us-east-1.amazonaws.com",
            "/2021-01-01/opensearch/application",
            &[OPENSEARCH],
        ),
        (
            "an anonymous request to the es host at the legacy version",
            Claiming::Anonymous,
            "es.us-east-1.amazonaws.com",
            "/2015-01-01/es/domain",
            &[ELASTICSEARCH],
        ),
        (
            // An API version NEITHER model publishes: claimed by nobody, which is the
            // router's `404 UnknownService` and NOT an ambiguity.
            "an API version neither model declares",
            Claiming::Signed(SHARED_KEY),
            "emulator.local",
            "/2099-01-01/opensearch/application",
            &[],
        ),
        (
            // The ROOT path: both models are REST, so neither binds `POST /`.
            "the root path on the shared signing name",
            Claiming::Signed(SHARED_KEY),
            "emulator.local",
            "/",
            &[],
        ),
    ];

    for (case, claiming, host, path, expected) in rows {
        let uri = match claiming {
            Claiming::Presigned => format!("{path}?{PRESIGNED}"),
            _ => path.to_string(),
        };
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", host);
        if let Claiming::Signed(signing_name) = claiming {
            builder = builder.header("authorization", authorization(signing_name));
        }
        let parts = builder
            .body(bytes::Bytes::new())
            .expect("a well-formed request")
            .into_parts()
            .0;
        let claimants: Vec<&str> = [
            (OPENSEARCH, opensearch.can_handle(&parts)),
            (ELASTICSEARCH, elasticsearch.can_handle(&parts)),
        ]
        .into_iter()
        .filter_map(|(key, claimed)| claimed.then_some(key))
        .collect();
        assert_eq!(
            claimants,
            expected.to_vec(),
            "case {case:?} ({path} to host {host}, {claiming:?}): expected it to be claimed \
             by exactly {expected:?}, got {claimants:?}"
        );
    }
}

/// One dispatched call and what the ONE model that owns its path must answer.
struct Call {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<Value>,
    expected_status: u16,
    /// The `x-amzn-errortype` the answer must carry, when the row pins one.
    expected_error: Option<&'static str>,
    /// Fragments the rendered answer must carry — each one a member or a word only the
    /// owning model's answer can produce.
    expected_contains: &'static [&'static str],
    /// Fragments the answer must NOT carry, for a row that pins which model's STATE
    /// answered rather than only which model's shapes.
    expected_absent: &'static [&'static str],
}

#[test]
fn the_es_endpoint_serves_both_api_versions() {
    std::thread::Builder::new()
        .name("es-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);

            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            // The rows span both API versions behind the one `es` key, and within each
            // version every kind of answer the owning model can give: a success out of its
            // own shapes, its own modelled refusal, and its scaffold `501 NotImplemented`
            // naming the operation it resolved. Each `expected_contains` fragment is
            // something ONLY the owning model can produce — `EngineVersion` and
            // `ClusterConfig` are the modern model's members, `ElasticsearchVersion` and
            // `ElasticsearchClusterConfig` the legacy model's, a refusal message names the
            // resource the owning model resolved the request against, and a 501 message
            // names the operation, which only the model declaring it can resolve.
            let calls: [Call; 11] = [
                Call {
                    case: "CreateDomain at the modern version",
                    method: "POST",
                    path: "/2021-01-01/opensearch/domain",
                    body: Some(json!({
                        "DomainName": "router-os-domain",
                        "EngineVersion": "OpenSearch_2.11",
                        "ClusterConfig": { "InstanceType": "r6g.large.search", "InstanceCount": 2 },
                    })),
                    expected_status: 200,
                    expected_error: None,
                    expected_contains: &[
                        "\"DomainName\":\"router-os-domain\"",
                        "\"EngineVersion\":\"OpenSearch_2.11\"",
                        ":es:us-east-1:",
                        "domain/router-os-domain",
                    ],
                    expected_absent: &["ElasticsearchVersion"],
                },
                Call {
                    // …and the read back, which reports the record the create persisted.
                    case: "DescribeDomain reads the modern domain back",
                    method: "GET",
                    path: "/2021-01-01/opensearch/domain/router-os-domain",
                    body: None,
                    expected_status: 200,
                    expected_error: None,
                    expected_contains: &[
                        "\"DomainName\":\"router-os-domain\"",
                        "\"InstanceCount\":2",
                    ],
                    expected_absent: &["ElasticsearchClusterConfig"],
                },
                Call {
                    case: "CreateElasticsearchDomain at the legacy version",
                    method: "POST",
                    path: "/2015-01-01/es/domain",
                    body: Some(json!({
                        "DomainName": "router-es-domain",
                        "ElasticsearchVersion": "7.10",
                        "ElasticsearchClusterConfig": {
                            "InstanceType": "t2.small.elasticsearch",
                            "InstanceCount": 1
                        },
                    })),
                    expected_status: 200,
                    expected_error: None,
                    expected_contains: &[
                        "\"DomainName\":\"router-es-domain\"",
                        "\"ElasticsearchVersion\":\"7.10\"",
                        ":es:us-east-1:",
                        "domain/router-es-domain",
                    ],
                    expected_absent: &["\"EngineVersion\""],
                },
                Call {
                    case: "DescribeElasticsearchDomain reads the legacy domain back",
                    method: "GET",
                    path: "/2015-01-01/es/domain/router-es-domain",
                    body: None,
                    expected_status: 200,
                    expected_error: None,
                    expected_contains: &[
                        "\"DomainName\":\"router-es-domain\"",
                        "ElasticsearchClusterConfig",
                    ],
                    expected_absent: &["\"EngineVersion\""],
                },
                Call {
                    // `ListDomainNames` is declared by BOTH models at `/<version>/domain`,
                    // so the two rows below also pin that each version reports its OWN
                    // service's state rather than the other's.
                    case: "ListDomainNames at the modern version",
                    method: "GET",
                    path: "/2021-01-01/domain",
                    body: None,
                    expected_status: 200,
                    expected_error: None,
                    expected_contains: &["router-os-domain"],
                    expected_absent: &["router-es-domain"],
                },
                Call {
                    case: "ListDomainNames at the legacy version",
                    method: "GET",
                    path: "/2015-01-01/domain",
                    body: None,
                    expected_status: 200,
                    expected_error: None,
                    expected_contains: &["router-es-domain"],
                    expected_absent: &["router-os-domain"],
                },
                Call {
                    // The issue's own request. `CreateApplication` is declared only by
                    // the modern model, so the honest answer is that model's own — the
                    // application it minted, named and ARN'd under this service — and
                    // not the router's `500 AmbiguousService`, which is what made the
                    // whole surface unreachable. Asserting the minted record rather
                    // than a bare status is what keeps this row about WHICH model
                    // served it.
                    case: "CreateApplication reaches the modern model",
                    method: "POST",
                    path: "/2021-01-01/opensearch/application",
                    body: Some(json!({
                        "name": "sim2real-app",
                        "dataSources": [{
                            "dataSourceArn":
                                "arn:aws:aoss:us-east-1:000000000000:collection/abc123xyz"
                        }],
                    })),
                    expected_status: 200,
                    expected_error: None,
                    expected_contains: &["sim2real-app", ":application/"],
                    expected_absent: &[],
                },
                Call {
                    // Only the modern model declares `StartMigration`, and it is
                    // implemented, so the witness that this model served the request is
                    // its OWN refusal rather than a scaffold's 501: the `applicationId`
                    // below names no application of this account — the one the row above
                    // minted carries a service-minted id — so the modern model answers
                    // the `ResourceNotFoundException` it declares for it, which the
                    // legacy model, declaring no such operation, could not produce.
                    case: "StartMigration reaches the modern model",
                    method: "POST",
                    path: "/2021-01-01/opensearch/app-migrations",
                    body: Some(json!({
                        "applicationId": "abc123xyz",
                        "migrationOptions": {
                            "source": {
                                "datasourceArn":
                                    "arn:aws:aoss:us-east-1:000000000000:collection/abc123xyz"
                            },
                            "workspace": { "createWorkspace": true, "name": "router-workspace" },
                        },
                    })),
                    // The shape's `@httpError` is 409 and this operation carries it
                    // under the status the service was recorded answering its own miss
                    // with.
                    expected_status: 404,
                    expected_error: Some("ResourceNotFoundException"),
                    // The refusal states that no such application exists and names the
                    // id it could not resolve; only the modern model declares the
                    // operation at all.
                    expected_contains: &["Application abc123xyz does not exist"],
                    expected_absent: &[],
                },
                Call {
                    // As for `StartMigration` above: the implemented modern-model
                    // listing answers its own refusal for an `applicationId` naming no
                    // application of this account — the missing-resource shape every
                    // call addressed by an application id states that miss with, which
                    // the legacy model, declaring no such operation, could not produce.
                    case: "ListMigrations reaches the modern model",
                    method: "GET",
                    path: "/2021-01-01/opensearch/app-migrations?applicationId=abc123xyz",
                    body: None,
                    expected_status: 404,
                    expected_error: Some("ResourceNotFoundException"),
                    expected_contains: &["Application abc123xyz not found"],
                    expected_absent: &[],
                },
                Call {
                    // As for `StartMigration` above: the implemented modern-model read
                    // answers its own not-found refusal for an id addressing no job. A
                    // migration's id names the application it belongs to, and the
                    // segment of this one names no application of this account, so the
                    // refusal states that — a sentence only the modern model can
                    // produce.
                    case: "GetMigration reaches the modern model",
                    method: "GET",
                    path: "/2021-01-01/opensearch/app-migrations/router-migration-id",
                    body: None,
                    // The status this service's recorded wire answer carries that
                    // refusal on, which the modern model's own `@httpError(409)` is not
                    // (`service_com_amazonaws_opensearch_amazonopensearchservice::
                    // scaffolded::RESOURCE_NOT_FOUND_STATUS`).
                    expected_status: 404,
                    expected_error: Some("ResourceNotFoundException"),
                    expected_contains: &["Application router not found"],
                    expected_absent: &[],
                },
                Call {
                    // A legacy-model-only operation, still scaffolded: its 501 names
                    // `ListElasticsearchVersions`, which only the legacy model declares.
                    case: "ListElasticsearchVersions reaches the legacy model",
                    method: "GET",
                    path: "/2015-01-01/es/versions",
                    body: None,
                    expected_status: 501,
                    expected_error: Some("NotImplemented"),
                    expected_contains: &["ListElasticsearchVersions"],
                    expected_absent: &[],
                },
            ];

            for call in calls {
                let mut builder = http::Request::builder()
                    .method(call.method)
                    .uri(format!("https://es.{REGION}.amazonaws.com{}", call.path))
                    .header("host", format!("es.{REGION}.amazonaws.com"))
                    .header("authorization", authorization(SHARED_KEY));
                let payload = match &call.body {
                    Some(body) => {
                        builder = builder.header("content-type", "application/json");
                        bytes::Bytes::from(body.to_string())
                    }
                    None => bytes::Bytes::new(),
                };
                let request = builder.body(payload).expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let error = response
                    .headers()
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                let case = call.case;

                // Neither routing failure may happen: `AmbiguousService` (500, several
                // claimants) nor `UnknownService` (404, none).
                assert!(
                    !rendered.contains("AmbiguousService") && !rendered.contains("UnknownService"),
                    "case {case:?}: the request must reach exactly one service, got {status} \
                     with {rendered}"
                );
                assert_eq!(
                    status, call.expected_status,
                    "case {case:?}: the owning model must answer HTTP {}, got {status} with \
                     {rendered}",
                    call.expected_status
                );
                if let Some(expected) = call.expected_error {
                    assert_eq!(
                        error.as_deref(),
                        Some(expected),
                        "case {case:?}: expected the error type {expected}, got {status} with \
                         {rendered}"
                    );
                }
                for expected in call.expected_contains {
                    assert!(
                        rendered.contains(expected),
                        "case {case:?}: expected {expected:?} in the owning model's answer, \
                         got {status} with {rendered}"
                    );
                }
                for absent in call.expected_absent {
                    assert!(
                        !rendered.contains(absent),
                        "case {case:?}: {absent:?} belongs to the OTHER model's answer and \
                         must be absent, got {status} with {rendered}"
                    );
                }
            }
        })
        .expect("spawn the routing thread")
        .join()
        .expect("OpenSearch Service and Elasticsearch Service must route apart");
}
