// SPDX-License-Identifier: CC0-1.0

//! `GetParameterHistory` end to end: `PutParameter` writes versions of a parameter and
//! the history read reports them.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol"). Nothing reaches into the
//! state struct, so a response that is not computed from persisted state cannot pass:
//! every history read follows the puts that created the versions it reports.
//!
//! The suites are tables. Their rows differ along what the rules under test generalise
//! over — how many versions a parameter has, the parameter's type and name form, which
//! members each put carried, the page size, the token, and the signed Region — rather
//! than the single parameter the recorded unit happened to read.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request signs for unless a case says otherwise. Deliberately not
/// the emulator's default, so a history read served under it proves the Region came
/// from the request's SigV4 credential scope.
const REGION: &str = "us-west-2";

/// A second Region, to prove one Region's versions are not another's: "A parameter name
/// must be unique within an Amazon Web Services Region"
/// (`smithy_query(query="doc", shape="PutParameterRequest", member="Name")`).
const OTHER_REGION: &str = "eu-central-1";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "AmazonSSM";

/// The published bounds of `GetParameterHistoryRequest$MaxResults`
/// (`smithy_query(query="shape", shape="GetParameterHistoryRequest")`: range 1..50).
const MIN_PAGE_SIZE: i64 = 1;

/// See [`MIN_PAGE_SIZE`].
const MAX_PAGE_SIZE: i64 = 50;

/// The key alias a `SecureString` is encrypted under when the put names none ("the
/// system uses the default key associated with your Amazon Web Services account",
/// `smithy_query(query="doc", shape="PutParameterRequest", member="KeyId")`).
const DEFAULT_KEY_ALIAS: &str = "alias/aws/ssm";

/// A router with just this service mounted on a fresh engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the router still reads the SigV4
    // credential scope below, which is where a stored parameter's Region comes from.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        engine,
    ));
    router
}

/// Call one operation signed for `region`, returning `(status, body)`.
async fn call_in_region(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{region}/ssm/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // The error shape travels in `__type`; fold in the header the framework also sets
    // so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Call one operation signed for [`REGION`].
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_in_region(router, REGION, operation, body).await
}

/// The wire error code of a failure body (`__type`, without its namespace).
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .split('#')
        .next_back()
        .unwrap_or(rendered)
        .to_string()
}

/// `PutParameter` signed for `region`, asserting it succeeded and returning the version
/// it reports.
async fn put_ok_in_region(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    body: &Value,
) -> i64 {
    let (status, response) = call_in_region(router, region, "PutParameter", body).await;
    assert_eq!(
        status, 200,
        "case {case}: PutParameter {body} must succeed, got {status} {response}"
    );
    response["Version"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case}: PutParameter reports the version it created"))
}

/// Write `values` as successive versions of `name` in `region`, overwriting after the
/// first, and answer the version numbers the puts reported.
async fn seed_versions_in_region(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    type_: &str,
    values: &[&str],
) -> Vec<i64> {
    let mut versions = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let mut body = json!({ "Name": name, "Type": type_, "Value": value });
        if index > 0 {
            body["Overwrite"] = json!(true);
        }
        versions.push(put_ok_in_region(router, region, name, &body).await);
    }
    versions
}

/// See [`seed_versions_in_region`]; signed for [`REGION`].
async fn seed_versions(
    router: &MultiServiceRouter,
    name: &str,
    type_: &str,
    values: &[&str],
) -> Vec<i64> {
    seed_versions_in_region(router, REGION, name, type_, values).await
}

/// The `Name` a read reports for the parameter whose put named `name`: a top-level
/// parameter's name carries no leading separator, while a hierarchical name is its whole
/// path (`PutParameterRequest$Name`: "For parameters in a hierarchy, you must include a
/// leading forward slash character (/)"), which is the rendering the parameter's ARN
/// carries too.
fn reported_name(name: &str) -> &str {
    let canonical = name.strip_prefix('/').unwrap_or(name);
    if canonical.contains('/') {
        name
    } else {
        canonical
    }
}

/// `GetParameterHistory` signed for `region`.
async fn history_in_region(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call_in_region(router, region, "GetParameterHistory", body).await
}

