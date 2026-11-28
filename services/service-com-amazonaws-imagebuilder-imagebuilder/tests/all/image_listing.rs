// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListImages`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handler — so what is asserted is what
//! a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! * the listing reports one entry per image VERSION, under the version ARN of that
//!   version's three assigned nodes and read from the greatest build version stored under
//!   it; a listing that asks for images by name instead reports one entry per NAME, under
//!   the name ARN whose version nodes are wildcards;
//! * `owner` says which inventory to report: the images this account built, the catalog AWS
//!   publishes, or — for an ownership this emulator holds no images of — an empty page, which
//!   an empty inventory answers as a 200 rather than as a refusal;
//! * `filters` selects on the five members the operation documents and refuses any other
//!   name;
//! * a page is bounded by `maxResults` and continued by a `nextToken` the listing ISSUED,
//!   and a token this listing did not issue carries no page position, so it is refused
//!   rather than read as "start from the beginning".
//!
//! The rows therefore differ along what those rules generalise over: how many build versions
//! a version holds, how many versions a name holds, the Region, the ownership asked for,
//! which member a filter names and how many values it carries, the page size, and — for the
//! token rule — how a token is wrong: fabricated, truncated, edited, or issued for another
//! listing of the same operation.
//!
//! The account's own build versions are filed through the store's own write path
//! (`ComAmazonawsImagebuilderImagebuilderState::insert_image`), which is where the image
//! build path files what a build produced.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_imagebuilder_imagebuilder::state::{
    image_build_version_arn, normalized_name, ComAmazonawsImagebuilderImagebuilderState,
    ImageBuildVersionRecord, MANAGED_IMAGES, MANAGED_IMAGE_OWNER, MANAGED_IMAGE_VERSION,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region a case signs for when it only needs one; the ARNs it builds carry it.
const REGION: &str = "us-west-2";

/// A second Region, so a listing is shown to report only the images of the Region whose
/// endpoint served it, and a token issued in one Region to be refused in the other.
const OTHER_REGION: &str = "eu-west-1";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN of an image this
/// account built carries.
const ACCOUNT: &str = "000000000000";

/// The version nodes a NAME ARN carries, from the note on `ImageVersion`'s `arn`:
/// "Versionless ARNs and Name ARNs do not include specific values in any of the nodes. The
/// nodes are either left off entirely, or they are specified as wildcards, for example:
/// x.x.x."
const WILDCARD_VERSION: &str = "x.x.x";

/// The smallest page size a request may ask for: the declared `maxResults` range's minimum
/// (`RestrictedInteger`, which this crate declares as `range 1..25` in
/// `smithy-model/local-additions.json`, see `smithy-model/LOCAL-ADDITIONS.md`).
const MIN_RESULTS_PER_PAGE: usize = 1;

/// The largest page size a request may ask for: that same range's maximum, the page size
/// Image Builder refuses anything above.
const MAX_RESULTS_PER_PAGE: usize = 25;

/// Why a token this listing did not issue is refused — the one sentence this service states
/// a refused parameter value in, about the member the token arrived in.
const INVALID_TOKEN_MESSAGE: &str = "The value supplied for parameter 'nextToken' is not \
                                     valid. The provided token does not meet the required \
                                     format.";

/// One emulator with an empty store, mounted exactly as the router mounts it, beside the
/// shared engine a build path would file its records in.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// A request signed for `imagebuilder` in `region` — the signal the router routes on and the
/// handler reads the Region from.
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

/// One filter a row asks for: the member it names and the values it carries.
type FilterSpec = Vec<(&'static str, Vec<&'static str>)>;

/// One build version a case starts from: its Region, name, version, build number, platform,
/// type and the instant its build recorded.
type Seed = (
    &'static str,
    &'static str,
    &'static str,
    u64,
    &'static str,
    &'static str,
    &'static str,
);

/// What one `ListImages` request asks for: the members a row of a table varies.
#[derive(Debug, Clone, Default)]
struct Query {
    owner: Option<&'static str>,
    by_name: bool,
    include_deprecated: Option<bool>,
    filters: FilterSpec,
    max_results: Option<usize>,
    next_token: Option<String>,
}

