// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for this service's two resource slices.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 routing and serialization, the modelled-constraint walk and the
//! handlers — so what is asserted is what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! * a deployment parameter is ONE persisted record keyed on
//!   (catalog, productId, agreementId, deploymentParameter.name), so a repeat
//!   `PutDeploymentParameter` for that identity updates it in place and reports the
//!   same `deploymentParameterId`/`resourceArn`, and every reported field is read
//!   out of the record rather than echoed;
//! * `TagResource` merges onto, and `UntagResource` removes from, that record's
//!   tag map, and `ListTagsForResource` reports exactly what those writes left;
//! * an ARN of this service's vendor naming no stored record is a 404 carrying this
//!   service's own `NotFoundException` code, a value this API cannot address at all a
//!   400 `BadRequestException` (the `resource_arn_validation` suite owns that rule),
//!   and a request outside the modelled constraints a `ValidationException`;
//! * a deployment configuration is published by a JOB, and every operation of
//!   that slice reports what the job records hold — the second half of this file
//!   states those rules beside the cases that pin them.
//!
//! The harness below (the router, the signed request, and the per-operation
//! callers) is `pub(crate)` because the sibling suites in this target —
//! `put_deployment_parameter_target`, `validation_messages` and
//! `not_found_error_code` — drive the same wire path and reuse it rather than
//! restating it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    seed_agreements, service_state, ProvisionedAgreement, MAX_CLIENT_TOKENS,
    MAX_TAGS_PER_RESOURCE, TAG_KEY_MAX_LEN, TAG_VALUE_MAX_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder and the body accessors are shared by every suite of this
// test binary; ONE copy lives in `tests/all/support.rs` and is re-exported here, so
// the sibling suites that import them from this module keep working.
pub(crate) use crate::support::{error_code, field_name, message, pct, rfc3339};

/// The region every request below signs for; every minted ARN must carry it.
pub(crate) const REGION: &str = "us-west-2";
/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";
/// This service's ARN namespace and sigv4 signing name, from the model's
/// `aws.api#service$arnNamespace` / `aws.auth#sigv4$name`.
pub(crate) const SIGNING_NAME: &str = "aws-marketplace";
/// The one catalog this service serves, which every request below names.
///
/// `PutDeploymentParameterRequest$catalog` is documented as a fixed value
/// (`smithy_query(query="doc",shape="PutDeploymentParameterRequest",member="catalog")`),
/// and the service reads the set it belongs to from
/// `state::SUPPORTED_CATALOGS` — which this constant is asserted against in the
/// `catalog_allowed_values` suite, so the two cannot drift.
pub(crate) const CATALOG: &str = "AWSMarketplace";
/// The longest `DeploymentParameterName` the model admits (`length 1..400`).
pub(crate) const PARAMETER_NAME_MAX_LEN: usize = 400;
/// The longest `SecretString` the model admits (`length 1..15000`).
pub(crate) const SECRET_MAX_LEN: usize = 15_000;
/// The second parameter name every lifecycle row writes under its own identity,
/// to prove a different name mints a different resource.
const ALTERNATE_NAME: &str = "AlternateParameter";

/// One emulator with an empty store, mounted exactly as the router mounts it,
/// beside the engine that store lives in.
///
/// The engine is handed back so a test can [`provision`] the agreements a write
/// targets: `PutDeploymentParameter` writes against an agreement of the account,
/// and this API models no operation that creates one.
///
/// AWS Secrets Manager is mounted beside this service because a deployment
/// parameter's `secretString` is held in a secret of that service — the write
/// `PutDeploymentParameter` performs — so a router without it is a deployment in
/// which this service cannot store the value it is given at all. It is reached
/// over the router's own in-process callback, exactly as the production router
/// mounts both.
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::EmulatedService::new(engine.clone()),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(engine.clone()),
    );
    (router, engine)
}

/// Record `agreement_id` of `catalog` as covering `product_ids` — the out-of-band
/// provisioning a deployment-parameter write is targeted at.
pub(crate) fn provision(
    engine: &engine::InMemoryEngine,
    catalog: &str,
    agreement_id: &str,
    product_ids: &[&str],
) {
    let agreement = product_ids.iter().fold(
        ProvisionedAgreement::new(catalog, agreement_id),
        |agreement, product_id| agreement.with_product(*product_id),
    );
    assert!(
        seed_agreements(engine, [agreement]).is_some(),
        "the shared engine must hold this service's store"
    );
}

/// Whether this service's store holds the target `(catalog, product_id,
/// agreement_id)`: the agreement the account holds, covering that product.
pub(crate) fn holds_target(
    engine: &engine::InMemoryEngine,
    catalog: &str,
    product_id: &str,
    agreement_id: &str,
) -> bool {
    service_state(engine)
        .expect("the shared engine holds this service's store")
        .lock()
        .holds_target(catalog, product_id, agreement_id)
}

/// Whether this service's store holds `agreement_id` in `catalog` at all,
/// whatever products that agreement covers.
pub(crate) fn holds_agreement(
    engine: &engine::InMemoryEngine,
    catalog: &str,
    agreement_id: &str,
) -> bool {
    service_state(engine)
        .expect("the shared engine holds this service's store")
        .lock()
        .holds_agreement(catalog, agreement_id)
}

/// Whether this service's store holds `product_id` in `catalog` at all: whether
/// ANY agreement the account holds there covers that product.
pub(crate) fn holds_product(
    engine: &engine::InMemoryEngine,
    catalog: &str,
    product_id: &str,
) -> bool {
    service_state(engine)
        .expect("the shared engine holds this service's store")
        .lock()
        .holds_product(catalog, product_id)
}

/// How many deployment parameters the store holds.
pub(crate) fn stored_parameters(engine: &engine::InMemoryEngine) -> usize {
    service_state(engine)
        .expect("the shared engine holds this service's store")
        .lock()
        .parameters
        .len()
}

/// How many targets the store has recorded: one per product an agreement covers,
/// read through the store's own accessor.
pub(crate) fn recorded_targets(engine: &engine::InMemoryEngine) -> usize {
    service_state(engine)
        .expect("the shared engine holds this service's store")
        .lock()
        .recorded_targets()
}

/// A request signed for `aws-marketplace` in [`REGION`]: the signal the router
/// routes on and the handlers read the ARN's region from.
pub(crate) fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("deployment-marketplace.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDMPDEPLOYTEST/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

pub(crate) async fn put(
    router: &MultiServiceRouter,
    catalog: &str,
    product_id: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!(
            "/catalogs/{}/products/{}/deployment-parameters",
            pct(catalog),
            pct(product_id)
        ),
        Some(body),
    )
    .await
}

pub(crate) async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> (u16, Value) {
    call(router, "GET", &format!("/tags/{}", pct(resource_arn)), None).await
}

pub(crate) async fn tag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &BTreeMap<String, String>,
) -> (u16, Value) {
    let body = json!({
        "tags": tags
            .iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect::<serde_json::Map<String, Value>>(),
    });
    call(
        router,
        "POST",
        &format!("/tags/{}", pct(resource_arn)),
        Some(&body),
    )
    .await
}

pub(crate) async fn untag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tag_keys: &[String],
) -> (u16, Value) {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", pct(key)))
        .collect::<Vec<_>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", pct(resource_arn)),
        None,
    )
    .await
}

