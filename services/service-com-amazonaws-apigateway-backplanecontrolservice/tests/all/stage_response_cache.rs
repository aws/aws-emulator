// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the STAGE RESPONSE CACHE of the `execute-api` data plane: what a
//! deployed stage answers for a method whose settings enable caching.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service, on the api's own
//! `{rest-api-id}.execute-api.{region}.amazonaws.com` host — so what is asserted is what a
//! client of the published invoke URL sees. The control plane is driven through the same
//! emulator ([`Gateway`]) to build the records the data plane then resolves, so nothing here
//! reaches into state.
//!
//! The rules under test, each over a TABLE of inputs rather than one recorded call:
//!
//! * a method whose settings enable caching on a stage whose cluster is up is answered from
//!   that cluster — the response a previous request was served, served again — until the
//!   entry stops being one the cluster holds, over a table of what ends it: the
//!   `cacheTtlInSeconds` passing, a `FlushStageCache`, and a cache-control ask the stage
//!   honours;
//! * a method the settings do NOT cache is answered by its integration every time, over a
//!   table of the reasons they do not: no settings at all, caching turned off, a TTL of
//!   zero, a stage with no cache cluster, and settings that name a different method;
//! * a `Cache-Control: max-age=0` ask that needs the `execute-api:InvalidateCache` grant and
//!   has none is answered as the stage's `unauthorizedCacheControlHeaderStrategy` says, over
//!   a table spanning each of the three modelled strategies, a stage that requires no
//!   authorization at all, an api whose resource policy grants the action, and each half of
//!   the refusal the failing strategy takes — the missing-token refusal for a caller that
//!   presented no credential, and the IAM-shaped one for a caller that presented one;
//! * the integration's `cacheNamespace` decides which resources share one entry, over a
//!   table of two resources grouped under one namespace and two carrying the per-resource
//!   default;
//! * the `methodSettings` key that enables caching is honoured in each spelling that names
//!   the invoked method — its own path escaped, its own path with a literal separator, and
//!   the whole-stage wildcard.
//!
//! What makes a cache hit OBSERVABLE: every case serves a `MOCK` method whose response
//! template is then REWRITTEN through `UpdateIntegrationResponse`. A request answered from
//! the records carries the rewritten body; one answered from the cluster carries the body
//! the first request was served. So the two are told apart by what the client reads, not by
//! anything the test knows about the store.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    CACHE_CLUSTER_AVAILABLE, CACHE_CLUSTER_CREATE_SPAN, CACHE_CLUSTER_FLUSH_SPAN,
};

use crate::execute_api_data_plane::{
    deploy, integration_response, invoke, json_template, method_response, mock_resource,
    put_integration, put_method,
};
use crate::flush_stage_cache::{cluster_status, flush_cache};
use crate::rest_api_lifecycle::{create_api, resource_under, string_member, Answer, Gateway};
use crate::stage_lifecycle::get_stage;

/// A margin on top of a stored span, so a test waiting one out is not racing it.
const MARGIN: Duration = Duration::from_millis(150);

/// The stage every case deploys to.
const STAGE: &str = "s";

/// The resource the cached method sits on.
const RESOURCE: &str = "items";

/// The region the control plane is called in, which is the Region the api's own ARNs name —
/// the one a refusal of an unauthorized cache invalidation reports.
const REGION: &str = "us-east-1";

/// The account every emulator-minted ARN names.
const ACCOUNT: &str = "000000000000";

/// The body the method's integration answers while the cluster is being filled.
const FIRST_BODY: &str = "{\"serial\":\"first\"}";

/// The body it answers once the response template has been rewritten: what a request
/// answered from the RECORDS reads, and what a request answered from the cluster must not.
const SECOND_BODY: &str = "{\"serial\":\"second\"}";

/// The header a stage states an ignored cache-control header in.
const WARNING_HEADER: &str = "warning";

/// The invalidation ask: the header API Gateway documents for reloading a cached entry.
const INVALIDATION_ASK: (&str, &str) = ("cache-control", "max-age=0");