/// `GetParameterHistory` signed for [`REGION`], asserting it succeeded.
async fn history_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = history_in_region(router, REGION, body).await;
    assert_eq!(
        status, 200,
        "case {case}: GetParameterHistory {body} must succeed, got {status} {response}"
    );
    response
}

/// The entries a history read reported, in the order it reported them.
fn entries(case: &str, response: &Value) -> Vec<Value> {
    response["Parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: the response carries a Parameters list: {response}"))
        .clone()
}

/// The `Version` of every reported entry, in order.
fn reported_versions(case: &str, response: &Value) -> Vec<i64> {
    entries(case, response)
        .iter()
        .map(|entry| {
            entry["Version"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {case}: every entry carries a Version: {entry}"))
        })
        .collect()
}

/// The `Value` of every reported entry, in order.
fn reported_values(case: &str, response: &Value) -> Vec<String> {
    entries(case, response)
        .iter()
        .map(|entry| {
            entry["Value"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: every entry carries a Value: {entry}"))
                .to_string()
        })
        .collect()
}

/// A history read reports every version the store retains for the named parameter,
/// oldest first, whatever the parameter's type, name form or number of versions.
#[tokio::test]
async fn a_history_read_reports_every_version_a_put_created_oldest_first() {
    let router = router();

    // (case, name, ParameterType, the values successive puts wrote)
    let cases: &[(&str, &str, &str, &[&str])] = &[
        ("one version", "/hist/one", "String", &["only"]),
        ("two versions", "/hist/two", "String", &["first", "second"]),
        (
            "five versions",
            "/hist/deep/levels/five",
            "String",
            &["v1", "v2", "v3", "v4", "v5"],
        ),
        ("a top-level name", "TopLevel", "String", &["a", "b"]),
        ("a rooted top-level name", "/Rooted", "String", &["a", "b"]),
        (
            "a list parameter",
            "/hist/list",
            "StringList",
            &["a,b", "c,d", "e,f"],
        ),
        (
            "an encrypted parameter",
            "/hist/secure",
            "SecureString",
            &["s3cr3t", "r0tated"],
        ),
        (
            "a value that repeats an earlier one",
            "/hist/repeat",
            "String",
            &["same", "other", "same"],
        ),
    ];

    for (case, name, type_, values) in cases {
        let created = seed_versions(&router, name, type_, values).await;
        // A `SecureString` version is stored as the ciphertext its put sealed, so a read
        // that compares the reported values with what the puts wrote asks for them
        // decrypted; the flag "is ignored for String and StringList parameter types".
        let response =
            history_ok(&router, case, &json!({ "Name": name, "WithDecryption": true })).await;

        assert_eq!(
            reported_versions(case, &response),
            created,
            "case {case}: the history reports the versions the puts created, oldest first, \
             got {response}"
        );
        assert_eq!(
            reported_values(case, &response),
            values.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
            "case {case}: each entry reports the value its own put wrote, got {response}"
        );
        assert!(
            response.get("NextToken").is_none(),
            "case {case}: a history that fits in one page carries no NextToken, got {response}"
        );

        let mut last_modified = f64::MIN;
        for entry in entries(case, &response) {
            assert_eq!(
                entry["Name"].as_str(),
                Some(reported_name(name)),
                "case {case}: every entry names the parameter as a read reports it, got {entry}"
            );
            assert_eq!(
                entry["Type"].as_str(),
                Some(*type_),
                "case {case}: every entry reports the stored type, got {entry}"
            );
            // "The alias of the KMS key used to encrypt the parameter. Applies to
            // SecureString parameters only" (`ParameterHistory$KeyId`).
            let key_id = entry["KeyId"].as_str();
            let expected_key = (*type_ == "SecureString").then_some(DEFAULT_KEY_ALIAS);
            assert_eq!(
                key_id, expected_key,
                "case {case}: only an encrypted parameter reports a KeyId, got {entry}"
            );
            let modified = entry["LastModifiedDate"].as_f64().unwrap_or_else(|| {
                panic!("case {case}: every entry carries a LastModifiedDate: {entry}")
            });
            assert!(
                modified >= last_modified,
                "case {case}: LastModifiedDate never goes backwards along the history, got \
                 {modified} after {last_modified} in {entry}"
            );
            last_modified = modified;
            // A version nothing labelled carries an empty label list rather than none:
            // the member is the version's label set, which it always has.
            assert_eq!(
                entry["Labels"].as_array().map(Vec::len),
                Some(0),
                "case {case}: a version nobody labelled reports an empty Labels list, got \
                 {entry}"
            );
        }

        // Round trip through the implemented single-parameter read: the newest entry of
        // the history is the version that read answers with. Both reads ask for the value
        // decrypted, so an encrypted parameter is compared at its plaintext.
        let (status, latest) = call(
            &router,
            "GetParameter",
            &json!({ "Name": name, "WithDecryption": true }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: GetParameter must succeed, got {status} {latest}"
        );
        assert_eq!(
            latest["Parameter"]["Version"].as_i64(),
            created.last().copied(),
            "case {case}: the newest version of the history is the one a plain read reports, \
             got {latest}"
        );
        assert_eq!(
            latest["Parameter"]["Value"].as_str(),
            values.last().copied(),
            "case {case}: the newest value of the history is the one a plain read reports, \
             got {latest}"
        );
    }
}

/// Each entry reports the members ITS OWN put carried, not the newest version's: a
/// description, an allowed pattern, a data type, a tier and a policy document belong to
/// the version they were written with.
#[tokio::test]
async fn each_entry_reports_the_members_its_own_put_carried() {
    let router = router();
    let name = "/hist/members";

    // An `Expiration` policy document, which only an advanced parameter may carry
    // ("Standard parameters ... can't be configured to use parameter policies",
    // `smithy_query(query="doc", shape="PutParameterRequest", member="Tier")`).
    let expiration = json!([{
        "Type": "Expiration",
        "Version": "1.0",
        "Attributes": { "Timestamp": "2035-01-01T00:00:00.000Z" }
    }]);

    // (case, the put body, the members the entry must report)
    struct Expected<'a> {
        description: Option<&'a str>,
        allowed_pattern: Option<&'a str>,
        data_type: &'a str,
        tier: &'a str,
        policy_types: Vec<&'a str>,
    }

    let puts: Vec<(&str, Value, Expected<'_>)> = vec![
        (
            "a described, pattern-checked first version",
            json!({
                "Name": name,
                "Type": "String",
                "Value": "10",
                "Description": "the first value",
                "AllowedPattern": "^[0-9]+$"
            }),
            Expected {
                description: Some("the first value"),
                allowed_pattern: Some("^[0-9]+$"),
                data_type: "text",
                tier: "Standard",
                policy_types: vec![],
            },
        ),
        (
            "a bare second version",
            json!({ "Name": name, "Value": "20", "Overwrite": true }),
            Expected {
                description: None,
                allowed_pattern: None,
                data_type: "text",
                tier: "Standard",
                policy_types: vec![],
            },
        ),
        (
            "a version carrying a policy document",
            json!({
                "Name": name,
                "Value": "30",
                "Overwrite": true,
                "Tier": "Advanced",
                "Description": "with a policy",
                "Policies": expiration.to_string()
            }),
            Expected {
                description: Some("with a policy"),
                allowed_pattern: None,
                data_type: "text",
                tier: "Advanced",
                policy_types: vec!["Expiration"],
            },
        ),
        (
            "a version naming another data type",
            json!({
                "Name": name,
                "Value": "ami-0abcdef1234567890",
                "Overwrite": true,
                "DataType": "aws:ec2:image"
            }),
            Expected {
                description: None,
                allowed_pattern: None,
                data_type: "aws:ec2:image",
                // An advanced parameter can't be reverted, so a put naming no tier
                // stays in the one the parameter has.
                tier: "Advanced",
                policy_types: vec![],
            },
        ),
    ];

    for (case, body, _) in &puts {
        put_ok_in_region(&router, REGION, case, body).await;
    }

    let response = history_ok(&router, "members", &json!({ "Name": name })).await;
    let reported = entries("members", &response);
    assert_eq!(
        reported.len(),
        puts.len(),
        "the history reports one entry per put, got {response}"
    );

    for ((case, _, expected), entry) in puts.iter().zip(reported.iter()) {
        assert_eq!(
            entry["Description"].as_str(),
            expected.description,
            "case {case}: the entry reports its own Description, got {entry}"
        );
        assert_eq!(
            entry["AllowedPattern"].as_str(),
            expected.allowed_pattern,
            "case {case}: the entry reports its own AllowedPattern, got {entry}"
        );
        assert_eq!(
            entry["DataType"].as_str(),
            Some(expected.data_type),
            "case {case}: the entry reports its own DataType, got {entry}"
        );
        assert_eq!(
            entry["Tier"].as_str(),
            Some(expected.tier),
            "case {case}: the entry reports its own Tier, got {entry}"
        );
        let policies = entry["Policies"].as_array().cloned().unwrap_or_default();
        let types: Vec<&str> = policies
            .iter()
            .map(|policy| policy["PolicyType"].as_str().unwrap_or("<none>"))
            .collect();
        assert_eq!(
            types, expected.policy_types,
            "case {case}: the entry reports the policies its own put carried, got {entry}"
        );
        for policy in &policies {
            // "Pending (the policy hasn't been enforced or applied yet)"
            // (`smithy_query(query="doc", shape="ParameterInlinePolicy",
            // member="PolicyStatus")`): nothing here enforces a stored policy.
            assert_eq!(
                policy["PolicyStatus"].as_str(),
                Some("Pending"),
                "case {case}: a stored policy nothing applied is pending, got {policy}"
            );
            let text = policy["PolicyText"].as_str().unwrap_or_default();
            let parsed: Value = serde_json::from_str(text)
                .unwrap_or_else(|_| panic!("case {case}: PolicyText is JSON text: {policy}"));
            assert_eq!(
                parsed["Type"],
                expiration[0]["Type"],
                "case {case}: PolicyText is the JSON text of the stored policy, got {policy}"
            );
        }
    }
}

/// A page carries at most `MaxResults` entries and the token resumes the walk, so paging
/// through a history visits every version exactly once, in order, whatever the page size.
#[tokio::test]
async fn paging_walks_the_whole_history_exactly_once() {
    let router = router();
    let name = "/hist/paged";
    let values: Vec<String> = (1..=7).map(|version| format!("v{version}")).collect();
    let as_refs: Vec<&str> = values.iter().map(String::as_str).collect();
    let created = seed_versions(&router, name, "String", &as_refs).await;

    // Page sizes spanning the published range: its lower bound, sizes that divide the
    // history and sizes that do not, and its upper bound (one page for the lot).
    for page_size in [MIN_PAGE_SIZE, 2, 3, 6, 7, MAX_PAGE_SIZE] {
        let case = format!("page size {page_size}");
        let mut seen_versions = Vec::new();
        let mut seen_values = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "Name": name, "MaxResults": page_size });
            if let Some(token) = &token {
                body["NextToken"] = json!(token);
            }
            let response = history_ok(&router, &case, &body).await;
            let page = entries(&case, &response);
            assert!(
                page.len() as i64 <= page_size,
                "case {case}: a page carries at most MaxResults entries, got {} in {response}",
                page.len()
            );
            seen_versions.extend(reported_versions(&case, &response));
            seen_values.extend(reported_values(&case, &response));
            pages += 1;
            assert!(
                pages <= created.len() + 1,
                "case {case}: paging must terminate, got {pages} pages"
            );
            match response["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen_versions, created,
            "case {case}: paging visits every version exactly once, oldest first"
        );
        assert_eq!(
            seen_values, values,
            "case {case}: paging visits every value exactly once, oldest first"
        );
        let expected_pages = created.len().div_ceil(page_size as usize);
        assert_eq!(
            pages,
            expected_pages.max(1),
            "case {case}: {} versions at {page_size} per page is {expected_pages} pages",
            created.len()
        );
    }
}