/// The `tags` member of a response, as a comparable map. An absent member reads
/// as the empty map.
pub(crate) fn tags_of(body: &Value) -> BTreeMap<String, String> {
    body.get("tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or("<not a string>").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A tag map from `(key, value)` pairs.
pub(crate) fn tag_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The ARN this emulator must mint for the deployment parameter `id`.
pub(crate) fn expected_arn(id: &str) -> String {
    format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/{id}")
}

/// Whether `id` satisfies the modelled `DeploymentParameterResourceIdentifier`
/// (`length 1..32`, `^dp-[a-zA-Z0-9]+$`).
pub(crate) fn is_parameter_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("dp-") else {
        return false;
    };
    !suffix.is_empty() && id.len() <= 32 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// One row of the lifecycle table: an identity, its content, and the tags its
/// create carries.
struct Lifecycle {
    case: &'static str,
    catalog: String,
    product_id: String,
    agreement_id: String,
    parameter: String,
    secret: String,
    put_tags: BTreeMap<String, String>,
    /// The instant `expirationDate` names, as an epoch second.
    ///
    /// The member is `@timestampFormat("date-time")`, so it reaches the wire as the
    /// RFC 3339 string `support::rfc3339` renders — the form an AWS SDK sends.
    expiration_date: Option<i64>,
}

/// The lifecycle table. The rows differ along everything the rule generalises
/// over: the form of each identity segment (the characters its own modelled
/// pattern admits — `ResourceId` is `^[A-Za-z0-9_/-]+$`,
/// `DeploymentParameterName` `^[a-zA-Z0-9/_+=.@-]+$`), the length of the secret at
/// both modelled bounds, the number of tags the create carries (none, one, a few,
/// and one short of the cap) and whether the create names an `expirationDate`.
///
/// Every row carries the SAME catalog: `catalog` is an allowed-value member (the
/// model documents it as a fixed value, `crate::catalog_allowed_values` pins the
/// rule), so a row varying it would be testing that rule and not this one.
fn lifecycle_table() -> Vec<Lifecycle> {
    // One short of the cap, so the single tag the TagResource step adds lands the
    // record exactly ON the modelled `TagsMap length max 50`.
    let nearly_full: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE - 1)
        .map(|i| (format!("key-{i:02}"), format!("value-{i:02}")))
        .collect();
    vec![
        Lifecycle {
            case: "plain identity, one tag",
            catalog: CATALOG.to_string(),
            product_id: "prod-abc123".to_string(),
            agreement_id: "agmt-abc123".to_string(),
            parameter: "DbPassword".to_string(),
            secret: "s3cr3t".to_string(),
            put_tags: tag_map(&[("env", "dev")]),
            expiration_date: None,
        },
        Lifecycle {
            case: "nested product id, punctuated name, no tags",
            catalog: CATALOG.to_string(),
            product_id: "prod/nested/id-7".to_string(),
            agreement_id: "agmt_1/2-3".to_string(),
            parameter: "api.key+v1=1@host/path_1-2".to_string(),
            secret: "x".to_string(),
            put_tags: BTreeMap::new(),
            expiration_date: Some(1_893_456_000),
        },
        Lifecycle {
            case: "secret at the modelled maximum, several punctuated tags",
            catalog: CATALOG.to_string(),
            product_id: "prod_01/beta-2".to_string(),
            agreement_id: "A".to_string(),
            parameter: "p".to_string(),
            secret: "y".repeat(SECRET_MAX_LEN),
            put_tags: tag_map(&[
                ("a", "1"),
                ("Z", "26"),
                ("mid.key:v", "value/with+chars=1"),
                ("at@sign", "ok_1"),
            ]),
            expiration_date: None,
        },
        Lifecycle {
            case: "name at the modelled maximum, one tag short of the cap",
            catalog: CATALOG.to_string(),
            product_id: "0".to_string(),
            agreement_id: "9".to_string(),
            parameter: "n".repeat(PARAMETER_NAME_MAX_LEN),
            secret: "rotate-me".to_string(),
            put_tags: nearly_full,
            expiration_date: None,
        },
        Lifecycle {
            case: "single-character identity",
            catalog: CATALOG.to_string(),
            product_id: "b".to_string(),
            agreement_id: "c".to_string(),
            parameter: "d".to_string(),
            secret: "e".to_string(),
            put_tags: tag_map(&[("k", "v")]),
            expiration_date: None,
        },
    ]
}

impl Lifecycle {
    /// The request body for this row's create, naming `parameter`/`secret`.
    fn body(&self, parameter: &str, secret: &str, tags: Option<&BTreeMap<String, String>>) -> Value {
        let mut body = json!({
            "agreementId": self.agreement_id,
            "deploymentParameter": { "name": parameter, "secretString": secret },
        });
        let map = body.as_object_mut().expect("an object body");
        if let Some(tags) = tags.filter(|tags| !tags.is_empty()) {
            map.insert(
                "tags".to_string(),
                Value::Object(tags.iter().map(|(k, v)| (k.clone(), json!(v))).collect()),
            );
        }
        if let Some(expiration) = self.expiration_date {
            map.insert("expirationDate".to_string(), json!(rfc3339(expiration)));
        }
        body
    }
}

#[tokio::test]
async fn a_deployment_parameter_is_one_record_that_put_tag_untag_and_list_all_answer_from() {
    let (router, engine) = emulator();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for row in lifecycle_table() {
        provision(&engine, &row.catalog, &row.agreement_id, &[&row.product_id]);
        let case = row.case;

        // --- create -------------------------------------------------------
        let (status, created) = put(
            &router,
            &row.catalog,
            &row.product_id,
            &row.body(&row.parameter, &row.secret, Some(&row.put_tags)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutDeploymentParameter must succeed, got {status} {created:?}"
        );
        let id = created
            .get("deploymentParameterId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_parameter_id(&id),
            "case {case:?}: expected a minted ^dp-[a-zA-Z0-9]+$ id of at most 32 characters, got {id:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: expected a freshly minted id, got the already-minted {id:?}"
        );
        let arn = created
            .get("resourceArn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            arn,
            expected_arn(&id),
            "case {case:?}: expected the ARN of the minted id, got {arn:?}"
        );
        assert_eq!(
            created.get("agreementId").and_then(Value::as_str),
            Some(row.agreement_id.as_str()),
            "case {case:?}: expected the stored agreementId, got {created:?}"
        );
        assert_eq!(
            tags_of(&created),
            row.put_tags,
            "case {case:?}: expected the create's tags back from the record, got {created:?}"
        );

        // --- the read that proves the create persisted --------------------
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must succeed, got {status} {listed:?}"
        );
        assert_eq!(
            tags_of(&listed),
            row.put_tags,
            "case {case:?}: expected the create-time tags, got {listed:?}"
        );

        // --- merge: one new key, and an overwrite of an existing one ------
        let mut added = tag_map(&[("team", "platform")]);
        if let Some(first) = row.put_tags.keys().next() {
            added.insert(first.clone(), "overwritten".to_string());
        }
        let (status, body) = tag(&router, &arn, &added).await;
        assert_eq!(
            status, 204,
            "case {case:?}: TagResource must answer 204, got {status} {body:?}"
        );
        assert!(
            body.as_object().is_none_or(serde_json::Map::is_empty),
            "case {case:?}: TagResource must answer no members, got {body:?}"
        );
        let mut expected = row.put_tags.clone();
        expected.extend(added.clone());
        assert!(
            expected.len() <= MAX_TAGS_PER_RESOURCE,
            "case {case:?}: the table row must stay inside the modelled cap, got {}",
            expected.len()
        );
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            tags_of(&listed),
            expected,
            "case {case:?}: expected the create-time and tag-time writes merged, got {listed:?}"
        );

        // --- removal, including a key that was never there ----------------
        let removals = vec!["team".to_string(), "never-set".to_string()];
        for attempt in 1..=2 {
            let (status, body) = untag(&router, &arn, &removals).await;
            assert_eq!(
                status, 204,
                "case {case:?}: UntagResource attempt {attempt} must answer 204 (@idempotent), \
                 got {status} {body:?}"
            );
        }
        expected.remove("team");
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            tags_of(&listed),
            expected,
            "case {case:?}: expected the removed key gone and the rest intact, got {listed:?}"
        );

        // --- a repeat Put for the SAME identity updates in place ----------
        let ignored = tag_map(&[("ignored-at-update", "yes")]);
        // A different secret of the same class: still inside `SecretString`'s
        // modelled `length 1..15000`, including for the row already at the maximum.
        let rotated = if row.secret.len() + "rotated-".len() > SECRET_MAX_LEN {
            "z".repeat(row.secret.len())
        } else {
            format!("rotated-{}", row.secret)
        };
        let (status, again) = put(
            &router,
            &row.catalog,
            &row.product_id,
            &row.body(&row.parameter, &rotated, Some(&ignored)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeat Put must succeed, got {status} {again:?}"
        );
        assert_eq!(
            again.get("deploymentParameterId").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case:?}: expected the same deploymentParameterId on update, got {again:?}"
        );
        assert_eq!(
            again.get("resourceArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the same resourceArn on update, got {again:?}"
        );
        assert_eq!(
            tags_of(&again),
            expected,
            "case {case:?}: tags are applied on create only, so an update must report the \
             record's tag map unchanged, got {again:?}"
        );
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            tags_of(&listed),
            expected,
            "case {case:?}: an update must not rewrite the tag map, got {listed:?}"
        );

        // --- a DIFFERENT name under the same catalog/product/agreement ----
        let (status, other) = put(
            &router,
            &row.catalog,
            &row.product_id,
            &row.body(ALTERNATE_NAME, "another-secret", None),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a second parameter name must succeed, got {status} {other:?}"
        );
        let other_id = other
            .get("deploymentParameterId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_parameter_id(&other_id) && other_id != id,
            "case {case:?}: a different parameter name must mint a different resource, \
             got {other_id:?} against {id:?}"
        );
        assert!(
            minted.insert(other_id.clone()),
            "case {case:?}: expected a freshly minted id, got {other_id:?}"
        );
        assert_eq!(
            tags_of(&other),
            BTreeMap::new(),
            "case {case:?}: a create carrying no tags must report an empty tag map, got {other:?}"
        );
        // The two records are independent: the first one's tags are untouched.
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            tags_of(&listed),
            expected,
            "case {case:?}: writing another parameter must not touch this record, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_that_names_no_record_is_not_found_on_every_tag_operation() {
    let (router, engine) = emulator();
    provision(&engine, "AWSMarketplace", "agmt-present", &["prod-present"]);

    // A record that DOES exist, so a 404 below is about the ARN and not about an
    // empty store.
    let (status, created) = put(
        &router,
        "AWSMarketplace",
        "prod-present",
        &json!({
            "agreementId": "agmt-present",
            "deploymentParameter": { "name": "Present", "secretString": "s" },
        }),
    )
    .await;
    assert_eq!(status, 200, "the setup Put must succeed, got {created:?}");
    let live = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();

    // `resourceArn` on the tagging operations targets a plain `String` with no
    // pattern, so each of these reaches the handler. Every row is an ARN of THIS
    // service's vendor, so what it observes is the store's answer about the ARN; a
    // value this API cannot address is refused one step earlier as a bad request,
    // which the `resource_arn_validation` suite owns.
    let unknown = vec![
        (
            "a well-formed ARN of an id that was never minted",
            expected_arn("dp-ffffffffffffffff"),
        ),
        (
            "the live ARN in another region",
            live.replace(REGION, "eu-west-1"),
        ),
        (
            "the live ARN in another account",
            live.replace(ACCOUNT, "000000000001"),
        ),
        (
            "an ARN of this vendor naming another resource type",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:secret/dp-0000000000000001"),
        ),
    ];

    for (case, resource_arn) in unknown {
        let (status, body) = list_tags(&router, &resource_arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: ListTagsForResource must be 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body:?}"
        );

        let (status, body) = tag(&router, &resource_arn, &tag_map(&[("k", "v")])).await;
        assert_eq!(
            status, 404,
            "case {case:?}: TagResource must be 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body:?}"
        );

        let (status, body) = untag(&router, &resource_arn, &["k".to_string()]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: UntagResource must be 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body:?}"
        );
    }

    // The live record is still readable, so none of the above wrote anything.
    let (status, listed) = list_tags(&router, &live).await;
    assert_eq!(
        status, 200,
        "the live record must still be readable, got {status} {listed:?}"
    );
    assert_eq!(
        tags_of(&listed),
        BTreeMap::new(),
        "a refused TagResource must leave the record's tags alone, got {listed:?}"
    );
}