/// A SigV4 credential scope naming this endpoint, as a signed request carries one: what
/// tells a caller that PRESENTED a credential from one that presented none. The signature
/// itself is not verified here (the harness runs with request authentication off), so only
/// its presence is what a row varies.
const SIGNED_CREDENTIAL: &str =
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/execute-api/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// One deployed api carrying a cached `GET /items`, with the stage's cache cluster settled.
struct Deployed {
    /// The api id.
    api: String,
    /// The api's root resource, for standing a second resource up under it.
    root: String,
    /// The resource the method sits on, for rewriting its integration response.
    resource: String,
}

impl Deployed {
    /// Stand up `GET /items` on a `MOCK` integration answering [`FIRST_BODY`], deployed to
    /// [`STAGE`] with `cluster` deciding whether the stage has a cache cluster at all.
    ///
    /// A stage that asked for one is waited out until it reports `AVAILABLE`, because "a
    /// cache cluster must be enabled on the stage for responses to be cached": a cluster
    /// that is still coming up caches nothing, which is a case of its own below.
    async fn stand_up(gateway: &Gateway, name: &str, cluster: bool) -> Self {
        let (api, root) = create_api(gateway, name).await;
        let resource = mock_resource(
            gateway,
            &api,
            &root,
            RESOURCE,
            "GET",
            "200",
            json_template(FIRST_BODY),
        )
        .await;
        let mut request = json!({ "stageName": STAGE });
        if cluster {
            request["cacheClusterEnabled"] = json!(true);
            request["cacheClusterSize"] = json!("0.5");
        }
        let deployed = gateway
            .call("POST", &format!("/restapis/{api}/deployments"), Some(request))
            .await;
        assert_eq!(
            deployed.status, 201,
            "case CreateDeployment({name}): expected 201, got {} with {:?}",
            deployed.status, deployed.body
        );
        if cluster {
            tokio::time::sleep(CACHE_CLUSTER_CREATE_SPAN + MARGIN).await;
            let stage = get_stage(gateway, &api, STAGE).await;
            assert_eq!(
                cluster_status(&stage.body, name),
                CACHE_CLUSTER_AVAILABLE,
                "case {name}: expected the stage's cache cluster to be up before any invoke"
            );
        }
        Self {
            api,
            root,
            resource,
        }
    }

    /// Rewrite the integration response's `application/json` template to `body` AND REDEPLOY,
    /// so a request answered from the records reads it and one answered from the cluster does
    /// not.
    ///
    /// The redeploy is what makes the edit the stage's: a stage serves the deployment its
    /// `deploymentId` names, so an edit to a method takes effect at the stage only once the
    /// api is deployed again (`crate::execute_api_data_plane`'s own snapshot rows pin that).
    /// It does not empty the cluster — `FlushStageCache` is the call that does — so what the
    /// stage HOLDS is what the rows below are about.
    async fn rewrite_response(&self, gateway: &Gateway, case: &str, body: &str) {
        let patched = gateway
            .call(
                "PATCH",
                &format!(
                    "/restapis/{api}/resources/{resource}/methods/GET/integration/responses/200",
                    api = self.api,
                    resource = self.resource
                ),
                Some(json!({
                    "patchOperations": [{
                        "op": "replace",
                        "path": "/responseTemplates/application~1json",
                        "value": body
                    }]
                })),
            )
            .await;
        assert_eq!(
            patched.status, 200,
            "case {case}: expected 200 from UpdateIntegrationResponse, got {} with {:?}",
            patched.status, patched.body
        );
        deploy(gateway, &self.api, STAGE).await;
    }

    /// `UpdateStage` with `operations`, asserting the modelled 200.
    async fn update_stage(&self, gateway: &Gateway, case: &str, operations: Value) -> Answer {
        let updated = gateway
            .call(
                "PATCH",
                &format!("/restapis/{api}/stages/{STAGE}", api = self.api),
                Some(json!({ "patchOperations": operations })),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected 200 from UpdateStage, got {} with {:?}",
            updated.status, updated.body
        );
        updated
    }

    /// `UpdateRestApi` attaching `document` as the api's resource policy, then redeploying
    /// so the stage serves the api as it now stands.
    async fn set_policy(&self, gateway: &Gateway, case: &str, document: &Value) {
        let patched = gateway
            .call(
                "PATCH",
                &format!("/restapis/{api}", api = self.api),
                Some(json!({
                    "patchOperations": [{
                        "op": "replace",
                        "path": "/policy",
                        "value": document.to_string()
                    }]
                })),
            )
            .await;
        assert_eq!(
            patched.status, 200,
            "case {case}: expected 200 from UpdateRestApi, got {} with {:?}",
            patched.status, patched.body
        );
        assert!(
            !string_member(&patched.body, "policy", case).is_empty(),
            "case {case}: expected the api to report the policy that was attached"
        );
    }

