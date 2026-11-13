// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the resource-tagging slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. The resource ARN rides in the URI path, as the
//! modelled `@httpLabel` puts it, percent-encoded the way a non-greedy label is.
//!
//! The rule under test: the tags of a resource are part of that resource's stored record,
//! so `TagResource` merges the keys a request carries into the tags the resource the ARN
//! names already holds — a key it already holds taking the new value — and
//! `ListTagsForResource` reports exactly the tags that record then carries, whether they
//! were written by a tagging call or by the create that made the resource. An ARN no
//! resource of the calling Region carries names nothing to tag or report, whatever
//! resource kind it names: the WRITE reports that as this service's own modelled
//! not-found error, and the LISTING as the resource-tagging layer's own `NotFoundException`
//! (HTTP 404, the fixed sentence "Resource was not found").
//!
//! The rows span what the rule generalises over: which resource kind the ARN names, how
//! the tags got there (a create or a tagging call), how many keys a call carries and what
//! they look like, and whether a key is new or already present.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it creates carry it.
const REGION: &str = "us-west-2";

/// A second Region, for the cases that check a regional endpoint reaches only its own
/// Region's resources.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The most tags one resource carries: the modelled `TagMap`'s own length maximum
/// (`smithy_query(query="shape",shape="TagMap")` reports `len 1..50`).
const MAX_RESOURCE_TAGS: usize = 50;

/// A build component document: one `build` phase, which is all `CreateComponent` needs to
/// store a component this suite can tag.
const BUILD_DOCUMENT: &str = "name: HelloWorld\nschemaVersion: 1.0\nphases:\n  - name: build\n    steps:\n      - name: Greet\n        action: ExecuteBash\n        inputs:\n          commands:\n            - echo hello\n";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `imagebuilder` in `region` — the signal the router routes on and
/// the handlers read the Region from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("imagebuilder.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDIMAGEBUILDER/20260101/{region}/imagebuilder/\
                 aws4_request, SignedHeaders=host, Signature=0000"
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
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
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

/// Percent-encode one path segment, as an AWS SDK encodes a NON-greedy `@httpLabel`:
/// everything outside the RFC 3986 unreserved set, so the `:` and `/` of an ARN travel as
/// `%3A` and `%2F` and the whole ARN stays ONE path segment.
fn encode_label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// `ListTagsForResource` for `arn`, in `region`.
async fn list_tags_in(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/tags/{}", encode_label(arn)),
        None,
    )
    .await
}

/// `ListTagsForResource` for `arn`, in [`REGION`].
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    list_tags_in(router, REGION, arn).await
}

/// `TagResource` writing `tags` onto `arn`, in `region`.
async fn tag_resource_in(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    tags: &[(&str, &str)],
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/tags/{}", encode_label(arn)),
        Some(&json!({ "tags": tag_object(tags) })),
    )
    .await
}

/// `TagResource` writing `tags` onto `arn`, in [`REGION`].
async fn tag_resource(
    router: &MultiServiceRouter,
    arn: &str,
    tags: &[(&str, &str)],
) -> (u16, Value) {
    tag_resource_in(router, REGION, arn, tags).await
}

/// The pairs as the JSON object a `tags` member carries.
fn tag_object(tags: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert((*key).to_string(), json!(value));
    }
    Value::Object(map)
}

/// The pairs as the map an assertion compares against, so key order never matters.
fn tag_map(tags: &[(&str, &str)]) -> BTreeMap<String, String> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The `tags` a `ListTagsForResource` answer reports, as a map.
fn reported_tags(case: &str, body: &Value) -> BTreeMap<String, String> {
    let tags = body
        .get("tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags member in {body}"));
    let object = tags
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected tags to be a map, got {tags}"));
    object
        .iter()
        .map(|(key, value)| {
            let text = value
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected text for tag {key}, got {value}"))
                .to_string();
            (key.clone(), text)
        })
        .collect()
}

/// The error code a refusal carries, from `__type` (restJson1 renders the shape there,
/// optionally namespace-qualified).
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error shape, got {body}"))
        .rsplit(['#', '/'])
        .next()
        .expect("a non-empty error type")
        .to_string()
}

/// Assert `status` and the error code of a refusal.
fn assert_refused(case: &str, status: u16, body: &Value, expect_status: u16, expect_code: &str) {
    assert_eq!(
        status, expect_status,
        "case {case:?}: expected HTTP {expect_status}, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, expect_code,
        "case {case:?}: expected the error code {expect_code:?}, got {code:?}"
    );
}

/// Assert a call succeeded, and answer its body.
fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    body
}