impl Query {
    /// The request body this query sends.
    fn body(&self) -> Value {
        let mut body = Map::new();
        if let Some(owner) = self.owner {
            body.insert("owner".to_string(), json!(owner));
        }
        if self.by_name {
            body.insert("byName".to_string(), json!(true));
        }
        if let Some(include_deprecated) = self.include_deprecated {
            body.insert("includeDeprecated".to_string(), json!(include_deprecated));
        }
        if !self.filters.is_empty() {
            let filters: Vec<Value> = self
                .filters
                .iter()
                .map(|(name, values)| json!({ "name": name, "values": values }))
                .collect();
            body.insert("filters".to_string(), json!(filters));
        }
        if let Some(max_results) = self.max_results {
            body.insert("maxResults".to_string(), json!(max_results));
        }
        if let Some(token) = &self.next_token {
            body.insert("nextToken".to_string(), json!(token));
        }
        Value::Object(body)
    }

    /// This query with `token` as its continuation token.
    fn continued_by(&self, token: &str) -> Self {
        Self {
            next_token: Some(token.to_string()),
            ..self.clone()
        }
    }
}

/// `ListImages` for `query`, in `region`.
async fn list_images_in(
    router: &MultiServiceRouter,
    region: &str,
    query: &Query,
) -> (u16, Value) {
    call_in(router, region, "POST", "/ListImages", Some(&query.body())).await
}

/// `GetImage` for `reference`, in `region`.
async fn get_image(router: &MultiServiceRouter, region: &str, reference: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "GET",
        &format!("/GetImage?imageBuildVersionArn={reference}"),
        None,
    )
    .await
}

/// One member of an answer, or a panic naming the case.
fn member<'body>(case: &str, body: &'body Value, name: &str) -> &'body Value {
    body.get(name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} member in {body}"))
}

/// One string member of an answer.
fn text(case: &str, body: &Value, name: &str) -> String {
    member(case, body, name)
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected {name} to be a string in {body}"))
        .to_string()
}

/// A successful answer's body, or a panic naming the case.
fn assert_ok(case: &str, status: u16, body: Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body}"
    );
    // Every answer of this service names the request it answers.
    assert!(
        body.get("requestId")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty()),
        "case {case:?}: expected a requestId in {body}"
    );
    body
}

/// The entries one page carries, in the order it carried them.
fn page_entries(case: &str, body: &Value) -> Vec<Value> {
    member(case, body, "imageVersionList")
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a list in {body}"))
        .clone()
}

/// The ARNs one page carries, in the order it carried them.
fn page_arns(case: &str, body: &Value) -> Vec<String> {
    page_entries(case, body)
        .iter()
        .map(|entry| text(case, entry, "arn"))
        .collect()
}

/// The continuation token one page answers with, when it answers one.
fn page_token(body: &Value) -> Option<String> {
    body.get("nextToken")?.as_str().map(str::to_string)
}