    /// The method ARN of the deployed `GET /items`, as a refusal of an unauthorized cache
    /// invalidation reports it.
    fn method_arn(&self) -> String {
        format!(
            "arn:aws:execute-api:{REGION}:{ACCOUNT}:{api}/{STAGE}/GET/{RESOURCE}",
            api = self.api
        )
    }

    /// Invoke `GET /items` on the stage, carrying `headers`.
    async fn invoke(
        &self,
        gateway: &Gateway,
        headers: &[(&str, &str)],
    ) -> crate::execute_api_data_plane::Invoked {
        invoke(
            gateway,
            &self.api,
            "GET",
            &format!("/{STAGE}/{RESOURCE}"),
            headers,
        )
        .await
    }

    /// One more `GET` resource under the root, on a `MOCK` integration answering `body` and
    /// carrying `namespace` as its `cacheNamespace`; deployed, so the stage serves it.
    async fn second_resource(&self, gateway: &Gateway, body: &str, namespace: Option<&str>) {
        let resource = resource_under(gateway, &self.api, &self.root, OTHER_RESOURCE).await;
        put_method(
            gateway,
            &self.api,
            &resource,
            "GET",
            json!({ "authorizationType": "NONE" }),
        )
        .await;
        let mut integration = json!({ "type": "MOCK" });
        if let Some(namespace) = namespace {
            integration["cacheNamespace"] = json!(namespace);
        }
        put_integration(gateway, &self.api, &resource, "GET", integration).await;
        method_response(gateway, &self.api, &resource, "GET", "200", json!({})).await;
        integration_response(
            gateway,
            &self.api,
            &resource,
            "GET",
            "200",
            json!({ "responseTemplates": json_template(body) }),
        )
        .await;
        deploy(gateway, &self.api, STAGE).await;
    }

    /// `UpdateIntegration` setting the FIRST resource's `cacheNamespace`, so both resources
    /// name the group the row put them in.
    async fn set_namespace(&self, gateway: &Gateway, case: &str, namespace: &str) {
        let patched = gateway
            .call(
                "PATCH",
                &format!(
                    "/restapis/{api}/resources/{resource}/methods/GET/integration",
                    api = self.api,
                    resource = self.resource
                ),
                Some(json!({
                    "patchOperations": [{
                        "op": "replace",
                        "path": "/cacheNamespace",
                        "value": namespace
                    }]
                })),
            )
            .await;
        assert_eq!(
            patched.status, 200,
            "case {case}: expected 200 from UpdateIntegration, got {} with {:?}",
            patched.status, patched.body
        );
        // Deployed, for the reason [`Self::rewrite_response`] gives: the group an edit put
        // the resource in is the stage's once the api is deployed again.
        deploy(gateway, &self.api, STAGE).await;
    }
}

/// The ops that turn caching on for the method path `method_path` with `ttl` seconds to
/// live.
fn caching_on_at(method_path: &str, ttl: i32) -> Vec<Value> {
    vec![
        json!({
            "op": "replace",
            "path": format!("{method_path}/caching/enabled"),
            "value": "true"
        }),
        json!({
            "op": "replace",
            "path": format!("{method_path}/caching/ttlInSeconds"),
            "value": ttl.to_string()
        }),
    ]
}

/// The ops that turn caching on for every method of the stage; see [`caching_on_at`].
fn caching_on(ttl: i32) -> Vec<Value> {
    caching_on_at("/*/*", ttl)
}

/// One op setting `/*/*/caching/requireAuthorizationForCacheControl`.
fn require_authorization(required: bool) -> Value {
    json!({
        "op": "replace",
        "path": "/*/*/caching/requireAuthorizationForCacheControl",
        "value": required.to_string()
    })
}

/// One op setting `/*/*/caching/unauthorizedCacheControlHeaderStrategy`.
fn strategy(value: &str) -> Value {
    json!({
        "op": "replace",
        "path": "/*/*/caching/unauthorizedCacheControlHeaderStrategy",
        "value": value
    })
}