#[tokio::test]
async fn a_put_outside_the_modelled_constraints_is_refused() {
    let (router, engine) = emulator();
    // Every row below is aimed at a target the account holds, so what each one
    // observes is the modelled constraint it breaks and not a missing agreement.
    // The tag-key and tag-value rows need this: the centralized walk does not
    // report map-key constraints, so those two are refused by the handler's own
    // tag check, which runs after the target is resolved.
    provision(&engine, "AWSMarketplace", "a1", &["prod-1"]);
    // The one legal row at the end of this test writes against this target: the
    // shortest product, agreement, name and secret the model admits, under the one
    // catalog it serves.
    provision(&engine, "AWSMarketplace", "a", &["p"]);

    // A `tags` map past the number of entries one resource holds is NOT a row here:
    // that count is the tagging front end's limit and carries its own wire identity,
    // which the `tag_count_front_end` suite pins.

    // Each row breaks exactly one modelled constraint of `PutDeploymentParameter`'s
    // input: `Catalog` (`^[a-zA-Z_-]+$`, len 1..64), `ResourceId`
    // (`^[A-Za-z0-9_/-]+$`, len 1..64), `DeploymentParameterName` (len 1..400),
    // `SecretString`'s MINIMUM (len 1..15000), `TagKey`/`TagValue` or `@required`.
    // `SecretString`'s MAXIMUM is deliberately absent: an oversized `@sensitive`
    // payload is refused one layer earlier, by the front door, with HTTP 403
    // `ForbiddenException` — the `front_door_forbidden` suite pins it. So is
    // `TagsMap`'s entry COUNT: that limit is the tagging front end's and carries its
    // own identity (`tag_count_front_end`).
    let over_long_catalog = "c".repeat(65);
    let over_long_product = "p".repeat(65);

    let refused: Vec<(&str, &str, &str, Value)> = vec![
        (
            "catalog carrying a space and a bang",
            "bad catalog 1!",
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "N", "secretString": "s" } }),
        ),
        (
            "catalog carrying a digit, which its pattern excludes",
            "AWSMarketplace1",
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "N", "secretString": "s" } }),
        ),
        (
            "catalog one character past its maximum",
            &over_long_catalog,
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "N", "secretString": "s" } }),
        ),
        (
            "product id carrying a bang",
            "AWSMarketplace",
            "prod!1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "N", "secretString": "s" } }),
        ),
        (
            "product id one character past its maximum",
            "AWSMarketplace",
            &over_long_product,
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "N", "secretString": "s" } }),
        ),
        (
            "agreementId outside its pattern",
            "AWSMarketplace",
            "prod-1",
            json!({ "agreementId": "bad id!", "deploymentParameter": { "name": "N", "secretString": "s" } }),
        ),
        (
            "agreementId absent",
            "AWSMarketplace",
            "prod-1",
            json!({ "deploymentParameter": { "name": "N", "secretString": "s" } }),
        ),
        (
            "deploymentParameter absent",
            "AWSMarketplace",
            "prod-1",
            json!({ "agreementId": "a1" }),
        ),
        (
            "parameter name absent",
            "AWSMarketplace",
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "secretString": "s" } }),
        ),
        (
            "parameter name empty",
            "AWSMarketplace",
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "", "secretString": "s" } }),
        ),
        (
            "parameter name one character past its maximum",
            "AWSMarketplace",
            "prod-1",
            json!({
                "agreementId": "a1",
                "deploymentParameter": { "name": "n".repeat(PARAMETER_NAME_MAX_LEN + 1), "secretString": "s" },
            }),
        ),
        (
            "parameter name outside its pattern",
            "AWSMarketplace",
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "bad name!", "secretString": "s" } }),
        ),
        (
            "secretString absent",
            "AWSMarketplace",
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "N" } }),
        ),
        (
            "secretString empty",
            "AWSMarketplace",
            "prod-1",
            json!({ "agreementId": "a1", "deploymentParameter": { "name": "N", "secretString": "" } }),
        ),
        (
            "a tag value one character past its maximum",
            "AWSMarketplace",
            "prod-1",
            json!({
                "agreementId": "a1",
                "deploymentParameter": { "name": "N", "secretString": "s" },
                "tags": { "k": "v".repeat(TAG_VALUE_MAX_LEN + 1) },
            }),
        ),
        (
            "a tag key outside its pattern",
            "AWSMarketplace",
            "prod-1",
            json!({
                "agreementId": "a1",
                "deploymentParameter": { "name": "N", "secretString": "s" },
                "tags": { "bad key!": "v" },
            }),
        ),
        (
            "clientToken shorter than its minimum",
            "AWSMarketplace",
            "prod-1",
            json!({
                "agreementId": "a1",
                "deploymentParameter": { "name": "N", "secretString": "s" },
                "clientToken": "too-short",
            }),
        ),
    ];

    for (case, catalog, product_id, body) in refused {
        let (status, answer) = put(&router, catalog, product_id, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }

    // A legal boundary row the refusals must not have swallowed: the shortest
    // product, agreement, name and secret the model admits.
    let (status, created) = put(
        &router,
        "AWSMarketplace",
        "p",
        &json!({ "agreementId": "a", "deploymentParameter": { "name": "n", "secretString": "s" } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the shortest legal request must succeed, got {status} {created:?}"
    );
}

#[tokio::test]
async fn a_tag_write_past_the_tagging_limits_is_refused_and_stores_nothing() {
    let (router, engine) = emulator();
    provision(&engine, "AWSMarketplace", "agmt-tags", &["prod-tags"]);

    let (status, created) = put(
        &router,
        "AWSMarketplace",
        "prod-tags",
        &json!({
            "agreementId": "agmt-tags",
            "deploymentParameter": { "name": "Tagged", "secretString": "s" },
            "tags": { "env": "dev" },
        }),
    )
    .await;
    assert_eq!(status, 200, "the setup Put must succeed, got {created:?}");
    let arn = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();

    // `TagResource$tags` targets the tag map the model leaves UNCONSTRAINED, so what
    // refuses a write here is AWS's own tagging limits and nothing else: a key must
    // carry at least one character and at most `TAG_KEY_MAX_LEN`, a value at most
    // `TAG_VALUE_MAX_LEN`, and the record may not pass `MAX_TAGS_PER_RESOURCE`. The
    // keys and values those limits ADMIT — spaces, letters outside ASCII, an empty
    // value — are the `unconstrained_tag_characters` suite's rule.
    //
    // The limits differ in WHOSE they are, which is what the expected code per row
    // says: a bound on what a tag IS is this service's own modelled input error,
    // while the COUNT of tags a resource may hold is the tagging front end's, with
    // its own unmodelled identity (`tag_count_front_end` owns that rule).
    let over_the_cap: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|i| (format!("extra-{i:02}"), "v".to_string()))
        .collect();
    let refused: Vec<(&str, BTreeMap<String, String>, &str)> = vec![
        ("an empty key", tag_map(&[("", "v")]), "ValidationException"),
        (
            "a key one character past its maximum",
            [("k".repeat(TAG_KEY_MAX_LEN + 1), "v".to_string())]
                .into_iter()
                .collect(),
            "ValidationException",
        ),
        (
            "a value one character past its maximum",
            [("k".to_string(), "v".repeat(TAG_VALUE_MAX_LEN + 1))]
                .into_iter()
                .collect(),
            "ValidationException",
        ),
        (
            "a merge one entry past the cap",
            over_the_cap,
            "BadRequestException",
        ),
    ];

    for (case, tags, expected_code) in refused {
        let (status, answer) = tag(&router, &arn, &tags).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            expected_code,
            "case {case:?}: expected {expected_code}, got {answer:?}"
        );
        let (_, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            tags_of(&listed),
            tag_map(&[("env", "dev")]),
            "case {case:?}: a refused TagResource must store nothing, got {listed:?}"
        );
    }

    // The boundary cases that DO fit: a key and a value at their maxima, and a
    // merge landing exactly on the cap.
    let at_the_bounds: BTreeMap<String, String> = [
        ("k".repeat(TAG_KEY_MAX_LEN), "v".repeat(TAG_VALUE_MAX_LEN)),
        ("punct".to_string(), "a/b+c=d.e:f@g_h-i".to_string()),
    ]
    .into_iter()
    .collect();
    let (status, answer) = tag(&router, &arn, &at_the_bounds).await;
    assert_eq!(
        status, 204,
        "tags at their modelled maxima must be accepted, got {status} {answer:?}"
    );
    let onto_the_cap: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE - 3)
        .map(|i| (format!("fill-{i:02}"), "v".to_string()))
        .collect();
    let (status, answer) = tag(&router, &arn, &onto_the_cap).await;
    assert_eq!(
        status, 204,
        "a merge landing exactly on the cap must be accepted, got {status} {answer:?}"
    );
    let (_, listed) = list_tags(&router, &arn).await;
    assert_eq!(
        tags_of(&listed).len(),
        MAX_TAGS_PER_RESOURCE,
        "expected the record to hold exactly the cap, got {listed:?}"
    );
}