/// The error code a refusal carries, from `__type` (restJson1 renders the shape there, and
/// the `x-amzn-errortype` header is folded in beside it).
fn error_code(case: &str, body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an error type in {body}"))
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Assert one refusal's status and error code.
fn assert_refused(case: &str, status: u16, body: &Value, expect_status: u16, expect_code: &str) {
    assert_eq!(
        status, expect_status,
        "case {case:?}: expected HTTP {expect_status}, got {status} with {body}"
    );
    let code = error_code(case, body);
    assert_eq!(
        code, expect_code,
        "case {case:?}: expected {expect_code}, got {code:?} in {body}"
    );
    assert!(
        body.get("imageVersionList").is_none(),
        "case {case:?}: expected a refusal to carry no page, got {body}"
    );
}

/// A version ARN of the image `name` names at `version` in `region` — the ARN one entry of
/// this listing reports, which carries no build number.
fn version_arn(region: &str, name: &str, version: &str) -> String {
    format!(
        "arn:aws:imagebuilder:{region}:{ACCOUNT}:image/{}/{version}",
        normalized_name(name)
    )
}

/// A published image's version ARN in `region`: its owner field is Amazon's catalog rather
/// than an account id.
fn managed_version_arn(region: &str, name: &str, version: &str) -> String {
    format!(
        "arn:aws:imagebuilder:{region}:{MANAGED_IMAGE_OWNER}:image/{}/{version}",
        normalized_name(name)
    )
}

/// File one image build version in the store, the way the image build path does.
fn file_build_version(engine: &Arc<engine::InMemoryEngine>, seed: &Seed) {
    let (region, name, version, build, platform, image_type, date_created) = *seed;
    let arn = image_build_version_arn(region, name, version, build);
    let record = ImageBuildVersionRecord {
        arn: arn.clone(),
        region: region.to_string(),
        name: name.to_string(),
        version: version.to_string(),
        build_version: build,
        platform: Some(platform.to_string()),
        image_type: Some(image_type.to_string()),
        build_type: Some("USER_INITIATED".to_string()),
        date_created: date_created.to_string(),
        ..ImageBuildVersionRecord::default()
    };
    let filed = engine
        .state::<ComAmazonawsImagebuilderImagebuilderState>()
        .lock()
        .insert_image(record);
    assert!(
        filed,
        "case recording {arn}: expected the store to file the build version"
    );
}

/// The build versions every case of the account's own inventory starts from: two builds of
/// one version, a second version of that name, a third name, and one in another Region.
const SEEDED: &[Seed] = &[
    (
        REGION,
        "Alpha Image",
        "1.0.0",
        1,
        "Linux",
        "AMI",
        "2026-01-01T00:00:00Z",
    ),
    (
        REGION,
        "Alpha Image",
        "1.0.0",
        2,
        "Linux",
        "AMI",
        "2026-02-02T00:00:00Z",
    ),
    (
        REGION,
        "Alpha Image",
        "1.1.0",
        1,
        "Linux",
        "AMI",
        "2026-03-03T00:00:00Z",
    ),
    (
        REGION,
        "beta-container",
        "2.0.0",
        1,
        "Windows",
        "DOCKER",
        "2026-04-04T00:00:00Z",
    ),
    (
        OTHER_REGION,
        "gamma-image",
        "1.0.0",
        1,
        "Linux",
        "AMI",
        "2026-05-05T00:00:00Z",
    ),
];

/// An emulator holding [`SEEDED`].
fn seeded() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator();
    for seed in SEEDED {
        file_build_version(&engine, seed);
    }
    (router, engine)
}

/// RULE: an image listing reports one entry per stored image VERSION — under that version's
/// own ARN, read from the greatest build version stored under it — or one entry per NAME,
/// with every version of that name aggregated under the name ARN, and only for the Region
/// whose endpoint served the request.
#[tokio::test]
async fn a_listing_reports_one_entry_per_version_or_per_name() {
    let (router, _engine) = seeded();

    // (case, the Region asked, whether entries aggregate by name, the ARNs expected)
    let rows: Vec<(&str, &str, bool, Vec<String>)> = vec![
        (
            "one entry per version",
            REGION,
            false,
            vec![
                version_arn(REGION, "Alpha Image", "1.0.0"),
                version_arn(REGION, "Alpha Image", "1.1.0"),
                version_arn(REGION, "beta-container", "2.0.0"),
            ],
        ),
        (
            "one entry per name",
            REGION,
            true,
            vec![
                version_arn(REGION, "Alpha Image", WILDCARD_VERSION),
                version_arn(REGION, "beta-container", WILDCARD_VERSION),
            ],
        ),
        (
            "another Region's endpoint, one entry per version",
            OTHER_REGION,
            false,
            vec![version_arn(OTHER_REGION, "gamma-image", "1.0.0")],
        ),
        (
            "another Region's endpoint, one entry per name",
            OTHER_REGION,
            true,
            vec![version_arn(OTHER_REGION, "gamma-image", WILDCARD_VERSION)],
        ),
    ];

    for (case, region, by_name, expected) in &rows {
        let query = Query {
            by_name: *by_name,
            ..Query::default()
        };
        let (status, body) = list_images_in(&router, region, &query).await;
        let body = assert_ok(case, status, body);
        let arns = page_arns(case, &body);
        assert_eq!(
            &arns, expected,
            "case {case:?}: expected {expected:?}, got {arns:?}"
        );
        assert!(
            page_token(&body).is_none(),
            "case {case:?}: expected no nextToken for a listing one page exhausts, got {body}"
        );
    }

    // What one entry reports: the version it stands for, the members the build recorded, and
    // the account that owns it. The entry of the version holding two builds is read from the
    // GREATER build, which is the one a reference to that version resolves to.
    let case = "the entry of the version holding two builds";
    let (status, body) = list_images_in(&router, REGION, &Query::default()).await;
    let body = assert_ok(case, status, body);
    let entries = page_entries(case, &body);
    let entry = entries
        .iter()
        .find(|entry| text(case, entry, "version") == "1.0.0")
        .unwrap_or_else(|| panic!("case {case:?}: expected the 1.0.0 entry in {entries:?}"));
    for (name, expected) in [
        ("arn", version_arn(REGION, "Alpha Image", "1.0.0")),
        ("name", "Alpha Image".to_string()),
        ("version", "1.0.0".to_string()),
        ("platform", "Linux".to_string()),
        ("type", "AMI".to_string()),
        ("owner", ACCOUNT.to_string()),
        ("buildType", "USER_INITIATED".to_string()),
        // The greater build of that version, which is the build the entry is read from.
        ("dateCreated", "2026-02-02T00:00:00Z".to_string()),
    ] {
        let actual = text(case, entry, name);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {name} {expected:?}, got {actual:?}"
        );
    }
    // Nothing this store holds says which operating system an account's own image carries,
    // so an entry of it reports none rather than naming one nothing observed.
    assert!(
        entry.get("osVersion").is_none_or(Value::is_null),
        "case {case:?}: expected no osVersion in {entry}"
    );

    // ROUND TRIP: the version ARN one entry reports reaches that image through this
    // service's own read, which resolves it to the build the entry was read from.
    let listed = text(case, entry, "arn");
    let (status, read) = get_image(&router, REGION, &listed).await;
    let read = assert_ok("reading the listed version ARN back", status, read);
    let image = member("reading the listed version ARN back", &read, "image").clone();
    let read_version = text("reading the listed version ARN back", &image, "version");
    assert_eq!(
        read_version, "1.0.0/2",
        "case reading the listed version ARN back: expected the greater build, got \
         {read_version:?}"
    );
}