/// A resource policy document carrying one statement.
fn policy_allowing(action: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": "*",
            "Action": action,
            "Resource": "execute-api:/*"
        }]
    })
}

/// What ends the entry the cluster is holding, which is what the rows of the first table
/// differ along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ends {
    /// Nothing: the entry is still held, so the cached body is served again.
    Nothing,
    /// The entry's own TTL passes.
    Ttl,
    /// `FlushStageCache` empties the cluster.
    Flush,
    /// The request asks for it to be reloaded, on a stage that requires no authorization
    /// for the ask.
    Ask,
}

#[tokio::test]
async fn the_methods_own_settings_key_caches_it_as_the_whole_stage_key_does() {
    // Rows differ along the `methodSettings` key the caching is turned on under: the
    // method's own path, in the escaped spelling `PatchOperation$path` documents, and the
    // whole-stage wildcard. Both name the invoked method, so both cache it — which is what
    // tells a wrong key construction from the wildcard fallback answering for it. A third
    // spelling, the resource path written with a literal separator (`/items/GET/…`), names
    // no method at all and is refused by `UpdateStage`, so it stores no key for this lookup
    // to resolve; `patch_entry_precedence` pins that refusal.
    let rows: &[(&str, &str)] = &[
        ("the method's own key, escaped", "/~1items/GET"),
        ("the whole-stage key", "/*/*"),
    ];

    for (index, (case, method_path)) in rows.iter().enumerate() {
        let gateway = Gateway::new();
        let deployed = Deployed::stand_up(&gateway, &format!("settings-key-{index}"), true).await;
        deployed
            .update_stage(
                &gateway,
                case,
                Value::Array(caching_on_at(method_path, 300)),
            )
            .await;

        let filled = deployed.invoke(&gateway, &[]).await;
        assert_eq!(
            filled.body, FIRST_BODY,
            "case {case}: expected the integration's own body first, got {:?}",
            filled.body
        );
        deployed
            .rewrite_response(&gateway, case, SECOND_BODY)
            .await;
        let cached = deployed.invoke(&gateway, &[]).await;
        assert_eq!(
            cached.body, FIRST_BODY,
            "case {case}: expected the entry these settings had the stage hold, got {:?}",
            cached.body
        );
    }
}

#[tokio::test]
async fn a_cached_method_is_answered_from_the_cluster_until_the_entry_it_holds_is_gone() {
    // Rows differ along what ENDS the held entry — the TTL passing, a flush, an honoured
    // ask — plus the row where nothing does, which is what makes the other three mean
    // anything: in all four the integration's own answer has been rewritten, so the body
    // says which side answered.
    let rows: &[(&str, Ends, &str)] = &[
        ("nothing ends the entry", Ends::Nothing, FIRST_BODY),
        ("the entry's TTL passes", Ends::Ttl, SECOND_BODY),
        ("a flush empties the cluster", Ends::Flush, SECOND_BODY),
        ("the caller asks for a reload", Ends::Ask, SECOND_BODY),
    ];

    for (case, ends, expected) in rows {
        let gateway = Gateway::new();
        let deployed = Deployed::stand_up(&gateway, &format!("cache-{ends:?}"), true).await;
        // A one-second TTL is the shortest a method setting can carry (the member is in
        // whole seconds), so the row that waits one out waits the least it can.
        let ttl = if *ends == Ends::Ttl { 1 } else { 300 };
        let mut operations = caching_on(ttl);
        operations.push(require_authorization(*ends != Ends::Ask));
        deployed
            .update_stage(&gateway, case, Value::Array(operations))
            .await;

        // The first request is a MISS: the integration answers it, and what it answered is
        // what the cluster now holds.
        let first = deployed.invoke(&gateway, &[]).await;
        assert_eq!(
            first.status, 200,
            "case {case}: expected 200 from the first invoke, got {} with {:?}",
            first.status, first.body
        );
        assert_eq!(
            first.body, FIRST_BODY,
            "case {case}: expected the integration's own body on a cache miss, got {:?}",
            first.body
        );

        // From here the records answer differently, so a cached response is visible.
        deployed
            .rewrite_response(&gateway, case, SECOND_BODY)
            .await;
        let cached = deployed.invoke(&gateway, &[]).await;
        assert_eq!(
            cached.body, FIRST_BODY,
            "case {case}: expected the held entry to be served while it is held, got {:?}",
            cached.body
        );

        match ends {
            Ends::Nothing => {}
            Ends::Ttl => tokio::time::sleep(Duration::from_secs(1) + MARGIN).await,
            Ends::Flush => {
                let flushed = flush_cache(&gateway, &deployed.api, STAGE).await;
                assert_eq!(
                    flushed.status, 202,
                    "case {case}: expected 202 from FlushStageCache, got {} with {:?}",
                    flushed.status, flushed.body
                );
                // A cluster that is being emptied is not one that is up, so it caches
                // nothing while the flush is outstanding: the row waits it out so that
                // what the next request reads is the EMPTIED cluster rather than one that
                // is mid-flush.
                tokio::time::sleep(CACHE_CLUSTER_FLUSH_SPAN + MARGIN).await;
            }
            Ends::Ask => {}
        }
        let headers: &[(&str, &str)] = if *ends == Ends::Ask {
            &[INVALIDATION_ASK]
        } else {
            &[]
        };
        let after = deployed.invoke(&gateway, headers).await;
        assert_eq!(
            after.status, 200,
            "case {case}: expected 200 after the entry ended, got {} with {:?}",
            after.status, after.body
        );
        assert_eq!(
            &after.body, expected,
            "case {case}: expected {expected} after the entry ended, got {:?}",
            after.body
        );
        assert!(
            after.header(WARNING_HEADER).is_none(),
            "case {case}: nothing was ignored, so no warning is stated, got {:?}",
            after.header(WARNING_HEADER)
        );

        // And the response that followed is cached in its turn: the entry a request filled
        // is the entry the next request reads, whichever request filled it.
        deployed.rewrite_response(&gateway, case, FIRST_BODY).await;
        let again = deployed.invoke(&gateway, &[]).await;
        assert_eq!(
            &again.body, expected,
            "case {case}: expected the entry the last request filled, got {:?}",
            again.body
        );
    }
}