/// A `MaxResults` outside the published range is refused, and a `NextToken` this service
/// did not issue is the modelled token error.
#[tokio::test]
async fn a_page_this_service_cannot_serve_is_refused() {
    let router = router();
    let name = "/hist/refusals";
    seed_versions(&router, name, "String", &["a", "b", "c"]).await;

    // Each published bound is served, and one past it is refused.
    for (case, page_size, expected) in [
        ("the lower bound", MIN_PAGE_SIZE, None),
        ("the upper bound", MAX_PAGE_SIZE, None),
        ("one below the lower bound", MIN_PAGE_SIZE - 1, Some(400)),
        ("one past the upper bound", MAX_PAGE_SIZE + 1, Some(400)),
    ] {
        let (status, response) = history_in_region(
            &router,
            REGION,
            &json!({ "Name": name, "MaxResults": page_size }),
        )
        .await;
        match expected {
            None => assert_eq!(
                status, 200,
                "case {case}: MaxResults {page_size} is inside the published range, got {status} \
                 {response}"
            ),
            Some(code) => {
                assert_eq!(
                    status, code,
                    "case {case}: MaxResults {page_size} is outside the published range, got \
                     {status} {response}"
                );
                assert_eq!(
                    error_code(&response),
                    "ValidationException",
                    "case {case}: a MaxResults outside the range is a validation failure, got \
                     {response}"
                );
            }
        }
    }

    // A token is this read's own, so anything it did not mint names no page of this
    // history: a value shaped like a version number is no more a token than prose is.
    for (case, token) in [
        ("prose", "not-a-version"),
        ("version zero", "0"),
        ("a negative version", "-3"),
        ("a name", name),
        ("a fraction", "1.5"),
        ("a version this history holds", "2"),
        ("the newest version in the clear", "3"),
    ] {
        let (status, response) =
            history_in_region(&router, REGION, &json!({ "Name": name, "NextToken": token })).await;
        assert_eq!(
            status, 400,
            "case {case}: the token {token} was never issued here, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidNextToken",
            "case {case}: an unissued token is the modelled token error, got {response}"
        );
    }

    // The token this read DID hand out resumes the walk where the page stopped.
    let first = history_ok(
        &router,
        "the first page of two",
        &json!({ "Name": name, "MaxResults": 2 }),
    )
    .await;
    let issued = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a page that left a version over hands out a token: {first}"))
        .to_string();
    let response = history_ok(
        &router,
        "the token the first page handed out",
        &json!({ "Name": name, "NextToken": issued }),
    )
    .await;
    assert_eq!(
        reported_versions("the token the first page handed out", &response),
        vec![3],
        "the token the first page handed out serves the version it stopped at, got {response}"
    );
}