#[tokio::test]
async fn a_client_token_identifies_exactly_one_write() {
    let (router, engine) = emulator();
    // Every identity this test aims the token at is a target the account holds, so
    // what the rows below observe is the token rule and not a missing agreement.
    provision(
        &engine,
        "AWSMarketplace",
        "agmt-token",
        &["prod-token", "prod-other"],
    );
    provision(&engine, "AWSMarketplace", "agmt-other", &["prod-token"]);
    // 36 characters of `^[a-zA-Z0-9/_+=.:@-]+$`, inside `ClientToken`'s len 32..64.
    let token = "0123456789abcdef0123456789abcdef0123";

    let first_body = json!({
        "agreementId": "agmt-token",
        "deploymentParameter": { "name": "Tokened", "secretString": "s" },
        "tags": { "env": "dev" },
        "clientToken": token,
    });
    let (status, created) = put(&router, "AWSMarketplace", "prod-token", &first_body).await;
    assert_eq!(status, 200, "the first Put must succeed, got {created:?}");

    // The same token aimed at the same identity is a REPLAY: the identical output,
    // and no second resource.
    let replay_body = json!({
        "agreementId": "agmt-token",
        "deploymentParameter": { "name": "Tokened", "secretString": "rotated" },
        "tags": { "late": "tag" },
        "clientToken": token,
    });
    let (status, replayed) = put(&router, "AWSMarketplace", "prod-token", &replay_body).await;
    assert_eq!(status, 200, "the replay must succeed, got {replayed:?}");
    assert_eq!(
        replayed, created,
        "a clientToken replay must answer the identical output, got {replayed:?} against {created:?}"
    );

    // The same token aimed at a DIFFERENT parameter conflicts: one token identifies
    // one write.
    let conflicting: Vec<(&str, &str, &str, Value)> = vec![
        (
            "another parameter name",
            "AWSMarketplace",
            "prod-token",
            json!({
                "agreementId": "agmt-token",
                "deploymentParameter": { "name": "Other", "secretString": "s" },
                "clientToken": token,
            }),
        ),
        (
            "another agreement",
            "AWSMarketplace",
            "prod-token",
            json!({
                "agreementId": "agmt-other",
                "deploymentParameter": { "name": "Tokened", "secretString": "s" },
                "clientToken": token,
            }),
        ),
        (
            "another product",
            "AWSMarketplace",
            "prod-other",
            json!({
                "agreementId": "agmt-token",
                "deploymentParameter": { "name": "Tokened", "secretString": "s" },
                "clientToken": token,
            }),
        ),
    ];
    let bound_id = created
        .get("deploymentParameterId")
        .and_then(Value::as_str)
        .expect("a minted id")
        .to_string();
    for (case, catalog, product_id, body) in conflicting {
        let (status, answer) = put(&router, catalog, product_id, &body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: reusing a clientToken must conflict, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {answer:?}"
        );
        assert_eq!(
            answer.get("resourceId").and_then(Value::as_str),
            Some(bound_id.as_str()),
            "case {case:?}: the conflict must name the parameter the token is bound to, \
             got {answer:?}"
        );
    }

    // A FRESH token for the same identity is the ordinary update-in-place path.
    let other_token = "fedcba9876543210fedcba9876543210fedc";
    let (status, updated) = put(
        &router,
        "AWSMarketplace",
        "prod-token",
        &json!({
            "agreementId": "agmt-token",
            "deploymentParameter": { "name": "Tokened", "secretString": "rotated" },
            "clientToken": other_token,
        }),
    )
    .await;
    assert_eq!(status, 200, "a fresh token must succeed, got {updated:?}");
    assert_eq!(
        updated.get("deploymentParameterId").and_then(Value::as_str),
        Some(bound_id.as_str()),
        "a fresh token for the same identity must still update in place, got {updated:?}"
    );
}

#[tokio::test]
async fn the_client_token_ledger_is_bounded_and_evicts_the_oldest_token() {
    let (router, engine) = emulator();
    provision(&engine, "AWSMarketplace", "agmt-ledger", &["prod-ledger"]);

    /// The i-th distinct token: 32 characters of `^[a-zA-Z0-9/_+=.:@-]+$`, the
    /// minimum `ClientToken` length.
    fn token(i: usize) -> String {
        format!("token-{i:026}")
    }
    /// A Put body for `name` under one fixed catalog/product/agreement.
    fn body(name: &str, client_token: &str) -> Value {
        json!({
            "agreementId": "agmt-ledger",
            "deploymentParameter": { "name": name, "secretString": "s" },
            "clientToken": client_token,
        })
    }

    // The ledger grows by one per fresh token, so it is capped with FIFO eviction;
    // past the cap the oldest token stops replaying, which is the same answer the
    // service gives for a token it never saw.
    let (status, created) = put(
        &router,
        "AWSMarketplace",
        "prod-ledger",
        &body("Ledger", &token(0)),
    )
    .await;
    assert_eq!(status, 200, "the first Put must succeed, got {created:?}");
    let first_id = created
        .get("deploymentParameterId")
        .and_then(Value::as_str)
        .expect("a minted id")
        .to_string();

    // Fill the ledger past its cap with fresh tokens aimed at the SAME identity, so
    // nothing new is minted and no other bound is tripped.
    for i in 1..=MAX_CLIENT_TOKENS {
        let (status, answer) = put(
            &router,
            "AWSMarketplace",
            "prod-ledger",
            &body("Ledger", &token(i)),
        )
        .await;
        assert_eq!(
            status, 200,
            "token {i} must update in place, got {status} {answer:?}"
        );
        assert_eq!(
            answer.get("deploymentParameterId").and_then(Value::as_str),
            Some(first_id.as_str()),
            "token {i} must not mint a second resource, got {answer:?}"
        );
    }

    // The oldest token has been evicted, so it no longer binds anything: aiming it
    // at another identity is an ordinary create rather than a conflict.
    let (status, evicted) = put(
        &router,
        "AWSMarketplace",
        "prod-ledger",
        &body("LedgerAfterEviction", &token(0)),
    )
    .await;
    assert_eq!(
        status, 200,
        "an evicted token must no longer conflict, got {status} {evicted:?}"
    );
    let evicted_id = evicted
        .get("deploymentParameterId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        is_parameter_id(&evicted_id) && evicted_id != first_id,
        "an evicted token must mint the new identity's own resource, got {evicted:?}"
    );

    // A token still inside the cap is still honoured, so eviction is bounded-oldest
    // rather than a lost ledger.
    let (status, answer) = put(
        &router,
        "AWSMarketplace",
        "prod-ledger",
        &body("LedgerStillBound", &token(MAX_CLIENT_TOKENS)),
    )
    .await;
    assert_eq!(
        status, 409,
        "the newest token must still be bound to its write, got {status} {answer:?}"
    );
    assert_eq!(
        answer.get("resourceId").and_then(Value::as_str),
        Some(first_id.as_str()),
        "the conflict must name the parameter the newest token is bound to, got {answer:?}"
    );
}