#[tokio::test]
async fn a_method_the_settings_do_not_cache_is_answered_by_its_integration_every_time() {
    // Rows differ along the reason the stage caches nothing for this method: the settings
    // are absent, they turn caching off, they allow no time to live, the stage has no
    // cluster for them to use, and they name a different method than the one invoked.
    let rows: &[(&str, bool, Vec<Value>)] = &[
        ("no method settings at all", true, Vec::new()),
        (
            "caching turned off",
            true,
            vec![json!({ "op": "replace", "path": "/*/*/caching/enabled", "value": "false" })],
        ),
        ("a time to live of zero", true, caching_on(0)),
        ("a stage with no cache cluster", false, caching_on(300)),
        (
            "settings naming another method",
            true,
            vec![
                json!({ "op": "replace", "path": "/~1other/GET/caching/enabled", "value": "true" }),
                json!({
                    "op": "replace",
                    "path": "/~1other/GET/caching/ttlInSeconds",
                    "value": "300"
                }),
            ],
        ),
    ];

    for (index, (case, cluster, operations)) in rows.iter().enumerate() {
        let gateway = Gateway::new();
        let deployed =
            Deployed::stand_up(&gateway, &format!("uncached-{index}"), *cluster).await;
        if !operations.is_empty() {
            deployed
                .update_stage(&gateway, case, Value::Array(operations.clone()))
                .await;
        }
        let first = deployed.invoke(&gateway, &[]).await;
        assert_eq!(
            first.body, FIRST_BODY,
            "case {case}: expected the integration's own body, got {:?}",
            first.body
        );
        deployed
            .rewrite_response(&gateway, case, SECOND_BODY)
            .await;
        let second = deployed.invoke(&gateway, &[]).await;
        assert_eq!(
            second.body, SECOND_BODY,
            "case {case}: expected the records to answer again, got {:?}",
            second.body
        );
    }
}

/// The body the second resource's own integration answers, which a shared namespace must
/// hide and a default namespace must not.
const OTHER_BODY: &str = "{\"serial\":\"other\"}";
const OTHER_RESOURCE: &str = "others";