/// A history read of a parameter this Region and account hold none of is the modelled
/// miss, and a `Name` the service could not have stored is refused.
#[tokio::test]
async fn a_history_read_of_a_parameter_nobody_created_is_the_modelled_miss() {
    let router = router();
    let stored = "/hist/present";
    seed_versions(&router, stored, "String", &["a", "b"]).await;
    seed_versions_in_region(&router, OTHER_REGION, "/hist/elsewhere", "String", &["x"]).await;

    // (case, the Name to read, the error code it answers with)
    let cases: &[(&str, String, &str)] = &[
        (
            "a name nobody created",
            "/hist/absent".to_string(),
            "ParameterNotFound",
        ),
        (
            "a parameter of another Region",
            "/hist/elsewhere".to_string(),
            "ParameterNotFound",
        ),
        (
            "an ARN of another Region",
            format!("arn:aws:ssm:{OTHER_REGION}:{ACCOUNT}:parameter{stored}"),
            "ParameterNotFound",
        ),
        (
            "an ARN of another account",
            format!("arn:aws:ssm:{REGION}:210987654321:parameter{stored}"),
            "ParameterNotFound",
        ),
        (
            "an ARN of another service",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:bucket/not-a-parameter"),
            "ParameterNotFound",
        ),
        (
            "a name with spaces between characters",
            "/hist/two words".to_string(),
            "ValidationException",
        ),
        (
            "a name naming a version",
            format!("{stored}:1"),
            "ValidationException",
        ),
        (
            "a name naming a label",
            format!("{stored}:production"),
            "ValidationException",
        ),
    ];

    for (case, name, expected) in cases {
        let (status, response) =
            history_in_region(&router, REGION, &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: reading {name} must fail, got {status} {response}"
        );
        assert_eq!(
            &error_code(&response),
            expected,
            "case {case}: reading {name} answers {expected}, got {response}"
        );
    }

    // A deleted parameter has no history to review: the store holds its versions no
    // longer.
    let (status, deleted) = call(&router, "DeleteParameter", &json!({ "Name": stored })).await;
    assert_eq!(
        status, 200,
        "deleting the seeded parameter must succeed, got {status} {deleted}"
    );
    let (status, response) = history_in_region(&router, REGION, &json!({ "Name": stored })).await;
    assert_eq!(
        status, 400,
        "a deleted parameter has no history, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ParameterNotFound",
        "a deleted parameter's history is the modelled miss, got {response}"
    );
}