/// RULE: a listing selects on the members its `filters` documents — `name`, `osVersion`,
/// `platform`, `type` and `version`, case-sensitively — and a filter naming any other member
/// is refused rather than ignored; a selection nothing matches is an empty page, not a
/// refusal.
#[tokio::test]
async fn a_listing_selects_on_the_filters_the_operation_documents() {
    let (router, _engine) = seeded();

    // (case, the filters, the ARNs expected)
    let rows: Vec<(&str, FilterSpec, Vec<String>)> = vec![
        (
            "one name",
            vec![("name", vec!["Alpha Image"])],
            vec![
                version_arn(REGION, "Alpha Image", "1.0.0"),
                version_arn(REGION, "Alpha Image", "1.1.0"),
            ],
        ),
        (
            "two values of one name filter",
            vec![("name", vec!["Alpha Image", "beta-container"])],
            vec![
                version_arn(REGION, "Alpha Image", "1.0.0"),
                version_arn(REGION, "Alpha Image", "1.1.0"),
                version_arn(REGION, "beta-container", "2.0.0"),
            ],
        ),
        (
            "a platform",
            vec![("platform", vec!["Windows"])],
            vec![version_arn(REGION, "beta-container", "2.0.0")],
        ),
        (
            "a type",
            vec![("type", vec!["DOCKER"])],
            vec![version_arn(REGION, "beta-container", "2.0.0")],
        ),
        (
            "a version",
            vec![("version", vec!["1.1.0"])],
            vec![version_arn(REGION, "Alpha Image", "1.1.0")],
        ),
        (
            "two filters together",
            vec![
                ("name", vec!["Alpha Image"]),
                ("version", vec!["1.0.0"]),
            ],
            vec![version_arn(REGION, "Alpha Image", "1.0.0")],
        ),
        (
            "a name this account never created",
            vec![("name", vec!["no-image-of-this-name"])],
            Vec::new(),
        ),
        (
            // "Filter values are case-sensitive": a value differing only in case selects
            // nothing.
            "a name differing only in case",
            vec![("name", vec!["ALPHA IMAGE"])],
            Vec::new(),
        ),
        (
            // An image an account built reports no `osVersion`, so no value of that filter
            // selects one.
            "an osVersion of an account's own image",
            vec![("osVersion", vec!["Amazon Linux 2"])],
            Vec::new(),
        ),
    ];

    for (case, filters, expected) in &rows {
        let query = Query {
            filters: filters.clone(),
            ..Query::default()
        };
        let (status, body) = list_images_in(&router, REGION, &query).await;
        let body = assert_ok(case, status, body);
        let arns = page_arns(case, &body);
        assert_eq!(
            &arns, expected,
            "case {case:?}: expected {expected:?}, got {arns:?}"
        );
    }

    // A filter this listing does not filter on, a filter that left out a member every filter
    // has to carry and a values list the model does not admit are each refused rather than
    // ignored.
    //
    // (case, the request body, the class it is refused with)
    let refusals: Vec<(&str, Value, &str)> = vec![
        (
            "a filter naming a member this listing does not filter on",
            json!({ "filters": [{ "name": "owner", "values": ["Self"] }] }),
            "InvalidParameterValueException",
        ),
        (
            // Neither member of a `Filter` is one the model can require, so a filter that
            // left one out is reported as the parameter the request omitted.
            "a filter naming no member",
            json!({ "filters": [{ "values": ["Alpha Image"] }] }),
            "MissingParametersException",
        ),
        (
            "a filter carrying no values",
            json!({ "filters": [{ "name": "name" }] }),
            "MissingParametersException",
        ),
        (
            // The modelled `values` list carries at least one value, which the centralized
            // walk refuses past before the handler runs.
            "a filter carrying an empty values list",
            json!({ "filters": [{ "name": "name", "values": [] }] }),
            "InvalidParameterValueException",
        ),
        (
            // "You can't combine this option with the version filter" (`byName`).
            "byName together with the version filter",
            json!({ "byName": true, "filters": [{ "name": "version", "values": ["1.0.0"] }] }),
            "InvalidRequestException",
        ),
    ];
    for (case, body, expected) in &refusals {
        let (status, refused) = call_in(&router, REGION, "POST", "/ListImages", Some(body)).await;
        assert_refused(case, status, &refused, 400, expected);
    }
}