#[tokio::test]
async fn the_cache_namespace_decides_which_resources_share_an_entry() {
    // Rows differ along the namespace the two resources' integrations carry: one the
    // developer grouped them under, and the per-resource default `PutIntegration` writes.
    // The grouped pair shares the entry the first request filled — "you can specify the
    // same cacheNamespace across resources to return the same cached data for requests to
    // different resources" — and the ungrouped pair does not.
    let rows: &[(&str, Option<&str>, &str)] = &[
        ("one namespace across both resources", Some("grouped"), FIRST_BODY),
        ("each resource's own default namespace", None, OTHER_BODY),
    ];

    for (index, (case, namespace, expected)) in rows.iter().enumerate() {
        let gateway = Gateway::new();
        let deployed = Deployed::stand_up(&gateway, &format!("namespace-{index}"), true).await;
        // The second resource answers a body of its own, so what the client reads says
        // which resource's integration produced it — or that neither did, because the
        // cluster answered.
        deployed
            .second_resource(&gateway, OTHER_BODY, *namespace)
            .await;
        if let Some(namespace) = namespace {
            // The FIRST resource has to be grouped too: a namespace is shared, so both
            // integrations have to name it.
            deployed.set_namespace(&gateway, case, namespace).await;
        }
        deployed
            .update_stage(&gateway, case, Value::Array(caching_on(300)))
            .await;

        let filled = deployed.invoke(&gateway, &[]).await;
        assert_eq!(
            filled.body, FIRST_BODY,
            "case {case}: expected the first resource's own body, got {:?}",
            filled.body
        );
        let other = invoke(
            &gateway,
            &deployed.api,
            "GET",
            &format!("/{STAGE}/{OTHER_RESOURCE}"),
            &[],
        )
        .await;
        assert_eq!(
            other.status, 200,
            "case {case}: expected 200 from the second resource, got {} with {:?}",
            other.status, other.body
        );
        assert_eq!(
            &other.body, expected,
            "case {case}: expected {expected} from the second resource, got {:?}",
            other.body
        );
    }
}

#[derive(Debug, Clone, Copy)]
struct Answered {
    /// The status the ask is answered with.
    status: u16,
    /// The body it is answered with, or `None` when the body is the refusal rather than one
    /// of the two integration bodies — which refusal is decided by whether the caller
    /// presented a credential at all ([`REFUSED_UNSIGNED_MESSAGE`]).
    body: Option<&'static str>,
    /// Whether the response carries the warning header.
    warned: bool,
}

/// The sentence an ask from a caller that presented NO credential is refused with: nothing
/// authenticated, so the endpoint answers the missing-token refusal rather than a decision
/// about a principal.
const REFUSED_UNSIGNED_MESSAGE: &str = "Missing Authentication Token";

/// What the refusal of an ask from a caller that DID present a credential names: the action
/// the api's policy decided against, in the sentence AWS refuses an unauthorized call with.
const REFUSED_SIGNED_ACTION: &str = "is not authorized to perform: execute-api:InvalidateCache";

