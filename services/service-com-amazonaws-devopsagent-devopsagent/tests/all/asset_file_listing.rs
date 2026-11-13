// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListAssetFiles`: what one asset's file listing
//! reports, how it is paged, which revision it is read at, and that a refused
//! create leaves it as it was.
//!
//! Every case drives the service through the same router the emulator serves, so
//! the routes, status codes, protocol envelope and state are all exercised.

use std::io::Write;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service's signing name (`aidevops`), which
/// is how the router resolves which service claims a request. Signature
/// verification is switched off for these tests, so the signature is a placeholder.
const AUTHORIZATION: &str = concat!(
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-west-2/aidevops/aws4_request, ",
    "SignedHeaders=host, Signature=0123456789abcdef"
);

/// One emulator: a single shared engine behind the production router.
fn new_router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_devopsagent_devopsagent::EmulatedService::new(Arc::new(
            engine::InMemoryEngine::new(),
        )),
    );
    router
}

/// Send one request and return its status and parsed body (`Value::Null` for an
/// empty body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .as_ref()
        .map(|b| Bytes::from(serde_json::to_vec(b).expect("serialize request body")))
        .unwrap_or_default();
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "aidevops.us-west-2.amazonaws.com")
        .header("authorization", AUTHORIZATION);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`) of an error response.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` of an error response.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Create an agent space and return its id.
async fn create_space(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "POST", "/v1/agentspaces", Some(json!({"name": name}))).await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected 201 from CreateAgentSpace, got {status}: {body}"
    );
    body["agentSpace"]["agentSpaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no agentSpaceId in {body}"))
        .to_string()
}

/// Create an asset of `asset_type` whose content is `content`, and return its id.
///
/// The metadata member each type is named by comes from this service's own asset
/// type table (`ASSET_TYPE_RULES` in `src/state.rs`).
async fn create_asset_of_type(
    router: &MultiServiceRouter,
    space: &str,
    case: &str,
    asset_type: &str,
    metadata: Value,
    content: Value,
) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/asset/agent-space/{space}/assets"),
        Some(json!({
            "assetType": asset_type,
            "metadata": metadata,
            "content": content,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateAsset, got {status}: {body}"
    );
    body["asset"]["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no assetId in {body}"))
        .to_string()
}

/// Create an `artifact` asset whose content is `content`, and return its id.
async fn create_asset(
    router: &MultiServiceRouter,
    space: &str,
    case: &str,
    content: Value,
) -> String {
    create_asset_of_type(
        router,
        space,
        case,
        "artifact",
        json!({"title": case}),
        content,
    )
    .await
}

/// A zip archive carrying `entries`, built with the same crate the handler reads
/// one with.
fn zip_bundle(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        for (path, content) in entries {
            writer
                .start_file(*path, zip::write::SimpleFileOptions::default())
                .expect("start zip entry");
            writer.write_all(content).expect("write zip entry");
        }
        writer.finish().expect("finish zip");
    }
    buffer.into_inner()
}

/// The `ListAssetFiles` URI for one asset.
fn files_uri(space: &str, asset: &str) -> String {
    format!("/asset/agent-space/{space}/assets/{asset}/files")
}

/// The file-plane URI for one file of one asset, with `wire_path` already in the
/// form the greedy `{path+}` label carries it in.
fn asset_file_uri(space: &str, asset: &str, wire_path: &str) -> String {
    format!("{}/{wire_path}", files_uri(space, asset))
}

/// List one asset's files, asserting a 200, and return the response body.
async fn list_files(
    router: &MultiServiceRouter,
    space: &str,
    asset: &str,
    case: &str,
    query: &str,
) -> Value {
    let uri = format!("{}{query}", files_uri(space, asset));
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListAssetFiles {uri}, got {status}: {body}"
    );
    body
}

/// The `items` of a listing, as a vector.
fn items(case: &str, body: &Value) -> Vec<Value> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no items list in {body}"))
        .clone()
}