#[tokio::test]
async fn interleaved_tag_and_untag_calls_track_an_independent_tag_model() {
    let (router, engine) = emulator();
    provision(&engine, "AWSMarketplace", "agmt-model", &["prod-model"]);
    let (status, created) = put(
        &router,
        "AWSMarketplace",
        "prod-model",
        &json!({
            "agreementId": "agmt-model",
            "deploymentParameter": { "name": "Modelled", "secretString": "s" },
        }),
    )
    .await;
    assert_eq!(status, 200, "the setup Put must succeed, got {created:?}");
    let arn = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();

    // The invariant: for ANY interleaving of TagResource and UntagResource calls,
    // ListTagsForResource reports the map an independent model (a plain map with
    // insert-overwrites and removes) holds. The sequence is generated by a fixed-seed
    // LCG so the run is bounded and reproducible from this constant alone; nothing is
    // written to disk.
    const SEED: u64 = 0x2026_1021_2308_25A5;
    const ROUNDS: usize = 48;
    let keys: Vec<String> = ["alpha", "beta", "gamma.delta", "e+f", "g=h", "i:j", "k@l", "m_n"]
        .iter()
        .map(|k| (*k).to_string())
        .collect();
    let values = ["0", "1", "two", "three-3", "v.4"];

    let mut rng = SEED;
    let mut next = move || {
        rng = rng
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (rng >> 33) as usize
    };

    let mut model: BTreeMap<String, String> = BTreeMap::new();
    for round in 0..ROUNDS {
        let count = 1 + next() % 3;
        let chosen: Vec<String> = (0..count)
            .map(|_| keys[next() % keys.len()].clone())
            .collect();
        let is_tag = next() % 2 == 0;
        if is_tag {
            let writes: BTreeMap<String, String> = chosen
                .iter()
                .map(|key| (key.clone(), values[next() % values.len()].to_string()))
                .collect();
            let (status, answer) = tag(&router, &arn, &writes).await;
            assert_eq!(
                status, 204,
                "round {round} TagResource({writes:?}) must answer 204, got {status} {answer:?}"
            );
            model.extend(writes);
        } else {
            let (status, answer) = untag(&router, &arn, &chosen).await;
            assert_eq!(
                status, 204,
                "round {round} UntagResource({chosen:?}) must answer 204, got {status} {answer:?}"
            );
            for key in &chosen {
                model.remove(key);
            }
        }

        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "round {round}: ListTagsForResource must succeed, got {status} {listed:?}"
        );
        assert_eq!(
            tags_of(&listed),
            model,
            "round {round} (seed {SEED:#x}, {}): expected the model's tag map, got {listed:?}",
            if is_tag { "after a tag" } else { "after an untag" }
        );
    }

    // A read never changes what a later reader sees.
    let (_, first) = list_tags(&router, &arn).await;
    let (_, second) = list_tags(&router, &arn).await;
    assert_eq!(
        tags_of(&first),
        tags_of(&second),
        "ListTagsForResource is @readonly: two reads must agree, got {first:?} then {second:?}"
    );
}

// ---------------------------------------------------------------------------
// The DEPLOYMENT-CONFIGURATION slice of this service: the job that publishes a
// configuration, the configuration it publishes, and that configuration's tags.
//
// The rules these cases pin:
//
// * `StartDeploymentConfigurationJob` records ONE job and answers the
//   `IN_PROGRESS` status the model fixes for it; what the job DOES is decided by
//   the configuration it carries — a fulfillment URL that is a public absolute
//   HTTPS URL is published, one that is not fails the job with the modelled
//   `ENDPOINT_INVALID` code and leaves the active configuration as it was;
// * every later report — `GetDeploymentConfigurationJob`,
//   `ListDeploymentConfigurationJobs`, `GetDeploymentConfiguration` and the
//   tagging operations — is read out of those records, so what one answers is
//   what the others answer;
// * a job is addressed INSIDE the product and delivery option of its path;
// * `tags` are applied by the start that CREATES the configuration and changed
//   afterwards only through `TagResource` / `UntagResource`;
// * one `clientToken` replays its own start and refuses a different one.
//
// The harness above is reused: the same router, the same signed request.
// ---------------------------------------------------------------------------

use std::time::{Duration, SystemTime};

use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::ops::types::{
    DELIVERY_OPTION_TYPE_VALUES, DEPLOYMENT_CONFIGURATION_JOB_ERROR_CODE_VALUES,
    DEPLOYMENT_CONFIGURATION_JOB_STATUS_VALUES,
};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::deployment_configuration::{
    ENDPOINT_INVALID_MESSAGE, JOB_ERROR_ENDPOINT_INVALID, JOB_STATUS_FAILED,
    JOB_STATUS_IN_PROGRESS, JOB_STATUS_SUCCEEDED, MAX_RESULTS_DEFAULT,
};

/// The delivery option type every case below names: the one value the modelled
/// `DeliveryOptionType` declares, which
/// `the_statuses_and_error_codes_this_service_reports_are_values_the_model_declares`
/// asserts against the generated value list.
pub(crate) const DELIVERY_OPTION: &str = "data-as-a-service";

/// The longest `fulfillmentUrl` the model admits
/// (`DeploymentConfigurationFulfillmentUrl`, `length 1..2048`).
const FULFILLMENT_URL_MAX_LEN: usize = 2048;

/// How long a case looks again for a job's terminal status before giving up.
///
/// GENEROUS on purpose: the terminal status is due a span after the start and the
/// cases below wait for the CONDITION, so this is only the point at which a job
/// that never settles is reported as a failure rather than hanging the suite.
const JOB_BUDGET: Duration = Duration::from_secs(60);

/// How often a case looks again while waiting for that status.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A `DeploymentConfigurationDetail` carrying the Data as a Service member.
pub(crate) fn data_as_a_service(fulfillment_url: &str) -> Value {
    json!({ "dataAsAService": { "fulfillmentUrl": fulfillment_url } })
}

/// `POST /catalogs/{catalog}/products/{productId}/delivery-options/{type}/deployment-configuration/jobs`.
pub(crate) async fn start_job(
    router: &MultiServiceRouter,
    catalog: &str,
    product_id: &str,
    delivery_option_type: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!(
            "/catalogs/{}/products/{}/delivery-options/{}/deployment-configuration/jobs",
            pct(catalog),
            pct(product_id),
            pct(delivery_option_type)
        ),
        Some(body),
    )
    .await
}

/// `GET …/deployment-configuration/jobs/{jobId}`.
pub(crate) async fn get_job(
    router: &MultiServiceRouter,
    catalog: &str,
    product_id: &str,
    delivery_option_type: &str,
    job_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!(
            "/catalogs/{}/products/{}/delivery-options/{}/deployment-configuration/jobs/{}",
            pct(catalog),
            pct(product_id),
            pct(delivery_option_type),
            pct(job_id)
        ),
        None,
    )
    .await
}

/// `GET …/deployment-configuration/jobs`, with `query` appended verbatim (`""`
/// for a request that names neither `maxResults` nor `nextToken`).
pub(crate) async fn list_jobs(
    router: &MultiServiceRouter,
    catalog: &str,
    product_id: &str,
    delivery_option_type: &str,
    query: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!(
            "/catalogs/{}/products/{}/delivery-options/{}/deployment-configuration/jobs{query}",
            pct(catalog),
            pct(product_id),
            pct(delivery_option_type)
        ),
        None,
    )
    .await
}

/// `GET …/deployment-configuration`.
pub(crate) async fn get_configuration(
    router: &MultiServiceRouter,
    catalog: &str,
    product_id: &str,
    delivery_option_type: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!(
            "/catalogs/{}/products/{}/delivery-options/{}/deployment-configuration",
            pct(catalog),
            pct(product_id),
            pct(delivery_option_type)
        ),
        None,
    )
    .await
}

/// The `status` an answered job or job summary reports.
pub(crate) fn status_of(body: &Value) -> String {
    body.get("status")
        .and_then(Value::as_str)
        .unwrap_or("<no status>")
        .to_string()
}

/// The `jobId` an answered job reports.
fn job_id_of(body: &Value) -> String {
    body.get("jobId")
        .and_then(Value::as_str)
        .unwrap_or("<no jobId>")
        .to_string()
}

/// The `fulfillmentUrl` of the configuration an answer carries.
fn fulfillment_url_of(body: &Value) -> String {
    body.pointer("/configuration/dataAsAService/fulfillmentUrl")
        .and_then(Value::as_str)
        .unwrap_or("<no fulfillmentUrl>")
        .to_string()
}

/// The `jobId`s of a page, in the order the page lists them.
fn listed_job_ids(body: &Value) -> Vec<String> {
    body.get("jobs")
        .and_then(Value::as_array)
        .map(|jobs| jobs.iter().map(job_id_of).collect())
        .unwrap_or_default()
}