#[tokio::test]
async fn an_unauthorized_cache_control_ask_is_answered_as_the_stages_strategy_says() {
    // Rows span every modelled `unauthorizedCacheControlHeaderStrategy` value, plus the two
    // ways an ask is AUTHORIZED at all: a stage that requires no authorization for it, and
    // an api whose resource policy grants the action. Two rows are the control for that
    // grant: a policy granting only `execute-api:Invoke` authorizes no invalidation, for a
    // caller that presented a credential and for one that presented none — the two halves
    // of the refusal the failing strategy takes.
    type Row = (
        &'static str,
        bool,
        Option<&'static str>,
        Option<&'static str>,
        bool,
        Answered,
    );
    let rows: &[Row] = &[
        (
            "the strategy that fails the request",
            true,
            Some("FAIL_WITH_403"),
            None,
            false,
            Answered {
                status: 403,
                body: None,
                warned: false,
            },
        ),
        (
            "the strategy that states the header was ignored",
            true,
            Some("SUCCEED_WITH_RESPONSE_HEADER"),
            None,
            false,
            Answered {
                status: 200,
                body: Some(FIRST_BODY),
                warned: true,
            },
        ),
        (
            "the strategy that ignores it silently",
            true,
            Some("SUCCEED_WITHOUT_RESPONSE_HEADER"),
            None,
            false,
            Answered {
                status: 200,
                body: Some(FIRST_BODY),
                warned: false,
            },
        ),
        (
            "a stage requiring no authorization for the ask",
            false,
            Some("FAIL_WITH_403"),
            None,
            false,
            Answered {
                status: 200,
                body: Some(SECOND_BODY),
                warned: false,
            },
        ),
        (
            "a policy granting the invalidation",
            true,
            Some("FAIL_WITH_403"),
            Some("execute-api:InvalidateCache"),
            false,
            Answered {
                status: 200,
                body: Some(SECOND_BODY),
                warned: false,
            },
        ),
        (
            "a policy granting only the invoke",
            true,
            Some("FAIL_WITH_403"),
            Some("execute-api:Invoke"),
            false,
            Answered {
                status: 403,
                body: None,
                warned: false,
            },
        ),
        (
            "a signed caller a policy granting only the invoke does not authorize",
            true,
            Some("FAIL_WITH_403"),
            Some("execute-api:Invoke"),
            true,
            Answered {
                status: 403,
                body: None,
                warned: false,
            },
        ),
    ];

    for (index, (case, require, named_strategy, granted, signed, expected)) in
        rows.iter().enumerate()
    {
        let gateway = Gateway::new();
        let deployed = Deployed::stand_up(&gateway, &format!("ask-{index}"), true).await;
        if let Some(action) = granted {
            // A policy that grants an action must still admit the invoke itself, which an
            // api carrying a document only does where the document says so: the grant
            // under test is added to an allow of the invoke, except in the row that grants
            // the invoke alone.
            let document = if *action == "execute-api:Invoke" {
                policy_allowing(action)
            } else {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        policy_allowing("execute-api:Invoke")["Statement"][0].clone(),
                        policy_allowing(action)["Statement"][0].clone()
                    ]
                })
            };
            deployed.set_policy(&gateway, case, &document).await;
        }
        let mut operations = caching_on(300);
        operations.push(require_authorization(*require));
        if let Some(named) = named_strategy {
            operations.push(strategy(named));
        }
        deployed
            .update_stage(&gateway, case, Value::Array(operations))
            .await;

        // Fill the cluster, then rewrite the records, so the ask has something to reload
        // and the body says whether it was reloaded.
        let filled = deployed.invoke(&gateway, &[]).await;
        assert_eq!(
            filled.body, FIRST_BODY,
            "case {case}: expected the first invoke to be answered by the integration, got {:?}",
            filled.body
        );
        deployed
            .rewrite_response(&gateway, case, SECOND_BODY)
            .await;

        let ask: Vec<(&str, &str)> = match signed {
            true => vec![INVALIDATION_ASK, ("authorization", SIGNED_CREDENTIAL)],
            false => vec![INVALIDATION_ASK],
        };
        let answered = deployed.invoke(&gateway, &ask).await;
        assert_eq!(
            answered.status, expected.status,
            "case {case}: expected {} for the ask, got {} with {:?}",
            expected.status, answered.status, answered.body
        );
        match expected.body {
            Some(body) => assert_eq!(
                &answered.body, body,
                "case {case}: expected {body} for the ask, got {:?}",
                answered.body
            ),
            None if *signed => {
                // A caller that PRESENTED a credential names a principal the api's policy
                // decided against, so the refusal is the IAM-shaped sentence naming the
                // action it asked for and the method it asked for.
                assert!(
                    answered.body.contains(REFUSED_SIGNED_ACTION)
                        && answered.body.contains(&deployed.method_arn()),
                    "case {case}: expected the IAM-shaped refusal sentence naming \
                     {REFUSED_SIGNED_ACTION}, got {:?}",
                    answered.body
                );
            }
            None => {
                // A caller that presented NO credential authenticated nobody, so the
                // endpoint answers the refusal it takes for an authorization it cannot take
                // at all rather than a decision about a principal.
                let parsed: Value = serde_json::from_str(&answered.body)
                    .unwrap_or_else(|_| panic!("case {case}: expected a JSON body, got {:?}", answered.body));
                assert_eq!(
                    parsed["message"].as_str(),
                    Some(REFUSED_UNSIGNED_MESSAGE),
                    "case {case}: expected the missing-token refusal, got {:?}",
                    answered.body
                );
            }
        }
        assert_eq!(
            answered.header(WARNING_HEADER).is_some(),
            expected.warned,
            "case {case}: expected warned={}, got {:?}",
            expected.warned,
            answered.header(WARNING_HEADER)
        );
    }
}