/// The text one member of a response carries.
fn text(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected text in {name} of {body}"))
        .to_string()
}

/// Create the infrastructure configuration `name` carries in `region`, optionally with
/// `tags`, and answer its ARN.
async fn create_configuration(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    tags: Option<&[(&str, &str)]>,
) -> String {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("clientToken".to_string(), json!(format!("token-{name}")));
    body.insert(
        "instanceProfileName".to_string(),
        json!("EC2InstanceProfileForImageBuilder"),
    );
    if let Some(tags) = tags {
        body.insert("tags".to_string(), tag_object(tags));
    }
    let (status, answer) = call_in(
        router,
        region,
        "PUT",
        "/CreateInfrastructureConfiguration",
        Some(&Value::Object(body)),
    )
    .await;
    let answer = assert_ok(
        &format!("create the infrastructure configuration {name}"),
        status,
        answer,
    );
    text(
        &format!("create the infrastructure configuration {name}"),
        &answer,
        "infrastructureConfigurationArn",
    )
}

/// Create one build version of the component `name` carries in `region`, optionally with
/// `tags`, and answer its build version ARN.
async fn create_component(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    tags: Option<&[(&str, &str)]>,
) -> String {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!("1.0.0"));
    body.insert("platform".to_string(), json!("Linux"));
    body.insert("clientToken".to_string(), json!(format!("token-{name}")));
    body.insert("data".to_string(), json!(BUILD_DOCUMENT));
    if let Some(tags) = tags {
        body.insert("tags".to_string(), tag_object(tags));
    }
    let (status, answer) =
        call_in(router, region, "PUT", "/CreateComponent", Some(&Value::Object(body))).await;
    let answer = assert_ok(&format!("create the component {name}"), status, answer);
    text(
        &format!("create the component {name}"),
        &answer,
        "componentBuildVersionArn",
    )
}

/// Create the image recipe `name` carries in `region`, optionally with `tags`, and answer
/// its ARN.
async fn create_recipe(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    tags: Option<&[(&str, &str)]>,
) -> String {
    let mut body = Map::new();
    body.insert("name".to_string(), json!(name));
    body.insert("semanticVersion".to_string(), json!("1.0.0"));
    body.insert("parentImage".to_string(), json!("ami-0123456789abcdef0"));
    body.insert("clientToken".to_string(), json!(format!("token-{name}")));
    if let Some(tags) = tags {
        body.insert("tags".to_string(), tag_object(tags));
    }
    let (status, answer) = call_in(
        router,
        region,
        "PUT",
        "/CreateImageRecipe",
        Some(&Value::Object(body)),
    )
    .await;
    let answer = assert_ok(&format!("create the image recipe {name}"), status, answer);
    text(
        &format!("create the image recipe {name}"),
        &answer,
        "imageRecipeArn",
    )
}

/// Which resource kind a row tags, and how a resource of that kind is created and read
/// back through its own Get.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    InfrastructureConfiguration,
    Component,
    ImageRecipe,
}

impl Kind {
    /// Create a resource of this kind in `region`, optionally carrying `tags`.
    async fn create(
        self,
        router: &MultiServiceRouter,
        region: &str,
        name: &str,
        tags: Option<&[(&str, &str)]>,
    ) -> String {
        match self {
            Self::InfrastructureConfiguration => {
                create_configuration(router, region, name, tags).await
            }
            Self::Component => create_component(router, region, name, tags).await,
            Self::ImageRecipe => create_recipe(router, region, name, tags).await,
        }
    }

    /// The tags this kind's OWN read reports for `arn` — the tags member of the resource
    /// the Get answers with.
    async fn tags_from_get(
        self,
        router: &MultiServiceRouter,
        case: &str,
        arn: &str,
    ) -> BTreeMap<String, String> {
        let (uri, member) = match self {
            Self::InfrastructureConfiguration => (
                format!("/GetInfrastructureConfiguration?infrastructureConfigurationArn={arn}"),
                "infrastructureConfiguration",
            ),
            Self::Component => (
                format!("/GetComponent?componentBuildVersionArn={arn}"),
                "component",
            ),
            Self::ImageRecipe => (
                format!("/GetImageRecipe?imageRecipeArn={arn}"),
                "imageRecipe",
            ),
        };
        let (status, body) = call_in(router, REGION, "GET", &uri, None).await;
        let body = assert_ok(case, status, body);
        let resource = body
            .get(member)
            .unwrap_or_else(|| panic!("case {case:?}: expected a {member} in {body}"));
        match resource.get("tags") {
            Some(_) => reported_tags(case, resource),
            None => BTreeMap::new(),
        }
    }
}