/// Whether `job_id` satisfies the modelled `DeploymentConfigurationJobId`
/// (`length 1..64`, `^[a-z0-9]+$`).
fn is_job_id(job_id: &str) -> bool {
    (1..=64).contains(&job_id.len())
        && job_id
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

/// The ARN this emulator must report for the deployment configuration of
/// `(catalog, product_id, delivery_option_type)`: the resource portion the
/// model's own `aws.api#arn` template states for the `DeploymentConfiguration`
/// resource.
fn expected_configuration_arn(
    catalog: &str,
    product_id: &str,
    delivery_option_type: &str,
) -> String {
    format!(
        "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:catalogs/{catalog}/products/{product_id}/\
         delivery-options/{delivery_option_type}"
    )
}

/// Look again at `job_id` until it has left `IN_PROGRESS`, and answer the body
/// that reported the terminal status.
///
/// The job's terminal status is due a span after the start and every call of this
/// service materialises what the clock has already made due, so this waits for the
/// CONDITION rather than for a length of time.
async fn settled_job(
    router: &MultiServiceRouter,
    case: &str,
    catalog: &str,
    product_id: &str,
    job_id: &str,
) -> Value {
    let deadline = SystemTime::now() + JOB_BUDGET;
    let mut last = (0_u16, Value::Null);
    while SystemTime::now() < deadline {
        last = get_job(router, catalog, product_id, DELIVERY_OPTION, job_id).await;
        assert_eq!(
            last.0, 200,
            "case {case:?}: GetDeploymentConfigurationJob must succeed, got {} {:?}",
            last.0, last.1
        );
        if status_of(&last.1) != JOB_STATUS_IN_PROGRESS {
            return last.1;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: job {job_id} never left {JOB_STATUS_IN_PROGRESS}, last answer {:?}",
        last.1
    );
}

/// One row of the publish table: a configuration to publish, and what the job
/// started with it does.
struct Publish {
    case: &'static str,
    product_id: &'static str,
    fulfillment_url: String,
    tags: BTreeMap<String, String>,
    expected_status: &'static str,
}

/// The publish table. The rows differ along what the rule generalises over: the
/// FORM of the fulfillment URL — its scheme, whether it is absolute at all, a
/// host name against a host that is an IP literal (v4 and v6), a port, a path and
/// a query, the modelled maximum length — and the tags the start carries.
///
/// The failing rows are the two halves of the condition the modelled
/// `ENDPOINT_INVALID` code is documented for: not a valid absolute HTTPS URL, and
/// a network location that is private or loopback.
fn publish_table() -> Vec<Publish> {
    let longest = {
        let prefix = "https://example.com/";
        format!(
            "{prefix}{}",
            "a".repeat(FULFILLMENT_URL_MAX_LEN - prefix.len())
        )
    };
    vec![
        Publish {
            case: "a plain https host, one tag",
            product_id: "prod-config-plain",
            fulfillment_url: "https://fulfilment.example.com/start".to_string(),
            tags: tag_map(&[("env", "prod")]),
            expected_status: JOB_STATUS_SUCCEEDED,
        },
        Publish {
            case: "a port, a path and a query, no tags",
            product_id: "prod-config-port",
            fulfillment_url: "https://fulfilment.example.com:8443/start?tenant=acme".to_string(),
            tags: BTreeMap::new(),
            expected_status: JOB_STATUS_SUCCEEDED,
        },
        Publish {
            case: "a public IPv4 literal",
            product_id: "prod-config-ipv4",
            fulfillment_url: "https://203.0.113.10/landing".to_string(),
            tags: tag_map(&[("team", "platform"), ("tier", "1")]),
            expected_status: JOB_STATUS_SUCCEEDED,
        },
        Publish {
            case: "a public IPv6 literal",
            product_id: "prod-config-ipv6",
            fulfillment_url: "https://[2001:db8::1]/landing".to_string(),
            tags: BTreeMap::new(),
            expected_status: JOB_STATUS_SUCCEEDED,
        },
        Publish {
            case: "a URL at the modelled maximum length",
            product_id: "prod-config-longest",
            fulfillment_url: longest,
            tags: BTreeMap::new(),
            expected_status: JOB_STATUS_SUCCEEDED,
        },
        Publish {
            case: "the http scheme, which is not https",
            product_id: "prod-config-http",
            fulfillment_url: "http://fulfilment.example.com/start".to_string(),
            tags: BTreeMap::new(),
            expected_status: JOB_STATUS_FAILED,
        },
        Publish {
            case: "no scheme at all, so not an absolute URL",
            product_id: "prod-config-relative",
            fulfillment_url: "fulfilment.example.com/start".to_string(),
            tags: BTreeMap::new(),
            expected_status: JOB_STATUS_FAILED,
        },
        Publish {
            case: "the loopback host name",
            product_id: "prod-config-localhost",
            fulfillment_url: "https://localhost/start".to_string(),
            tags: BTreeMap::new(),
            expected_status: JOB_STATUS_FAILED,
        },
        Publish {
            case: "a private IPv4 address",
            product_id: "prod-config-private",
            fulfillment_url: "https://10.0.0.5/start".to_string(),
            tags: BTreeMap::new(),
            expected_status: JOB_STATUS_FAILED,
        },
        Publish {
            case: "the IPv6 loopback address",
            product_id: "prod-config-ipv6-loopback",
            fulfillment_url: "https://[::1]/start".to_string(),
            tags: BTreeMap::new(),
            expected_status: JOB_STATUS_FAILED,
        },
    ]
}

#[tokio::test]
async fn a_job_publishes_or_fails_by_what_the_configuration_it_carries_is() {
    let (router, engine) = emulator();
    let products: Vec<&str> = publish_table().iter().map(|row| row.product_id).collect();
    provision(&engine, CATALOG, "agmt-config", &products);

    // Every job is started FIRST and looked at afterwards: the terminal status is
    // due a span after each start, so starting them together is what keeps this
    // case waiting for one span rather than one per row.
    let mut started: Vec<(Publish, String)> = Vec::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for row in publish_table() {
        let case = row.case;
        let mut body = json!({ "configuration": data_as_a_service(&row.fulfillment_url) });
        if !row.tags.is_empty() {
            body.as_object_mut().expect("an object body").insert(
                "tags".to_string(),
                Value::Object(row.tags.iter().map(|(k, v)| (k.clone(), json!(v))).collect()),
            );
        }
        let (status, answer) =
            start_job(&router, CATALOG, row.product_id, DELIVERY_OPTION, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartDeploymentConfigurationJob must succeed, got {status} {answer:?}"
        );
        let job_id = job_id_of(&answer);
        assert!(
            is_job_id(&job_id),
            "case {case:?}: expected a minted ^[a-z0-9]+$ job id of at most 64 characters, \
             got {job_id:?}"
        );
        assert!(
            minted.insert(job_id.clone()),
            "case {case:?}: expected a freshly minted job id, got the already-minted {job_id:?}"
        );
        assert_eq!(
            status_of(&answer),
            JOB_STATUS_IN_PROGRESS,
            "case {case:?}: the model fixes the started job's status, got {answer:?}"
        );
        assert_eq!(
            fulfillment_url_of(&answer),
            row.fulfillment_url,
            "case {case:?}: expected the submitted configuration back, got {answer:?}"
        );
        assert!(
            answer.get("createdAt").is_some(),
            "case {case:?}: a started job must report when it was created, got {answer:?}"
        );
        started.push((row, job_id));
    }

    for (row, job_id) in started {
        let case = row.case;
        let settled = settled_job(&router, case, CATALOG, row.product_id, &job_id).await;
        assert_eq!(
            status_of(&settled),
            row.expected_status,
            "case {case:?}: expected the job to reach {}, got {settled:?}",
            row.expected_status
        );
        assert_eq!(
            job_id_of(&settled),
            job_id,
            "case {case:?}: expected the job that was started, got {settled:?}"
        );
        assert_eq!(
            fulfillment_url_of(&settled),
            row.fulfillment_url,
            "case {case:?}: expected the configuration the job was started with, got {settled:?}"
        );
        assert!(
            settled.get("endedAt").is_some(),
            "case {case:?}: a job that reached a terminal status must report when it ended, \
             got {settled:?}"
        );

        let arn = settled.get("configurationArn").and_then(Value::as_str);
        let reported_code = settled.get("errorCode").and_then(Value::as_str);
        let reported_message = settled.get("message").and_then(Value::as_str);
        if row.expected_status == JOB_STATUS_SUCCEEDED {
            assert_eq!(
                arn,
                Some(
                    expected_configuration_arn(CATALOG, row.product_id, DELIVERY_OPTION).as_str()
                ),
                "case {case:?}: a succeeded job must report the published configuration's ARN, \
                 got {settled:?}"
            );
            assert_eq!(
                (reported_code, reported_message),
                (None, None),
                "case {case:?}: errorCode and message are reported only for a FAILED job, \
                 got {settled:?}"
            );

            // The read that proves the publish: the configuration is now the active
            // one of its resource.
            let (status, active) =
                get_configuration(&router, CATALOG, row.product_id, DELIVERY_OPTION).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetDeploymentConfiguration must report the published \
                 configuration, got {status} {active:?}"
            );
            assert_eq!(
                fulfillment_url_of(&active),
                row.fulfillment_url,
                "case {case:?}: expected the published configuration, got {active:?}"
            );
        } else {
            assert_eq!(
                arn, None,
                "case {case:?}: configurationArn is reported only for a SUCCEEDED job, \
                 got {settled:?}"
            );
            assert_eq!(
                reported_code,
                Some(JOB_ERROR_ENDPOINT_INVALID),
                "case {case:?}: expected the modelled error code for a URL that cannot be \
                 published, got {settled:?}"
            );
            assert_eq!(
                reported_message,
                Some(ENDPOINT_INVALID_MESSAGE),
                "case {case:?}: expected the documented reason for that code, got {settled:?}"
            );

            // A failed job publishes nothing, so the resource still has no active
            // configuration at all.
            let (status, answer) =
                get_configuration(&router, CATALOG, row.product_id, DELIVERY_OPTION).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a failed job must leave the resource without an active \
                 configuration, got {status} {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case:?}: expected the modelled not-found shape, got {answer:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_job_and_a_configuration_are_addressed_inside_the_product_the_path_names() {
    let (router, engine) = emulator();
    provision(
        &engine,
        CATALOG,
        "agmt-addressed",
        &["prod-addressed", "prod-other", "prod-empty"],
    );

    let (status, started) = start_job(
        &router,
        CATALOG,
        "prod-addressed",
        DELIVERY_OPTION,
        &json!({ "configuration": data_as_a_service("https://addressed.example.com/start") }),
    )
    .await;
    assert_eq!(status, 200, "the setup start must succeed, got {started:?}");
    let job_id = job_id_of(&started);

    // Each row names a resource the request's path addresses and the segment the
    // service must blame when it does not resolve. A job of ANOTHER product is not
    // this resource's job, which the model calls out in as many words.
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a job id nothing minted",
            "prod-addressed",
            "0123456789abcdef",
            "JobId",
        ),
        (
            "a job of another product",
            "prod-other",
            job_id.as_str(),
            "JobId",
        ),
        (
            "a product the account does not hold",
            "prod-unheld",
            job_id.as_str(),
            "ProductId",
        ),
    ];
    for (case, product_id, requested_job, blamed) in rows {
        let (status, answer) =
            get_job(&router, CATALOG, product_id, DELIVERY_OPTION, requested_job).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a not-found, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled not-found shape, got {answer:?}"
        );
        let reported = message(&answer);
        assert!(
            reported.starts_with(&format!("{blamed}: ")),
            "case {case:?}: the not-found must blame {blamed}, got {reported:?}"
        );
    }

    // The job its own path addresses still resolves, so nothing above refused it for
    // the wrong reason.
    let (status, answer) =
        get_job(&router, CATALOG, "prod-addressed", DELIVERY_OPTION, &job_id).await;
    assert_eq!(
        status, 200,
        "the job of the product that started it must resolve, got {status} {answer:?}"
    );

    // A product nobody has published a configuration for has no active
    // configuration, and a product the account does not hold is the outer resource.
    for (case, product_id, blamed) in [
        ("a held product with no succeeded job", "prod-empty", "DeliveryOptionType"),
        ("a product the account does not hold", "prod-unheld", "ProductId"),
    ] {
        let (status, answer) = get_configuration(&router, CATALOG, product_id, DELIVERY_OPTION).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a not-found, got {status} {answer:?}"
        );
        assert!(
            message(&answer).starts_with(&format!("{blamed}: ")),
            "case {case:?}: the not-found must blame {blamed}, got {answer:?}"
        );
    }

    // A start against a product the account does not hold writes nothing: a product
    // exists because a seller published it, out of band of this API.
    let (status, answer) = start_job(
        &router,
        CATALOG,
        "prod-unheld",
        DELIVERY_OPTION,
        &json!({ "configuration": data_as_a_service("https://unheld.example.com/start") }),
    )
    .await;
    assert_eq!(
        status, 404,
        "a start for a product the account does not hold must be a not-found, \
         got {status} {answer:?}"
    );
    assert!(
        message(&answer).starts_with("ProductId: "),
        "that not-found must blame ProductId, got {answer:?}"
    );
    let (status, listed) = list_jobs(&router, CATALOG, "prod-unheld", DELIVERY_OPTION, "").await;
    assert_eq!(
        status, 200,
        "listing a product with no jobs must succeed, got {status} {listed:?}"
    );
    assert_eq!(
        listed_job_ids(&listed),
        Vec::<String>::new(),
        "the refused start must have recorded no job, got {listed:?}"
    );
}

#[tokio::test]
async fn list_reports_one_resources_jobs_newest_first_one_page_at_a_time() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, "agmt-paged", &["prod-paged", "prod-quiet"]);

    // Five jobs for one resource, in a known order, plus one for a sibling product
    // that no page below may carry.
    let mut in_start_order: Vec<String> = Vec::new();
    for i in 0..5 {
        let (status, started) = start_job(
            &router,
            CATALOG,
            "prod-paged",
            DELIVERY_OPTION,
            &json!({
                "configuration": data_as_a_service(&format!("https://paged-{i}.example.com/start")),
            }),
        )
        .await;
        assert_eq!(status, 200, "start {i} must succeed, got {started:?}");
        in_start_order.push(job_id_of(&started));
    }
    let (status, other) = start_job(
        &router,
        CATALOG,
        "prod-quiet",
        DELIVERY_OPTION,
        &json!({ "configuration": data_as_a_service("https://quiet.example.com/start") }),
    )
    .await;
    assert_eq!(status, 200, "the sibling start must succeed, got {other:?}");
    let sibling = job_id_of(&other);

    let newest_first: Vec<String> = in_start_order.iter().rev().cloned().collect();

    // A page holds at most `maxResults` jobs and the token names the job the NEXT
    // page starts at, so walking the pages walks the order exactly once.
    for page_size in [1_usize, 2, 5, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={}", pct(token)),
                None => format!("?maxResults={page_size}"),
            };
            let (status, page) =
                list_jobs(&router, CATALOG, "prod-paged", DELIVERY_OPTION, &query).await;
            assert_eq!(
                status, 200,
                "page size {page_size}: ListDeploymentConfigurationJobs must succeed, \
                 got {status} {page:?}"
            );
            let ids = listed_job_ids(&page);
            assert!(
                ids.len() <= page_size,
                "page size {page_size}: a page must hold at most that many jobs, got {ids:?}"
            );
            assert!(
                !ids.contains(&sibling),
                "page size {page_size}: a page must carry only this resource's jobs, got {ids:?}"
            );
            for job in &page
                .get("jobs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                assert!(
                    DEPLOYMENT_CONFIGURATION_JOB_STATUS_VALUES.contains(&status_of(job).as_str()),
                    "page size {page_size}: a summary must report a modelled status, got {job:?}"
                );
                assert!(
                    job.get("createdAt").is_some() && job.get("configuration").is_some(),
                    "page size {page_size}: a summary must report its configuration and \
                     creation time, got {job:?}"
                );
            }
            walked.extend(ids);
            pages += 1;
            token = page
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= newest_first.len(),
                "page size {page_size}: paging must end, walked {walked:?}"
            );
        }
        assert_eq!(
            walked, newest_first,
            "page size {page_size}: expected every job once, newest first, got {walked:?}"
        );
    }

    // No `maxResults`: the member's modelled default is the page size, which is more
    // than the jobs this resource has, so one page carries them all and there is no
    // token.
    assert!(
        newest_first.len() < MAX_RESULTS_DEFAULT,
        "this case must stay inside the modelled default page size of {MAX_RESULTS_DEFAULT}"
    );
    let (status, page) = list_jobs(&router, CATALOG, "prod-paged", DELIVERY_OPTION, "").await;
    assert_eq!(status, 200, "the default page must succeed, got {page:?}");
    assert_eq!(
        listed_job_ids(&page),
        newest_first,
        "the default page size must carry every job of this resource, got {page:?}"
    );
    assert_eq!(
        page.get("nextToken"),
        None,
        "the last page must carry no token, got {page:?}"
    );

    // A token of any other provenance names no page in that order.
    for (case, token) in [
        ("a job id of another resource", sibling.as_str()),
        ("a job id nothing minted", "0123456789abcdef"),
        ("text that is no job id at all", "not-a-token"),
    ] {
        let (status, answer) = list_jobs(
            &router,
            CATALOG,
            "prod-paged",
            DELIVERY_OPTION,
            &format!("?nextToken={}", pct(token)),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unusable nextToken must be refused, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected the modelled input error, got {answer:?}"
        );
        assert_eq!(
            field_name(&answer),
            "nextToken",
            "case {case:?}: the refusal must name the member, got {answer:?}"
        );
    }
}