/// RULE: a `nextToken` is opaque — the only value the parameter admits is one a previous page
/// of THAT listing handed back — so a token this listing did not issue carries no page
/// position and is refused with HTTP 400 `InvalidParameterValueException`, naming the member
/// it arrived in, rather than being read as "start from the beginning".
///
/// The rows differ along how a token is wrong: fabricated, truncated, edited, and issued for
/// another listing of this same operation (another Region's endpoint, another ownership,
/// another set of filters, and the by-name listing of the same images).
#[tokio::test]
async fn a_page_token_belongs_to_the_listing_that_issued_it() {
    let (router, _engine) = seeded();

    let walked = Query {
        max_results: Some(MIN_RESULTS_PER_PAGE),
        ..Query::default()
    };
    let (status, first) = list_images_in(&router, REGION, &walked).await;
    let first = assert_ok("the first page", status, first);
    let issued = page_token(&first)
        .unwrap_or_else(|| panic!("case the first page: expected a nextToken in {first}"));

    let truncated: String = issued.chars().take(issued.chars().count() - 1).collect();
    let edited = format!("{issued}00");
    // (case, the token, the Region asked, the query it is spent on)
    let refusals: Vec<(&str, String, &str, Query)> = vec![
        (
            "a token this service never minted",
            "not-a-real-pagination-token==".to_string(),
            REGION,
            walked.clone(),
        ),
        (
            "a token that is not base64 at all",
            "@@@".to_string(),
            REGION,
            walked.clone(),
        ),
        (
            "an issued token with its last character dropped",
            truncated,
            REGION,
            walked.clone(),
        ),
        (
            "an issued token with characters appended",
            edited,
            REGION,
            walked.clone(),
        ),
        (
            "an issued token spent in another Region",
            issued.clone(),
            OTHER_REGION,
            walked.clone(),
        ),
        (
            "an issued token spent on the by-name listing",
            issued.clone(),
            REGION,
            Query {
                by_name: true,
                ..walked.clone()
            },
        ),
        (
            "an issued token spent on another ownership",
            issued.clone(),
            REGION,
            Query {
                owner: Some("Amazon"),
                ..walked.clone()
            },
        ),
        (
            "an issued token spent on another set of filters",
            issued.clone(),
            REGION,
            Query {
                filters: vec![("name", vec!["Alpha Image"])],
                ..walked.clone()
            },
        ),
    ];
    for (case, token, region, query) in &refusals {
        let (status, body) = list_images_in(&router, region, &query.continued_by(token)).await;
        assert_refused(case, status, &body, 400, "InvalidParameterValueException");
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message, INVALID_TOKEN_MESSAGE,
            "case {case:?}: expected {INVALID_TOKEN_MESSAGE:?}, got {message:?}"
        );
    }

    // The token it DID issue continues that listing, whatever page size the next request
    // asks for: the page size says how much of the listing to answer with, not which listing
    // it is.
    let expected = vec![
        version_arn(REGION, "Alpha Image", "1.0.0"),
        version_arn(REGION, "Alpha Image", "1.1.0"),
        version_arn(REGION, "beta-container", "2.0.0"),
    ];
    for page_size in [MIN_RESULTS_PER_PAGE, MAX_RESULTS_PER_PAGE] {
        let case = format!("continuing at {page_size} per page");
        let query = Query {
            max_results: Some(page_size),
            ..Query::default()
        };
        let (status, body) = list_images_in(&router, REGION, &query.continued_by(&issued)).await;
        let body = assert_ok(&case, status, body);
        let arns = page_arns(&case, &body);
        let want: Vec<String> = expected.iter().skip(1).take(page_size).cloned().collect();
        assert_eq!(
            arns, want,
            "case {case:?}: expected {want:?}, got {arns:?}"
        );
    }

    // Walking the listing one entry at a time, each page continued by the token the previous
    // one answered, reports every entry exactly once and ends without a token.
    let mut walk: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for step in 0..=expected.len() {
        let case = format!("the walk's page {step}");
        let query = match &token {
            Some(token) => walked.continued_by(token),
            None => walked.clone(),
        };
        let (status, body) = list_images_in(&router, REGION, &query).await;
        let body = assert_ok(&case, status, body);
        walk.extend(page_arns(&case, &body));
        token = page_token(&body);
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case the walk: expected the last page to answer no token, got {token:?}"
    );
    assert_eq!(
        walk, expected,
        "case the walk: expected {expected:?}, got {walk:?}"
    );
}