/// The `path` of each reported summary, in the order the listing served them.
fn paths(case: &str, body: &Value) -> Vec<String> {
    items(case, body)
        .iter()
        .map(|item| {
            item["path"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no path in {item}"))
                .to_string()
        })
        .collect()
}

/// Create one file of an asset through the wire, asserting it was accepted.
async fn add_file(
    router: &MultiServiceRouter,
    space: &str,
    asset: &str,
    case: &str,
    wire_path: &str,
) {
    let (status, body) = call(
        router,
        "POST",
        &asset_file_uri(space, asset, wire_path),
        Some(json!({"content": {"text": format!("content of {wire_path}")}})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?} adding {wire_path:?}: expected 201 from CreateAssetFile, got {status}: {body}"
    );
}

/// One asset a listing case is read from: how its content arrived, which files are
/// created in it afterwards, and the files it then holds.
struct Row {
    case: &'static str,
    content: Value,
    /// Paths to create afterwards, in the WIRE form the URI carries (a space and
    /// parens are percent-encoded; the modelled `AssetFilePath` pattern
    /// `^[a-zA-Z0-9_./ ()-]+$` admits them raw, the URI does not).
    added: Vec<(&'static str, &'static str)>,
    /// Every path the asset holds once the row is set up, in no particular order.
    holds: Vec<&'static str>,
}

/// The rows every listing case runs over: assets whose content arrived through
/// each member of `AssetContent` (`smithy_query(query="shape",
/// shape="AssetContent")`) — a single `file`, a `zip` bundle of one and of several
/// entries, and a `sourceUrl` import that materialises no file — each with and
/// without files created in it afterwards.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a single-file asset",
            content: json!({"file": {"path": "only.md", "body": {"text": "only"}}}),
            added: vec![],
            holds: vec!["only.md"],
        },
        Row {
            case: "an import that materialises no file",
            content: json!({"sourceUrl": {"url": "https://example.com/bundle.zip"}}),
            added: vec![],
            holds: vec![],
        },
        Row {
            case: "a bundle of one entry",
            content: json!({
                "zip": {"zipFile": aws_smithy_types::base64::encode(zip_bundle(&[("only.md", b"only")]))}
            }),
            added: vec![],
            holds: vec!["only.md"],
        },
        Row {
            case: "a bundle of several entries",
            content: json!({
                "zip": {"zipFile": aws_smithy_types::base64::encode(zip_bundle(&[
                    ("two.md", b"two"),
                    ("one.md", b"one"),
                    ("dir/three.md", b"three"),
                ]))}
            }),
            added: vec![],
            holds: vec!["one.md", "two.md", "dir/three.md"],
        },
        Row {
            case: "an import with files created in it",
            content: json!({"sourceUrl": {"url": "https://example.com/bundle.zip"}}),
            added: vec![
                ("zeta.md", "zeta.md"),
                ("alpha.md", "alpha.md"),
                ("notes/report%20(1).bin", "notes/report (1).bin"),
                ("a/b/c/d/deep.md", "a/b/c/d/deep.md"),
            ],
            holds: vec![
                "zeta.md",
                "alpha.md",
                "notes/report (1).bin",
                "a/b/c/d/deep.md",
            ],
        },
        Row {
            case: "a bundle with files created in it",
            content: json!({
                "zip": {"zipFile": aws_smithy_types::base64::encode(zip_bundle(&[
                    ("bundled.md", b"bundled"),
                    ("dir/bundled.md", b"nested"),
                ]))}
            }),
            added: vec![("added.md", "added.md"), ("dir/added.md", "dir/added.md")],
            holds: vec![
                "bundled.md",
                "dir/bundled.md",
                "added.md",
                "dir/added.md",
            ],
        },
    ]
}

/// Set one row up in `space` and return the asset's id with the paths it holds, in
/// the order a listing reports them (by path).
async fn seed(router: &MultiServiceRouter, space: &str, row: &Row) -> (String, Vec<String>) {
    let asset = create_asset(router, space, row.case, row.content.clone()).await;
    for (wire_path, _) in &row.added {
        add_file(router, space, &asset, row.case, wire_path).await;
    }
    let mut expected: Vec<String> = row.holds.iter().map(|path| path.to_string()).collect();
    expected.sort();
    (asset, expected)
}

/// RULE: `ListAssetFiles` reports exactly the files the asset's content holds —
/// however that content arrived and whatever was created in it since — ordered by
/// path, each summary carrying that file's path, metadata, asset version and
/// timestamps as the file plane's own read of it reports them.
#[tokio::test]
async fn the_listing_reports_every_file_the_asset_holds() {
    let router = new_router();
    let space = create_space(&router, "asset-file-listing").await;

    for row in rows() {
        let (asset, expected) = seed(&router, &space, &row).await;
        let case = row.case;

        let body = list_files(&router, &space, &asset, case, "").await;
        assert_eq!(
            paths(case, &body),
            expected,
            "case {case:?}: listing reported {:?}",
            paths(case, &body)
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: a listing that served every file carried {:?}",
            body.get("nextToken")
        );

        // Every summary is the file plane's own record of that file, minus the
        // content a listing does not carry.
        for item in items(case, &body) {
            let path = item["path"].as_str().expect("a reported path");
            let wire_path = path.replace(' ', "%20");
            let (status, read) =
                call(&router, "GET", &asset_file_uri(&space, &asset, &wire_path), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from GetAssetFile {path:?}, got {status}: {read}"
            );
            for member in ["path", "metadata", "version", "createdAt", "updatedAt"] {
                assert_eq!(
                    item.get(member),
                    read["file"].get(member),
                    "case {case:?} file {path:?}: the listing's {member} is {:?} and the read's {:?}",
                    item.get(member),
                    read["file"].get(member)
                );
            }
            assert!(
                item.get("content").is_none(),
                "case {case:?} file {path:?}: a summary carried content {:?}",
                item.get("content")
            );
        }
    }
}