#[tokio::test]
async fn tags_are_applied_by_the_creating_start_and_then_only_by_the_tagging_operations() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, "agmt-tagged", &["prod-tagged"]);

    let created = tag_map(&[("env", "prod"), ("owner", "seller")]);
    let (status, started) = start_job(
        &router,
        CATALOG,
        "prod-tagged",
        DELIVERY_OPTION,
        &json!({
            "configuration": data_as_a_service("https://tagged.example.com/start"),
            "tags": created.iter().map(|(k, v)| (k.clone(), json!(v))).collect::<serde_json::Map<String, Value>>(),
        }),
    )
    .await;
    assert_eq!(status, 200, "the creating start must succeed, got {started:?}");
    let job_id = job_id_of(&started);
    let settled = settled_job(&router, "creating start", CATALOG, "prod-tagged", &job_id).await;
    assert_eq!(
        status_of(&settled),
        JOB_STATUS_SUCCEEDED,
        "the creating start must publish its configuration, got {settled:?}"
    );
    let arn = settled
        .get("configurationArn")
        .and_then(Value::as_str)
        .expect("a succeeded job reports the published configuration's ARN")
        .to_string();

    // The round trip: the configuration the job published is a resource the tagging
    // operations address, holding the tags the START carried.
    let (status, listed) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 200,
        "the published configuration's tags must be readable, got {status} {listed:?}"
    );
    assert_eq!(
        tags_of(&listed),
        created,
        "expected the creating start's tags, got {listed:?}"
    );

    // A start for a configuration that already exists may not carry tags at all.
    let (status, refused) = start_job(
        &router,
        CATALOG,
        "prod-tagged",
        DELIVERY_OPTION,
        &json!({
            "configuration": data_as_a_service("https://tagged.example.com/second"),
            "tags": { "late": "no" },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "tags on a start for an existing configuration must be refused, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected the modelled input error, got {refused:?}"
    );
    assert_eq!(
        field_name(&refused),
        "tags",
        "the refusal must name the member, got {refused:?}"
    );

    // …and it wrote nothing: the configuration, and its tags, are as the create left
    // them.
    let (_, active) = get_configuration(&router, CATALOG, "prod-tagged", DELIVERY_OPTION).await;
    assert_eq!(
        fulfillment_url_of(&active),
        "https://tagged.example.com/start",
        "the refused start must not have published its configuration, got {active:?}"
    );

    // The tagging operations are what changes those tags.
    let (status, answer) = tag(&router, &arn, &tag_map(&[("env", "staging"), ("cost", "7")])).await;
    assert_eq!(
        status, 204,
        "TagResource on a configuration must answer 204, got {status} {answer:?}"
    );
    let (_, listed) = list_tags(&router, &arn).await;
    assert_eq!(
        tags_of(&listed),
        tag_map(&[("env", "staging"), ("owner", "seller"), ("cost", "7")]),
        "expected the creating start's tags with the tagging write merged on, got {listed:?}"
    );
    let (status, answer) = untag(&router, &arn, &["owner".to_string(), "never".to_string()]).await;
    assert_eq!(
        status, 204,
        "UntagResource on a configuration must answer 204, got {status} {answer:?}"
    );
    let (_, listed) = list_tags(&router, &arn).await;
    assert_eq!(
        tags_of(&listed),
        tag_map(&[("env", "staging"), ("cost", "7")]),
        "expected the removed key gone and the rest intact, got {listed:?}"
    );

    // A second start WITHOUT tags publishes over the configuration: the resource
    // keeps its ARN and the tags the tagging operations left on it.
    let (status, second) = start_job(
        &router,
        CATALOG,
        "prod-tagged",
        DELIVERY_OPTION,
        &json!({ "configuration": data_as_a_service("https://tagged.example.com/third") }),
    )
    .await;
    assert_eq!(status, 200, "the second start must succeed, got {second:?}");
    let second_id = job_id_of(&second);
    let settled = settled_job(&router, "second start", CATALOG, "prod-tagged", &second_id).await;
    assert_eq!(
        settled.get("configurationArn").and_then(Value::as_str),
        Some(arn.as_str()),
        "publishing over a configuration must keep the resource's ARN, got {settled:?}"
    );
    let (_, active) = get_configuration(&router, CATALOG, "prod-tagged", DELIVERY_OPTION).await;
    assert_eq!(
        fulfillment_url_of(&active),
        "https://tagged.example.com/third",
        "the active configuration must be the newest succeeded job's, got {active:?}"
    );
    let (_, listed) = list_tags(&router, &arn).await;
    assert_eq!(
        tags_of(&listed),
        tag_map(&[("env", "staging"), ("cost", "7")]),
        "a later publish must not change the configuration's tags, got {listed:?}"
    );

    // A tag set outside what a tag of this service may hold is refused on the
    // start's own input, and nothing is recorded for it.
    let (status, refused) = start_job(
        &router,
        CATALOG,
        "prod-tagged",
        DELIVERY_OPTION,
        &json!({
            "configuration": data_as_a_service("https://tagged.example.com/fourth"),
            "tags": { "aws:created-by": "me" },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a reserved tag key must be refused, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected the modelled input error, got {refused:?}"
    );
}

#[tokio::test]
async fn one_client_token_replays_its_start_and_refuses_a_different_one() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, "agmt-token", &["prod-token", "prod-token-2"]);

    // Inside `ClientToken`'s modelled `length 32..64` / `^[a-zA-Z0-9/_+=.:@-]+$`.
    let token = "cfg-token-0000000000000000000000000001";
    let configuration = data_as_a_service("https://token.example.com/start");
    let body = json!({ "configuration": configuration, "clientToken": token });

    let (status, first) = start_job(&router, CATALOG, "prod-token", DELIVERY_OPTION, &body).await;
    assert_eq!(status, 200, "the first start must succeed, got {first:?}");
    let job_id = job_id_of(&first);

    // The same request through the same token answers with the job it started, and
    // starts no second one.
    for attempt in 1..=2 {
        let (status, replayed) =
            start_job(&router, CATALOG, "prod-token", DELIVERY_OPTION, &body).await;
        assert_eq!(
            status, 200,
            "replay {attempt} must succeed, got {status} {replayed:?}"
        );
        assert_eq!(
            job_id_of(&replayed),
            job_id,
            "replay {attempt} must answer with the original job, got {replayed:?}"
        );
    }
    let (_, listed) = list_jobs(&router, CATALOG, "prod-token", DELIVERY_OPTION, "").await;
    assert_eq!(
        listed_job_ids(&listed),
        vec![job_id.clone()],
        "a replayed token must have started exactly one job, got {listed:?}"
    );

    // A DIFFERENT request through that token is a conflict, and starts nothing.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "another configuration",
            "prod-token",
            json!({
                "configuration": data_as_a_service("https://token.example.com/other"),
                "clientToken": token,
            }),
        ),
        (
            "another product",
            "prod-token-2",
            json!({ "configuration": configuration, "clientToken": token }),
        ),
        (
            "the same configuration with tags",
            "prod-token",
            json!({
                "configuration": configuration,
                "clientToken": token,
                "tags": { "env": "prod" },
            }),
        ),
    ];
    for (case, product_id, body) in rows {
        let (status, answer) = start_job(&router, CATALOG, product_id, DELIVERY_OPTION, &body).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a token reused for a different request must conflict, \
             got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ConflictException",
            "case {case:?}: expected the modelled conflict, got {answer:?}"
        );
        assert_eq!(
            answer.get("resourceId").and_then(Value::as_str),
            Some(job_id.as_str()),
            "case {case:?}: the conflict must name the job the token is bound to, got {answer:?}"
        );
    }
    let (_, listed) = list_jobs(&router, CATALOG, "prod-token", DELIVERY_OPTION, "").await;
    assert_eq!(
        listed_job_ids(&listed),
        vec![job_id.clone()],
        "a refused start must record no job, got {listed:?}"
    );

    // Another token is another start.
    let (status, fresh) = start_job(
        &router,
        CATALOG,
        "prod-token",
        DELIVERY_OPTION,
        &json!({
            "configuration": configuration,
            "clientToken": "cfg-token-0000000000000000000000000002",
        }),
    )
    .await;
    assert_eq!(status, 200, "a fresh token must start a job, got {fresh:?}");
    assert_ne!(
        job_id_of(&fresh),
        job_id,
        "a fresh token must start its own job, got {fresh:?}"
    );
}