/// One row of the tagging table: a resource kind, the tags its create carried, and the two
/// tagging calls made against it afterwards.
struct Row {
    case: &'static str,
    kind: Kind,
    name: &'static str,
    created_with: Option<&'static [(&'static str, &'static str)]>,
    first_call: &'static [(&'static str, &'static str)],
    second_call: &'static [(&'static str, &'static str)],
    /// The tags the resource carries once both calls have been served.
    expected: &'static [(&'static str, &'static str)],
}

/// The tagging table. The rows differ along what the rule generalises over: the resource
/// kind, whether the tags started at the create, how many keys one call carries, the shape
/// of a key and a value (the modelled `TagKey` pattern admits spaces, `.`, `:`, `/`, `=`,
/// `+`, `-` and `@`, and `TagValue` admits the empty string), and whether the second call
/// adds a key or replaces one.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one tag on an infrastructure configuration",
            kind: Kind::InfrastructureConfiguration,
            name: "Tagged Infra",
            created_with: None,
            first_call: &[("Owner", "platform")],
            second_call: &[],
            expected: &[("Owner", "platform")],
        },
        Row {
            case: "a second call replaces one key and adds another",
            kind: Kind::InfrastructureConfiguration,
            name: "Merged Infra",
            created_with: None,
            first_call: &[("Owner", "platform"), ("Stage", "dev")],
            second_call: &[("Stage", "prod"), ("Team", "builders")],
            expected: &[("Owner", "platform"), ("Stage", "prod"), ("Team", "builders")],
        },
        Row {
            case: "tags a create carried are merged into, not replaced",
            kind: Kind::Component,
            name: "Tagged Component",
            created_with: Some(&[("Source", "create"), ("Stage", "dev")]),
            first_call: &[("Stage", "prod")],
            second_call: &[("Added", "later")],
            expected: &[("Source", "create"), ("Stage", "prod"), ("Added", "later")],
        },
        Row {
            case: "keys and values across the modelled character set",
            kind: Kind::ImageRecipe,
            name: "Tagged Recipe",
            created_with: None,
            first_call: &[
                ("cost center", "eng/platform"),
                ("app.version", "1.2.3+build"),
                ("empty", ""),
                ("contact@example", "a=b"),
            ],
            second_call: &[("empty", "now-set")],
            expected: &[
                ("cost center", "eng/platform"),
                ("app.version", "1.2.3+build"),
                ("empty", "now-set"),
                ("contact@example", "a=b"),
            ],
        },
        Row {
            case: "an image recipe created with tags and never tagged again",
            kind: Kind::ImageRecipe,
            name: "Created With Tags",
            created_with: Some(&[("Owner", "platform"), ("Stage", "prod")]),
            first_call: &[],
            second_call: &[],
            expected: &[("Owner", "platform"), ("Stage", "prod")],
        },
    ]
}