/// RULE: a listing is served a page at a time: each page carries at most
/// `maxResults` files in path order, and a `nextToken` only while files follow it,
/// so following the tokens walks every file of the asset exactly once.
#[tokio::test]
async fn the_listing_is_served_a_page_at_a_time() {
    let router = new_router();
    let space = create_space(&router, "asset-file-paging").await;

    // `maxResults` carries `@range(min: 1, max: 100)`
    // (`smithy_query(query="operation", operation="ListAssetFiles")`), so both
    // published bounds and a size between them are paged through here.
    for page_size in [1usize, 2, 3, 100] {
        for row in rows() {
            let (asset, expected) = seed(&router, &space, &row).await;
            let case = format!("{} at maxResults={page_size}", row.case);

            let mut seen: Vec<String> = Vec::new();
            let mut query = format!("?maxResults={page_size}");
            loop {
                let body = list_files(&router, &space, &asset, &case, &query).await;
                let page = paths(&case, &body);
                assert!(
                    page.len() <= page_size,
                    "case {case:?}: a page carried {} files: {page:?}",
                    page.len()
                );
                seen.extend(page.clone());
                match body.get("nextToken").and_then(Value::as_str) {
                    Some(token) => {
                        assert_eq!(
                            page.len(),
                            page_size,
                            "case {case:?}: a short page carried a nextToken: {page:?}"
                        );
                        query = format!("?maxResults={page_size}&nextToken={token}");
                    }
                    None => break,
                }
                assert!(
                    seen.len() <= expected.len(),
                    "case {case:?}: paging served {:?} for {expected:?}",
                    seen
                );
            }
            assert_eq!(
                seen, expected,
                "case {case:?}: paging served {seen:?}"
            );
        }
    }
}

/// RULE: `maxResults` outside the modelled range names no page size, so the
/// request is refused with HTTP 400 `ValidationException`, and so is a `nextToken`
/// this service did not issue — neither is read as "serve everything".
#[tokio::test]
async fn a_page_the_model_does_not_admit_is_refused() {
    let router = new_router();
    let space = create_space(&router, "asset-file-paging-refusals").await;
    let asset = create_asset(
        &router,
        &space,
        "paging refusals",
        json!({"zip": {"zipFile": aws_smithy_types::base64::encode(zip_bundle(&[
            ("one.md", b"one"),
            ("two.md", b"two"),
        ]))}}),
    )
    .await;

    // One past each published bound of `maxResults` (`@range(min: 1, max: 100)`),
    // then tokens no issuing of this service's own could have produced.
    let refused = [
        "?maxResults=0",
        "?maxResults=101",
        "?nextToken=not-a-token",
        // The prefix a token of this service carries, over a cursor that names no
        // file of an asset (an absolute path, and a walk out of the asset).
        "?nextToken=v1:2f726f6f74",
        "?nextToken=v1:2e2e2f6f75742e6d64",
    ] ;
    for query in refused {
        let uri = format!("{}{query}", files_uri(&space, &asset));
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 400,
            "case {query:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {query:?}: refusal answered {:?}",
            error_code(&body)
        );
    }

    // A token the service DID issue resumes the listing it was issued from.
    let first = list_files(&router, &space, &asset, "issued token", "?maxResults=1").await;
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"issued token\": no nextToken in {first}"));
    let second = list_files(
        &router,
        &space,
        &asset,
        "issued token",
        &format!("?maxResults=1&nextToken={token}"),
    )
    .await;
    assert_eq!(
        paths("issued token", &second),
        vec!["two.md".to_string()],
        "case \"issued token\": the resumed page served {:?}",
        paths("issued token", &second)
    );
}