/// RULE: `owner` says which inventory the listing reports — the images this account built,
/// the catalog AWS publishes, or, for an ownership this emulator holds no images of, an empty
/// page — and the catalog is reported exactly as a read of one of its images reports it.
#[tokio::test]
async fn which_inventory_an_ownership_reports() {
    let (router, _engine) = seeded();

    let catalog: Vec<String> = {
        let mut arns: Vec<String> = MANAGED_IMAGES
            .iter()
            .map(|image| managed_version_arn(REGION, image.name, MANAGED_IMAGE_VERSION))
            .collect();
        arns.sort();
        arns
    };
    let catalog_by_name: Vec<String> = {
        let mut arns: Vec<String> = MANAGED_IMAGES
            .iter()
            .map(|image| managed_version_arn(REGION, image.name, WILDCARD_VERSION))
            .collect();
        arns.sort();
        arns
    };
    let own: Vec<String> = vec![
        version_arn(REGION, "Alpha Image", "1.0.0"),
        version_arn(REGION, "Alpha Image", "1.1.0"),
        version_arn(REGION, "beta-container", "2.0.0"),
    ];

    // (case, the ownership asked for, whether entries aggregate by name, the ARNs expected)
    let rows: Vec<(&str, Option<&'static str>, bool, Vec<String>)> = vec![
        ("no ownership named", None, false, own.clone()),
        ("this account's own images", Some("Self"), false, own.clone()),
        ("Amazon's published catalog", Some("Amazon"), false, catalog.clone()),
        (
            "Amazon's published catalog, by name",
            Some("Amazon"),
            true,
            catalog_by_name,
        ),
        ("images shared with this account", Some("Shared"), false, Vec::new()),
        ("third-party images", Some("ThirdParty"), false, Vec::new()),
        (
            "Amazon Web Services Marketplace images",
            Some("AWSMarketplace"),
            false,
            Vec::new(),
        ),
    ];

    for (case, owner, by_name, expected) in &rows {
        let query = Query {
            owner: *owner,
            by_name: *by_name,
            max_results: Some(MAX_RESULTS_PER_PAGE),
            ..Query::default()
        };
        let (status, body) = list_images_in(&router, REGION, &query).await;
        let body = assert_ok(case, status, body);
        let arns = page_arns(case, &body);
        assert_eq!(
            &arns, expected,
            "case {case:?}: expected {expected:?}, got {arns:?}"
        );
    }

    // What one catalog entry reports, on an entry no issue of this listing cited: the
    // operating system the catalog records, the platform its name states, the version AWS
    // published it at and Amazon as its owner.
    let entry = MANAGED_IMAGES
        .last()
        .expect("the catalog holds at least one published image");
    let case = format!("the published image {:?}", entry.name);
    let platform = if entry.name.contains("Windows") {
        "Windows"
    } else {
        "Linux"
    };
    let query = Query {
        owner: Some("Amazon"),
        filters: vec![("osVersion", vec![entry.os_version])],
        max_results: Some(MAX_RESULTS_PER_PAGE),
        ..Query::default()
    };
    let (status, body) = list_images_in(&router, REGION, &query).await;
    let body = assert_ok(&case, status, body);
    let entries = page_entries(&case, &body);
    let reported = entries
        .iter()
        .find(|reported| text(&case, reported, "name") == entry.name)
        .unwrap_or_else(|| panic!("case {case:?}: expected the entry in {entries:?}"));
    for (name, expected) in [
        (
            "arn",
            managed_version_arn(REGION, entry.name, MANAGED_IMAGE_VERSION),
        ),
        ("name", entry.name.to_string()),
        ("version", MANAGED_IMAGE_VERSION.to_string()),
        ("osVersion", entry.os_version.to_string()),
        ("platform", platform.to_string()),
        ("type", "AMI".to_string()),
        ("owner", "Amazon".to_string()),
        ("imageSource", "AMAZON_MANAGED".to_string()),
    ] {
        let actual = text(&case, reported, name);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {name} {expected:?}, got {actual:?}"
        );
    }

    // ROUND TRIP: the ARN a catalog entry reports reaches that published image through this
    // service's own read.
    let listed = text(&case, reported, "arn");
    let (status, read) = get_image(&router, REGION, &listed).await;
    let read = assert_ok("reading a listed catalog ARN back", status, read);
    let image = member("reading a listed catalog ARN back", &read, "image").clone();
    let read_name = text("reading a listed catalog ARN back", &image, "name");
    assert_eq!(
        read_name, entry.name,
        "case reading a listed catalog ARN back: expected {:?}, got {read_name:?}",
        entry.name
    );

    // "Specifies whether to include deprecated Amazon-managed images in the results":
    // no image this service reports carries a deprecation time, so either value of that
    // member reports the same catalog, and a token stays spendable across it.
    for include_deprecated in [false, true] {
        let case = format!("includeDeprecated {include_deprecated}");
        let query = Query {
            owner: Some("Amazon"),
            include_deprecated: Some(include_deprecated),
            max_results: Some(MAX_RESULTS_PER_PAGE),
            ..Query::default()
        };
        let (status, body) = list_images_in(&router, REGION, &query).await;
        let body = assert_ok(&case, status, body);
        let arns = page_arns(&case, &body);
        assert_eq!(
            arns, catalog,
            "case {case:?}: expected {catalog:?}, got {arns:?}"
        );
    }

    // The catalog pages like every other listing: a page bounded below its size answers a
    // token, and walking it reports each published image exactly once.
    let paged = Query {
        owner: Some("Amazon"),
        max_results: Some(3),
        ..Query::default()
    };
    let mut walk: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for step in 0..=catalog.len() {
        let case = format!("the catalog walk's page {step}");
        let query = match &token {
            Some(token) => paged.continued_by(token),
            None => paged.clone(),
        };
        let (status, body) = list_images_in(&router, REGION, &query).await;
        let body = assert_ok(&case, status, body);
        let page = page_arns(&case, &body);
        assert!(
            page.len() <= 3,
            "case {case:?}: expected at most 3 entries, got {page:?}"
        );
        walk.extend(page);
        token = page_token(&body);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walk, catalog,
        "case the catalog walk: expected {catalog:?}, got {walk:?}"
    );
}