#[tokio::test]
async fn tags_a_call_writes_are_the_tags_every_later_read_reports() {
    for row in rows() {
        let router = emulator();
        let arn = row
            .kind
            .create(&router, REGION, row.name, row.created_with)
            .await;

        for (step, call) in [row.first_call, row.second_call].into_iter().enumerate() {
            if call.is_empty() {
                continue;
            }
            let (status, body) = tag_resource(&router, &arn, call).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected tagging call {step} to answer HTTP 200, got {status} with \
                 {body}",
                row.case
            );
        }

        let (status, body) = list_tags(&router, &arn).await;
        let body = assert_ok(row.case, status, body);
        let actual = reported_tags(row.case, &body);
        let expected = tag_map(row.expected);
        assert_eq!(
            actual, expected,
            "case {:?}: expected the tags {expected:?}, got {actual:?}",
            row.case
        );

        // The tags live on the resource's own record, so the resource's own read reports
        // the same map.
        let from_get = row.kind.tags_from_get(&router, row.case, &arn).await;
        assert_eq!(
            from_get, expected,
            "case {:?}: expected the resource's own read to report {expected:?}, got {from_get:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_resource_nothing_tagged_reports_an_empty_tag_map() {
    let kinds = [
        ("an untagged infrastructure configuration", Kind::InfrastructureConfiguration),
        ("an untagged component", Kind::Component),
        ("an untagged image recipe", Kind::ImageRecipe),
    ];
    for (case, kind) in kinds {
        let router = emulator();
        let arn = kind.create(&router, REGION, "Untagged Thing", None).await;
        let (status, body) = list_tags(&router, &arn).await;
        let body = assert_ok(case, status, body);
        let actual = reported_tags(case, &body);
        assert!(
            actual.is_empty(),
            "case {case:?}: expected no tags, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_this_region_holds_no_resource_under_is_reported_missing() {
    let router = emulator();
    // One resource of each stored kind exists, so a refusal below is about the ARN in the
    // request and not about an empty store.
    let configuration = create_configuration(&router, REGION, "Present Infra", None).await;
    let component = create_component(&router, REGION, "Present Component", None).await;
    create_recipe(&router, REGION, "Present Recipe", None).await;

    // Each row is a well-formed `ImageBuilderArn` the calling Region holds nothing under:
    // a never-created resource of a kind this store holds, a resource kind it holds none
    // of at all (the modelled pattern admits ten), and a resource that exists in ANOTHER
    // Region.
    let cases: Vec<(String, String)> = vec![
        (
            "a never-created infrastructure configuration".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:infrastructure-configuration/absent"),
        ),
        (
            "a never-created component build version".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/absent/1.0.0/1"),
        ),
        (
            "another build version of a component that exists".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:component/present-component/1.0.0/2"),
        ),
        (
            "a never-created image recipe".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image-recipe/absent/1.0.0"),
        ),
        (
            "an image pipeline, a kind this store holds none of".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:image-pipeline/absent"),
        ),
        (
            "a distribution configuration, a kind this store holds none of".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:distribution-configuration/absent"),
        ),
        (
            "a build workflow, a kind this store holds none of".to_string(),
            format!("arn:aws:imagebuilder:{REGION}:{ACCOUNT}:workflow/build/absent/1.0.0/1"),
        ),
        (
            "a configuration of another Region".to_string(),
            configuration.replace(REGION, OTHER_REGION),
        ),
    ];

    for (case, arn) in &cases {
        let (status, body) = list_tags(&router, arn).await;
        assert_refused(case, status, &body, 404, "NotFoundException");

        let (status, body) = tag_resource(&router, arn, &[("Owner", "platform")]).await;
        assert_refused(
            &format!("{case}, tagged"),
            status,
            &body,
            404,
            "ResourceNotFoundException",
        );
    }

    // The same ARNs, read from the Region that does hold their resources, answer.
    let (status, body) = list_tags(&router, &component).await;
    assert_ok("the component that exists", status, body);

    // And a configuration this Region holds is not reachable from the other Region's
    // endpoint either — the refusal above is about the Region, not about the ARN's text.
    let (status, body) = list_tags_in(&router, OTHER_REGION, &configuration).await;
    assert_refused(
        "a configuration read from another Region",
        status,
        &body,
        404,
        "NotFoundException",
    );
}

#[tokio::test]
async fn a_resource_carries_no_more_tags_than_the_modelled_maximum() {
    let router = emulator();
    let arn = create_configuration(&router, REGION, "Bounded Infra", None).await;

    // Fill the resource to the bound with two calls, since one call's `tags` map is itself
    // bounded by the same maximum.
    let keys: Vec<String> = (0..MAX_RESOURCE_TAGS).map(|n| format!("key-{n:02}")).collect();
    for chunk in keys.chunks(25) {
        let tags: Vec<(&str, &str)> = chunk.iter().map(|key| (key.as_str(), "set")).collect();
        let (status, body) = tag_resource(&router, &arn, &tags).await;
        assert_eq!(
            status, 200,
            "case filling to the bound: expected HTTP 200, got {status} with {body}"
        );
    }

    let (status, body) = list_tags(&router, &arn).await;
    let body = assert_ok("filled to the bound", status, body);
    let filled = reported_tags("filled to the bound", &body);
    assert_eq!(
        filled.len(),
        MAX_RESOURCE_TAGS,
        "case filled to the bound: expected {MAX_RESOURCE_TAGS} tags, got {}",
        filled.len()
    );

    // A key already present replaces its value, so the map does not grow past the bound.
    let (status, body) = tag_resource(&router, &arn, &[("key-00", "replaced")]).await;
    assert_eq!(
        status, 200,
        "case replacing a key at the bound: expected HTTP 200, got {status} with {body}"
    );

    // One key PAST the bound is refused, and the resource keeps the tags it had.
    let (status, body) = tag_resource(&router, &arn, &[("one-too-many", "set")]).await;
    assert_refused(
        "one key past the bound",
        status,
        &body,
        400,
        "InvalidParameterException",
    );

    let (status, body) = list_tags(&router, &arn).await;
    let body = assert_ok("after the refusal", status, body);
    let after = reported_tags("after the refusal", &body);
    let mut expected = filled;
    expected.insert("key-00".to_string(), "replaced".to_string());
    assert_eq!(
        after, expected,
        "case after the refusal: expected the tags to be unchanged at {expected:?}, got {after:?}"
    );
}