/// RULE: `assetVersion` reads the listing at one revision of the asset, so it
/// reports the files that revision's content held; omitted, the revision the asset
/// is at is listed, and a version the asset does not have names no revision at all
/// (HTTP 404 `ResourceNotFoundException`).
#[tokio::test]
async fn a_listing_is_read_at_one_revision_of_the_asset() {
    let router = new_router();
    let space = create_space(&router, "asset-file-revisions").await;
    let case = "revisions of one asset";
    let asset = create_asset(
        &router,
        &space,
        case,
        json!({"file": {"path": "only.md", "body": {"text": "first"}}}),
    )
    .await;

    // Revision 1 holds the file the asset was created with. Updating it moves the
    // asset to revision 2, which holds the same path at the new content; deleting
    // it would empty the asset, so the second revision is reached by an update.
    let (status, updated) = call(
        &router,
        "PATCH",
        &asset_file_uri(&space, &asset, "only.md"),
        Some(json!({"content": {"text": "second"}})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from UpdateAssetFile, got {status}: {updated}"
    );

    for (query, expected_version) in [("", 2), ("?assetVersion=2", 2), ("?assetVersion=1", 1)] {
        let body = list_files(&router, &space, &asset, case, query).await;
        assert_eq!(
            paths(case, &body),
            vec!["only.md".to_string()],
            "case {case:?} at {query:?}: listing reported {:?}",
            paths(case, &body)
        );
        assert_eq!(
            items(case, &body)[0]["version"].as_i64(),
            Some(expected_version),
            "case {case:?} at {query:?}: the file reported version {:?}",
            items(case, &body)[0]["version"]
        );
    }

    // A revision the asset never reached names nothing to list.
    for version in [3, 4, 100] {
        let uri = format!("{}?assetVersion={version}", files_uri(&space, &asset));
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 404,
            "case {case:?} at version {version}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?} at version {version}: refusal answered {:?}",
            error_code(&body)
        );
    }
}

/// RULE: a listing is addressed to a resource inside an agent space, so a space
/// that resolves to nothing fails closed (HTTP 403 `AccessDeniedException`) and
/// only inside a space the caller reaches is an asset that resolves to nothing
/// reported as missing (HTTP 404 `ResourceNotFoundException`).
#[tokio::test]
async fn a_listing_of_something_that_resolves_to_nothing_is_refused() {
    let router = new_router();
    let space = create_space(&router, "asset-file-listing-refusals").await;
    let asset = create_asset(
        &router,
        &space,
        "listing refusals",
        json!({"file": {"path": "only.md", "body": {"text": "only"}}}),
    )
    .await;

    for (case, uri, status_expected, code_expected) in [
        (
            "a space that resolves to nothing",
            files_uri("as-0000000000000000", &asset),
            403,
            "AccessDeniedException",
        ),
        (
            "an asset that resolves to nothing",
            files_uri(&space, "asset-absent"),
            404,
            "ResourceNotFoundException",
        ),
    ] {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, status_expected,
            "case {case:?}: expected {status_expected}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            code_expected,
            "case {case:?}: refusal answered {:?}",
            error_code(&body)
        );
    }
}

/// RULE: an asset whose content IS one file holds exactly that file, so a create
/// of another is refused with HTTP 400 `ValidationException` reporting how many
/// files an asset of its TYPE supports and pointing the caller at the operation
/// that replaces a stored file — and the refusal changes nothing: the listing
/// still reports the one file, at the content it was created with.
#[tokio::test]
async fn the_refused_second_file_leaves_the_single_file_asset_as_it_was() {
    let router = new_router();
    let space = create_space(&router, "one-file-asset-refusal").await;

    // The rows differ along the asset's TYPE — the member of its metadata each
    // type is named by comes from this service's own asset type table
    // (`ASSET_TYPE_RULES` in `src/state.rs`), and the quoted type in the refusal
    // is the asset's own.
    let cases: Vec<(&str, Value)> = vec![
        ("artifact", json!({"title": "an artifact"})),
        ("skill", json!({"name": "a skill"})),
        ("attachment", json!({"filename": "held.md"})),
    ];

    for (asset_type, metadata) in &cases {
        let case = format!("an asset of type {asset_type:?}");
        let asset = create_asset_of_type(
            &router,
            &space,
            &case,
            asset_type,
            metadata.clone(),
            json!({"file": {"path": "held.md", "body": {"text": "original"}}}),
        )
        .await;

        let (status, refusal) = call(
            &router,
            "POST",
            &asset_file_uri(&space, &asset, "second.md"),
            Some(json!({"content": {"text": "second"}})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateAssetFile, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ValidationException",
            "case {case:?}: refusal answered {:?}",
            error_code(&refusal)
        );
        assert_eq!(
            error_message(&refusal),
            format!(
                "Assets of type '{asset_type}' support only one file. \
                 Use UpdateAssetFile to replace the existing file."
            ),
            "case {case:?}: refusal message was {:?}",
            error_message(&refusal)
        );

        // Nothing of the asset moved: the listing still reports the one file it
        // was created with, and that file still holds its original content.
        let body = list_files(&router, &space, &asset, &case, "").await;
        assert_eq!(
            paths(&case, &body),
            vec!["held.md".to_string()],
            "case {case:?}: listing after the refusal reported {:?}",
            paths(&case, &body)
        );
        let (status, read) = call(
            &router,
            "GET",
            &asset_file_uri(&space, &asset, "held.md"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetAssetFile, got {status}: {read}"
        );
        assert_eq!(
            read["file"]["content"]["text"].as_str(),
            Some("original"),
            "case {case:?}: the held file reported content {:?}",
            read["file"]["content"]
        );
    }
}