/// A parameter is reached through the endpoint it was created against, and its full ARN
/// names the same history as its bare name: the name a read resolves is the one the put
/// stored, in the Region the request is signed for.
#[tokio::test]
async fn a_full_arn_and_a_bare_name_name_one_history_per_region() {
    let router = router();
    let name = "/hist/shared";

    // The same name in two Regions, with a different number of versions in each.
    let here = seed_versions(&router, name, "String", &["here-1", "here-2", "here-3"]).await;
    let there = seed_versions_in_region(&router, OTHER_REGION, name, "String", &["there-1"]).await;

    // (case, the Region to sign for, the Name to read, the values it must report)
    let cases: &[(&str, &str, String, Vec<&str>)] = &[
        (
            "a bare name here",
            REGION,
            name.to_string(),
            vec!["here-1", "here-2", "here-3"],
        ),
        (
            "a name without its leading slash",
            REGION,
            name.trim_start_matches('/').to_string(),
            vec!["here-1", "here-2", "here-3"],
        ),
        (
            "the full ARN here",
            REGION,
            format!("arn:aws:ssm:{REGION}:{ACCOUNT}:parameter{name}"),
            vec!["here-1", "here-2", "here-3"],
        ),
        (
            "a bare name in the other Region",
            OTHER_REGION,
            name.to_string(),
            vec!["there-1"],
        ),
        (
            "the full ARN in the other Region",
            OTHER_REGION,
            format!("arn:aws:ssm:{OTHER_REGION}:{ACCOUNT}:parameter{name}"),
            vec!["there-1"],
        ),
    ];

    for (case, region, read_name, expected) in cases {
        let (status, response) =
            history_in_region(&router, region, &json!({ "Name": read_name })).await;
        assert_eq!(
            status, 200,
            "case {case}: reading {read_name} in {region} must succeed, got {status} {response}"
        );
        assert_eq!(
            reported_values(case, &response),
            expected.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
            "case {case}: the history of {read_name} in {region} is that Region's own, got \
             {response}"
        );
    }

    assert_eq!(
        here.len(),
        3,
        "the seeded Region holds three versions, got {here:?}"
    );
    assert_eq!(
        there,
        vec![1],
        "the other Region's parameter starts its own version numbering, got {there:?}"
    );
}