#[tokio::test]
async fn the_statuses_and_error_codes_this_service_reports_are_values_the_model_declares() {
    // Every value this service reports for a job comes from the model's own enums,
    // read here off the generated value lists rather than restated.
    assert!(
        DELIVERY_OPTION_TYPE_VALUES.contains(&DELIVERY_OPTION),
        "expected a modelled DeliveryOptionType, got {DELIVERY_OPTION:?} of \
         {DELIVERY_OPTION_TYPE_VALUES:?}"
    );
    for status in [
        JOB_STATUS_IN_PROGRESS,
        JOB_STATUS_SUCCEEDED,
        JOB_STATUS_FAILED,
    ] {
        assert!(
            DEPLOYMENT_CONFIGURATION_JOB_STATUS_VALUES.contains(&status),
            "expected a modelled DeploymentConfigurationJobStatus, got {status:?} of \
             {DEPLOYMENT_CONFIGURATION_JOB_STATUS_VALUES:?}"
        );
    }
    assert!(
        DEPLOYMENT_CONFIGURATION_JOB_ERROR_CODE_VALUES.contains(&JOB_ERROR_ENDPOINT_INVALID),
        "expected a modelled DeploymentConfigurationJobErrorCode, got \
         {JOB_ERROR_ENDPOINT_INVALID:?} of {DEPLOYMENT_CONFIGURATION_JOB_ERROR_CODE_VALUES:?}"
    );
}