/// `WithDecryption` selects which FORM of each value a history read reports: the stored
/// ciphertext of a `SecureString` when it is absent or false, that version's plaintext
/// when it is true — and nothing at all for the two types "this flag is ignored for"
/// (`GetParameterHistoryRequest$WithDecryption`), whose values are not encrypted.
#[tokio::test]
async fn with_decryption_selects_which_form_of_each_value_is_reported() {
    let router = router();

    // The `ParameterType` values the model's enum declares
    // (`smithy_query(query="shape", shape="ParameterType")`).
    for type_ in ["String", "StringList", "SecureString"] {
        let name = format!("/hist/decrypt/{type_}");
        let values = ["one", "two"];
        let plaintexts: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
        seed_versions(&router, &name, type_, &values).await;
        let encrypted = type_ == "SecureString";
        for with_decryption in [None, Some(false), Some(true)] {
            let case = format!("{type_} with WithDecryption {with_decryption:?}");
            let mut body = json!({ "Name": name });
            if let Some(flag) = with_decryption {
                body["WithDecryption"] = json!(flag);
            }
            let response = history_ok(&router, &case, &body).await;
            let reported = reported_values(&case, &response);
            if encrypted && with_decryption != Some(true) {
                assert_eq!(
                    reported.len(),
                    plaintexts.len(),
                    "case {case}: the history reports one value per version, got {response}"
                );
                for (reported, plaintext) in reported.iter().zip(&plaintexts) {
                    assert_ne!(
                        reported, plaintext,
                        "case {case}: an undecrypted read reports the ciphertext, not the \
                         plaintext, got {response}"
                    );
                    assert!(
                        ::aws_smithy_types::base64::decode(reported).is_ok(),
                        "case {case}: an undecrypted read reports base64 ciphertext, got \
                         {reported:?}"
                    );
                }
            } else {
                assert_eq!(
                    reported, plaintexts,
                    "case {case}: the history reports the values its puts wrote, got {response}"
                );
            }
        }
    }
}
