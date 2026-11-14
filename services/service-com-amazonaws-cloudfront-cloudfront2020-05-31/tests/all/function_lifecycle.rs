// SPDX-License-Identifier: CC0-1.0

//! The CloudFront function control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path
//! a client takes. The rules under test are the ones CloudFront's function surface
//! states:
//!
//! * a function is PERSISTED by the call that creates it, which MINTS its identity (the
//!   ARN built from its name, and the version token of its `DEVELOPMENT` stage), so a
//!   later `DescribeFunction` reports that record's configuration and metadata and a
//!   later `GetFunction` reports its code byte for byte — neither answers from the
//!   request being served;
//! * "When you create a function, it's in the `DEVELOPMENT` stage", and the `LIVE` stage
//!   exists only once `PublishFunction` has copied `DEVELOPMENT` into it, so a read of
//!   the `LIVE` stage of a function that was only created is a function that does not
//!   exist;
//! * "A function with the same name already exists in this Amazon Web Services account.
//!   To create a function, you must provide a unique name": a name identifies ONE
//!   function for its whole life, so a repeated name is refused and leaves the function
//!   that holds it untouched;
//! * a create the service cannot hold — code past the published function-size quota, a
//!   list wrapper whose `Quantity` disagrees with its `Items`, more key value store
//!   associations than a function may carry, or an association naming a key value store
//!   this service does not hold — is refused with the error the model names for it, and
//!   stores nothing.
//!
//! The table rows differ along everything these rules generalise over: the name's length
//! (both published bounds of `FunctionName`, and the character classes its pattern
//! admits), the runtime (every value the modelled `FunctionRuntime` enum admits), the
//! comment's length and whether it is ASCII, the code's size (the modelled minimum, a
//! real script, and the published quota bound) and whether the code is text at all (a
//! `FunctionBlob` is a blob, so arbitrary bytes have to survive the round trip), whether
//! the configuration associates a key value store, and whether the create carries tags.

use std::collections::BTreeSet;

use bytes::Bytes;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    FUNCTION_STAGE_DEVELOPMENT, FUNCTION_STAGE_LIVE, FUNCTION_STATUS_UNPUBLISHED, MAX_FUNCTIONS,
    MAX_FUNCTION_CODE_BYTES, MAX_TAGS_PER_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, is_version_token, Answer, Node, ACCOUNT_ID, API_VERSION, AUTHORIZATION, HOST,
};
// The XML-entity escaper every suite of this crate sends request documents through: one
// `pub(crate)` copy, in the suite that first needed it, rather than a copy per suite.
use crate::response_headers_policy_lifecycle::escape;

/// Encode `bytes` as the padded standard base64 a restXml blob MEMBER travels as (the
/// form every AWS SDK puts a blob inside a document in).
///
/// `aws_smithy_types::base64::encode` is the runtime's own encoder — the one the sibling
/// suites of this workspace send blob members through — and this crate already depends on
/// `aws-smithy-types`, so nothing is added to the manifest or the lockfile for it.
pub(crate) fn base64(bytes: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(bytes)
}

// ---------------------------------------------------------------------------
// Calling the function surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/function` with the request document `body`.
pub(crate) async fn create_function(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/function"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/function/{Name}/describe`, naming `stage` when it is `Some` and
/// leaving the optional query parameter out entirely when it is `None`.
pub(crate) async fn describe_function(
    router: &MultiServiceRouter,
    name: &str,
    stage: Option<&str>,
) -> Answer {
    call(router, "GET", &describe_uri(name, stage), None, None).await
}

/// The URI a describe of `name` at `stage` is sent to.
fn describe_uri(name: &str, stage: Option<&str>) -> String {
    match stage {
        Some(stage) => format!("/{API_VERSION}/function/{name}/describe?Stage={stage}"),
        None => format!("/{API_VERSION}/function/{name}/describe"),
    }
}

/// `POST /2020-05-31/key-value-store`, so a function configuration can name a store this
/// service actually holds. Answers the ARN the create minted.
pub(crate) async fn create_key_value_store(router: &MultiServiceRouter, name: &str) -> String {
    let body = format!(
        "<CreateKeyValueStoreRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name></CreateKeyValueStoreRequest>"
    );
    let answer = call(
        router,
        "POST",
        &format!("/{API_VERSION}/key-value-store"),
        Some(&body),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "the key value store {name} must be created, got {} with {}",
        answer.status, answer.raw
    );
    answer
        .root()
        .text_at("ARN")
        .unwrap_or_else(|| panic!("the key value store {name} carries an ARN: {}", answer.raw))
        .to_string()
}

/// `GET /2020-05-31/tagging?Resource=<arn>`.
async fn list_tags_for_resource(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/tagging?Resource={arn}"),
        None,
        None,
    )
    .await
}

/// `POST /2020-05-31/tagging?Operation=Tag&Resource=<arn>` with one tag.
async fn tag_resource(
    router: &MultiServiceRouter,
    arn: &str,
    key: &str,
    value: &str,
) -> Answer {
    let body = format!(
        "<Tags xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\"><Items>\
         <Tag><Key>{}</Key><Value>{}</Value></Tag></Items></Tags>",
        escape(key),
        escape(value)
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/tagging?Operation=Tag&Resource={arn}"),
        Some(&body),
        None,
    )
    .await
}

/// What a `GetFunction` answered: the status, the `ETag` and `Content-Type` headers, and
/// the response payload as BYTES.
///
/// `GetFunction`'s payload is the `FunctionBlob` itself rather than a document, so it is
/// read as bytes: code that is not valid UTF-8 (a blob may be anything) must come back
/// unchanged, which a string would not show.
pub(crate) struct CodeAnswer {
    pub(crate) status: u16,
    pub(crate) etag: String,
    content_type: String,
    pub(crate) body: Vec<u8>,
}

/// `GET /2020-05-31/function/{Name}`, read as raw bytes.
pub(crate) async fn get_function(
    router: &MultiServiceRouter,
    name: &str,
    stage: Option<&str>,
) -> CodeAnswer {
    let uri = match stage {
        Some(stage) => format!("/{API_VERSION}/function/{name}?Stage={stage}"),
        None => format!("/{API_VERSION}/function/{name}"),
    };
    let request = ::http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", HOST)
        .header("authorization", AUTHORIZATION)
        .body(Bytes::new())
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let etag = header("etag");
    let content_type = header("content-type");
    CodeAnswer {
        status,
        etag,
        content_type,
        body: response.into_body().to_vec(),
    }
}

// ---------------------------------------------------------------------------
// The creates under test
// ---------------------------------------------------------------------------

/// One `CreateFunction` request a client submits.
#[derive(Debug, Clone)]
pub(crate) struct Create {
    pub(crate) case: &'static str,
    pub(crate) name: String,
    /// `FunctionConfig.Comment`, which is required on the wire.
    pub(crate) comment: String,
    /// `FunctionConfig.Runtime`.
    pub(crate) runtime: &'static str,
    /// `FunctionCode`, as the bytes a client submits.
    pub(crate) code: Vec<u8>,
    /// The `KeyValueStoreARN` of each association the configuration names.
    pub(crate) associations: Vec<String>,
    /// `Tags.Items`, as `(Key, Value)` pairs.
    pub(crate) tags: Vec<(&'static str, &'static str)>,
}

impl Create {
    /// The `CreateFunctionRequest` XML a client sends for this create.
    pub(crate) fn xml(&self) -> String {
        let mut out = format!(
            "<CreateFunctionRequest xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
        );
        out.push_str(&format!("<Name>{}</Name>", escape(&self.name)));
        out.push_str(&self.config_xml());
        out.push_str(&format!(
            "<FunctionCode>{}</FunctionCode>",
            base64(&self.code)
        ));
        if !self.tags.is_empty() {
            out.push_str("<Tags><Items>");
            for (key, value) in &self.tags {
                out.push_str(&format!(
                    "<Tag><Key>{}</Key><Value>{}</Value></Tag>",
                    escape(key),
                    escape(value)
                ));
            }
            out.push_str("</Items></Tags>");
        }
        out.push_str("</CreateFunctionRequest>");
        out
    }

    /// The `FunctionConfig` element this create carries.
    pub(crate) fn config_xml(&self) -> String {
        let mut out = format!(
            "<FunctionConfig><Comment>{}</Comment><Runtime>{}</Runtime>",
            escape(&self.comment),
            self.runtime
        );
        if !self.associations.is_empty() {
            out.push_str(&format!(
                "<KeyValueStoreAssociations><Quantity>{}</Quantity><Items>",
                self.associations.len()
            ));
            for arn in &self.associations {
                out.push_str(&format!(
                    "<KeyValueStoreAssociation><KeyValueStoreARN>{}</KeyValueStoreARN>\
                     </KeyValueStoreAssociation>",
                    escape(arn)
                ));
            }
            out.push_str("</Items></KeyValueStoreAssociations>");
        }
        out.push_str("</FunctionConfig>");
        out
    }
}

/// A CloudFront function that rewrites the request URI and a header — the shape of code a
/// viewer-request function carries, including the characters XML predefines, so the
/// round trip is tested on code a client really sends.
pub(crate) const VIEWER_REQUEST_CODE: &str = "function handler(event) {\n\
     \x20   var request = event.request;\n\
     \x20   if (request.uri === '/old' && request.method === 'GET') {\n\
     \x20       request.uri = '/new';\n\
     \x20   }\n\
     \x20   request.headers['x-rewritten'] = { value: '1 < 2' };\n\
     \x20   return request;\n\
     }\n";

/// The creates every rule below is asserted over.
///
/// `FunctionName` is `length 1..64, pattern ^[a-zA-Z0-9-_]{1,64}$` and `FunctionBlob` is
/// `length 1..40960`; `FunctionRuntime` admits exactly `cloudfront-js-1.0` and
/// `cloudfront-js-2.0` (all three from `smithy_query` on `CreateFunction` and
/// `FunctionRuntime`). So the rows sit at both name bounds, draw from each character
/// class the name pattern admits, use each runtime the enum admits, and carry the
/// smallest code the model allows, a real script, code that is not UTF-8 at all, and code
/// exactly at the published function-size quota.
///
/// `kvs_arn` is the ARN of a key value store this service holds, so the row that
/// associates one names a store that really exists.
pub(crate) fn create_table(kvs_arn: &str) -> Vec<Create> {
    vec![
        Create {
            case: "the shortest admissible name, the smallest admissible code",
            name: "a".to_string(),
            comment: String::new(),
            runtime: "cloudfront-js-1.0",
            code: b"f".to_vec(),
            associations: Vec::new(),
            tags: Vec::new(),
        },
        Create {
            case: "the longest admissible name (64), a viewer-request script",
            name: "n".repeat(64),
            comment: "rewrites the URI and a header".to_string(),
            runtime: "cloudfront-js-2.0",
            code: VIEWER_REQUEST_CODE.as_bytes().to_vec(),
            associations: Vec::new(),
            tags: Vec::new(),
        },
        Create {
            case: "hyphens and underscores, code that is not UTF-8",
            name: "orders-by_region-2".to_string(),
            comment: "プロダクション — flags & limits".to_string(),
            runtime: "cloudfront-js-1.0",
            code: vec![0x00, 0xff, 0xfe, 0x80, b'\n', 0x7f, 0x01],
            associations: Vec::new(),
            tags: Vec::new(),
        },
        Create {
            case: "mixed case and digits, code exactly at the published quota",
            name: "Viewer42".to_string(),
            comment: "c".repeat(128),
            runtime: "cloudfront-js-2.0",
            code: vec![b'x'; MAX_FUNCTION_CODE_BYTES],
            associations: Vec::new(),
            tags: Vec::new(),
        },
        Create {
            case: "a key value store association, with tags",
            name: "catalog_lookup".to_string(),
            comment: "reads the catalog from a key value store".to_string(),
            runtime: "cloudfront-js-2.0",
            code: VIEWER_REQUEST_CODE.as_bytes().to_vec(),
            associations: vec![kvs_arn.to_string()],
            tags: vec![("stage", "prod"), ("owner", "edge-team")],
        },
    ]
}

// ---------------------------------------------------------------------------
// Reading what the service stored
// ---------------------------------------------------------------------------

/// The text of one member of the `FunctionSummary` document, or a panic naming the case.
pub(crate) fn member(summary: &Node, path: &str, case: &str, raw: &str) -> String {
    summary
        .text_at(path)
        .unwrap_or_else(|| panic!("case {case}: the answer carries no {path}: {raw}"))
        .to_string()
}

/// Assert the `FunctionSummary` document `answer` carries is the function `create` asked
/// for, in `stage`, and answer the ARN it reports.
pub(crate) fn assert_summary(answer: &Answer, create: &Create, stage: &str, context: &str) -> String {
    let case = create.case;
    let summary = answer.root();
    let raw = answer.raw.as_str();

    let name = member(summary, "Name", case, raw);
    assert_eq!(
        name, create.name,
        "case {case} ({context}): expected the submitted Name, got {name:?}"
    );

    let status = member(summary, "Status", case, raw);
    assert_eq!(
        status, FUNCTION_STATUS_UNPUBLISHED,
        "case {case} ({context}): expected Status {FUNCTION_STATUS_UNPUBLISHED}, got {status:?}"
    );

    let comment = summary
        .text_at("FunctionConfig/Comment")
        .unwrap_or("")
        .to_string();
    assert_eq!(
        comment, create.comment,
        "case {case} ({context}): expected the submitted Comment, got {comment:?}"
    );

    let runtime = member(summary, "FunctionConfig/Runtime", case, raw);
    assert_eq!(
        runtime, create.runtime,
        "case {case} ({context}): expected the submitted Runtime, got {runtime:?}"
    );

    let associated: Vec<String> = summary
        .at("FunctionConfig/KeyValueStoreAssociations/Items")
        .map(|items| {
            items
                .children_named("KeyValueStoreAssociation")
                .iter()
                .map(|association| {
                    association
                        .text_at("KeyValueStoreARN")
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        associated, create.associations,
        "case {case} ({context}): expected the submitted key value store associations, got \
         {associated:?}"
    );

    let reported_stage = member(summary, "FunctionMetadata/Stage", case, raw);
    assert_eq!(
        reported_stage, stage,
        "case {case} ({context}): expected Stage {stage}, got {reported_stage:?}"
    );

    let created = member(summary, "FunctionMetadata/CreatedTime", case, raw);
    let modified = member(summary, "FunctionMetadata/LastModifiedTime", case, raw);
    assert!(
        !created.is_empty() && !modified.is_empty(),
        "case {case} ({context}): expected both function timestamps, got {created:?} and \
         {modified:?}"
    );

    let arn = member(summary, "FunctionMetadata/FunctionARN", case, raw);
    assert_eq!(
        arn,
        format!("arn:aws:cloudfront::{ACCOUNT_ID}:function/{}", create.name),
        "case {case} ({context}): expected the ARN built from the Name, got {arn:?}"
    );
    arn
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create PERSISTS the function it mints, and both reads answer from that one record:
/// the describe reports the submitted configuration with the `DEVELOPMENT` stage and the
/// unpublished status, the get reports the submitted code byte for byte, and both report
/// the version token the create handed back. Across the table, no two functions share an
/// ARN or a version token.
#[tokio::test]
async fn create_persists_the_function_both_reads_then_report() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "function_table_store").await;
    let mut arns = BTreeSet::new();
    let mut etags = BTreeSet::new();

    for create in create_table(&kvs_arn) {
        let case = create.case;
        let created = create_function(&router, &create.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );

        let arn = assert_summary(&created, &create, FUNCTION_STAGE_DEVELOPMENT, "the create");
        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token in the ETag header, got {etag:?}"
        );
        let location = created.header("location").to_string();
        assert_eq!(
            location,
            format!("https://{HOST}/{API_VERSION}/function/{arn}"),
            "case {case}: expected the new function's URI in Location, got {location:?}"
        );
        assert!(arns.insert(arn.clone()), "case {case}: the ARN {arn} repeated");
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );

        // The describe answers from the stored record, not from any request.
        let described = describe_function(&router, &create.name, None).await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected HTTP 200 from the describe, got {} with {}",
            described.status, described.raw
        );
        let described_arn =
            assert_summary(&described, &create, FUNCTION_STAGE_DEVELOPMENT, "the describe");
        assert_eq!(
            described_arn, arn,
            "case {case}: the describe must report the ARN the create minted"
        );
        assert_eq!(
            described.header("etag"),
            etag,
            "case {case}: expected the create's version token from the describe"
        );

        // Naming the stage explicitly is the same read as naming none.
        let explicit =
            describe_function(&router, &create.name, Some(FUNCTION_STAGE_DEVELOPMENT)).await;
        assert_eq!(
            explicit.raw, described.raw,
            "case {case}: naming the DEVELOPMENT stage must read the same record"
        );

        // The code comes back byte for byte, whatever bytes it was.
        let code = get_function(&router, &create.name, None).await;
        assert_eq!(
            code.status, 200,
            "case {case}: expected HTTP 200 from the get, got {} with {:?}",
            code.status,
            String::from_utf8_lossy(&code.body)
        );
        assert_eq!(
            code.body, create.code,
            "case {case}: expected the submitted code ({} bytes), got {} bytes",
            create.code.len(),
            code.body.len()
        );
        assert_eq!(
            code.etag, etag,
            "case {case}: expected the create's version token from the get, got {:?}",
            code.etag
        );
        assert_eq!(
            code.content_type, "application/octet-stream",
            "case {case}: expected the blob media type, got {:?}",
            code.content_type
        );

        // The tags submitted with the create are the function's tags.
        let tags = list_tags_for_resource(&router, &arn).await;
        assert_eq!(
            tags.status, 200,
            "case {case}: expected HTTP 200 from the tag read, got {} with {}",
            tags.status, tags.raw
        );
        let read: Vec<(String, String)> = function_tags(&tags);
        let expected: Vec<(String, String)> = {
            let mut expected: Vec<(String, String)> = create
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            expected.sort();
            expected
        };
        assert_eq!(
            read, expected,
            "case {case}: expected the submitted tags on the function, got {read:?}"
        );
    }
}

/// "When you create a function, it's in the `DEVELOPMENT` stage": the `LIVE` stage exists
/// only once a publish has copied `DEVELOPMENT` into it, so every read of the `LIVE`
/// stage of a function that was only created is a function that does not exist — while
/// the same read of `DEVELOPMENT` answers.
#[tokio::test]
async fn the_live_stage_exists_only_after_a_publish() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "live_stage_store").await;

    for create in create_table(&kvs_arn) {
        let case = create.case;
        let created = create_function(&router, &create.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );

        let described = describe_function(&router, &create.name, Some(FUNCTION_STAGE_LIVE)).await;
        assert_eq!(
            described.status, 404,
            "case {case}: expected HTTP 404 describing the LIVE stage, got {} with {}",
            described.status, described.raw
        );
        assert_eq!(
            described.error_code(),
            "NoSuchFunctionExists",
            "case {case}: expected NoSuchFunctionExists, got {}",
            described.raw
        );

        let code = get_function(&router, &create.name, Some(FUNCTION_STAGE_LIVE)).await;
        assert_eq!(
            code.status, 404,
            "case {case}: expected HTTP 404 getting the LIVE stage, got {} with {:?}",
            code.status,
            String::from_utf8_lossy(&code.body)
        );

        // The DEVELOPMENT stage is there all the same.
        let development =
            describe_function(&router, &create.name, Some(FUNCTION_STAGE_DEVELOPMENT)).await;
        assert_eq!(
            development.status, 200,
            "case {case}: expected HTTP 200 describing DEVELOPMENT, got {} with {}",
            development.status, development.raw
        );
    }
}

/// "A function with the same name already exists in this Amazon Web Services account":
/// the second create under a name is refused with `FunctionAlreadyExists` (HTTP 409), and
/// the function that holds the name keeps the code, the configuration and the version
/// token its own create stored.
#[tokio::test]
async fn a_name_names_at_most_one_function() {
    let router = emulator();
    let kvs_arn = create_key_value_store(&router, "duplicate_name_store").await;

    for create in create_table(&kvs_arn) {
        let case = create.case;
        let first = create_function(&router, &create.xml()).await;
        assert_eq!(
            first.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            first.status, first.raw
        );
        let etag = first.header("etag").to_string();

        // The same name again — with a different comment, runtime and code, so a
        // function that was replaced rather than refused would be visible.
        let repeat = Create {
            comment: "a second create under a name already taken".to_string(),
            runtime: "cloudfront-js-1.0",
            code: b"function handler(event) { return event.request; }".to_vec(),
            associations: Vec::new(),
            tags: Vec::new(),
            ..create.clone()
        };
        let second = create_function(&router, &repeat.xml()).await;
        assert_eq!(
            second.status, 409,
            "case {case}: expected HTTP 409 for a repeated name, got {} with {}",
            second.status, second.raw
        );
        assert_eq!(
            second.error_code(),
            "FunctionAlreadyExists",
            "case {case}: expected FunctionAlreadyExists, got {}",
            second.raw
        );

        let described = describe_function(&router, &create.name, None).await;
        assert_summary(
            &described,
            &create,
            FUNCTION_STAGE_DEVELOPMENT,
            "after a refusal",
        );
        assert_eq!(
            described.header("etag"),
            etag,
            "case {case}: the refused create must not have spent a version token"
        );
        let code = get_function(&router, &create.name, None).await;
        assert_eq!(
            code.body, create.code,
            "case {case}: the refused create must not have replaced the code"
        );
    }
}

/// A name no function holds is not found — `NoSuchFunctionExists` (HTTP 404) — whatever
/// shape the name takes, and a create under one name leaves every other name unfound.
#[tokio::test]
async fn a_name_no_function_holds_is_not_found() {
    let router = emulator();
    let present = Create {
        case: "the one function that exists",
        name: "present_function".to_string(),
        comment: "the only function".to_string(),
        runtime: "cloudfront-js-2.0",
        code: VIEWER_REQUEST_CODE.as_bytes().to_vec(),
        associations: Vec::new(),
        tags: Vec::new(),
    };
    let created = create_function(&router, &present.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        present.case, created.status, created.raw
    );

    // Rows: a name never created, a name differing only in case (names are compared as
    // given), a name that is a prefix of the one that exists, and the longest name the
    // model admits.
    let absent: [(&str, String); 4] = [
        ("a name never created", "absent_function".to_string()),
        ("the same name in another case", "PRESENT_FUNCTION".to_string()),
        ("a prefix of the existing name", "present".to_string()),
        ("the longest admissible name", "z".repeat(64)),
    ];
    for (case, name) in absent {
        let described = describe_function(&router, &name, None).await;
        assert_eq!(
            described.status, 404,
            "case {case}: expected HTTP 404 for {name:?}, got {} with {}",
            described.status, described.raw
        );
        assert_eq!(
            described.error_code(),
            "NoSuchFunctionExists",
            "case {case}: expected NoSuchFunctionExists for {name:?}, got {}",
            described.raw
        );

        let code = get_function(&router, &name, None).await;
        assert_eq!(
            code.status, 404,
            "case {case}: expected HTTP 404 from the get for {name:?}, got {}",
            code.status
        );
    }
}

/// CloudFront's published quota bounds the function code: code exactly at the bound is
/// accepted, code past it is refused with `FunctionSizeLimitExceeded` (HTTP 413), and code
/// past the MODELLED bound (`FunctionBlob`, `length 1..40960`) is refused at the boundary
/// with HTTP 400. Every refusal stores nothing, so the name it named stays unused.
#[tokio::test]
async fn code_past_the_published_quota_is_refused() {
    let router = emulator();

    // Rows: the published bound itself, one byte past it, and one byte past the modelled
    // maximum of `FunctionBlob`.
    let cases: [(&str, usize, u16, &str); 3] = [
        ("exactly the published quota", MAX_FUNCTION_CODE_BYTES, 201, ""),
        (
            "one byte past the published quota",
            MAX_FUNCTION_CODE_BYTES + 1,
            413,
            "FunctionSizeLimitExceeded",
        ),
        ("one byte past the modelled maximum", 40_961, 400, ""),
    ];

    for (case, size, status, error_code) in cases {
        let name = format!("sized_{size}");
        let create = Create {
            case: "sized code",
            name: name.clone(),
            comment: format!("{size} bytes of code"),
            runtime: "cloudfront-js-2.0",
            code: vec![b'y'; size],
            associations: Vec::new(),
            tags: Vec::new(),
        };
        let answer = create_function(&router, &create.xml()).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status} for {size} bytes, got {} with {}",
            answer.status, answer.raw
        );
        if !error_code.is_empty() {
            assert_eq!(
                answer.error_code(),
                error_code,
                "case {case}: expected {error_code}, got {}",
                answer.raw
            );
        }

        let described = describe_function(&router, &name, None).await;
        let expected_read = if status == 201 { 200 } else { 404 };
        assert_eq!(
            described.status, expected_read,
            "case {case}: expected HTTP {expected_read} from the describe, got {} with {}",
            described.status, described.raw
        );
    }
}

/// CloudFront's published account quota bounds how many functions one account holds: the
/// create at the bound is accepted and the one past it is refused with `TooManyFunctions`,
/// which stores nothing — so the name it named stays unused and every function already
/// stored is untouched.
#[tokio::test]
async fn a_create_past_the_account_quota_is_refused() {
    let router = emulator();

    // The quota is an entry count, so the rows that matter are the last create within it
    // and the first one past it.
    for index in 0..MAX_FUNCTIONS {
        let create = Create {
            case: "filling the account quota",
            name: format!("quota_{index}"),
            comment: format!("function {index}"),
            runtime: "cloudfront-js-2.0",
            code: b"f".to_vec(),
            associations: Vec::new(),
            tags: Vec::new(),
        };
        let answer = create_function(&router, &create.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case within the quota: expected HTTP 201 for function {index}, got {} with {}",
            answer.status, answer.raw
        );
    }

    let over = Create {
        case: "one function past the account quota",
        name: "quota_past".to_string(),
        comment: "one too many".to_string(),
        runtime: "cloudfront-js-2.0",
        code: b"f".to_vec(),
        associations: Vec::new(),
        tags: Vec::new(),
    };
    let answer = create_function(&router, &over.xml()).await;
    assert_eq!(
        answer.status, 400,
        "case {}: expected HTTP 400, got {} with {}",
        over.case, answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "TooManyFunctions",
        "case {}: expected TooManyFunctions, got {}",
        over.case,
        answer.raw
    );

    let described = describe_function(&router, &over.name, None).await;
    assert_eq!(
        described.status, 404,
        "case {}: the refused create must store nothing, got {} with {}",
        over.case, described.status, described.raw
    );
    // The functions already stored are still there, unchanged.
    let first = describe_function(&router, "quota_0", None).await;
    assert_eq!(
        first.status, 200,
        "case {}: the refused create must leave the stored functions alone, got {} with {}",
        over.case, first.status, first.raw
    );
}

/// A configuration this service cannot hold is refused with the modelled `InvalidArgument`
/// (HTTP 400) and stores nothing: a `KeyValueStoreAssociations` wrapper whose `Quantity`
/// disagrees with its `Items`, more associations than a function may carry, and an
/// association naming a key value store this service does not hold.
#[tokio::test]
async fn a_configuration_the_service_cannot_hold_is_refused() {
    let router = emulator();
    let first = create_key_value_store(&router, "config_store_one").await;
    let second = create_key_value_store(&router, "config_store_two").await;
    // A well-formed key value store ARN (the modelled pattern demands one) that no store
    // in this service holds.
    let absent = format!(
        "arn:aws:cloudfront::{ACCOUNT_ID}:key-value-store/\
         00000000-0000-4000-8000-000000000000"
    );

    // Rows: a wrapper that overstates its length, one that understates it, two
    // associations where one is the most a function may carry, and one association naming
    // a store that does not exist.
    let overstated = format!(
        "<FunctionConfig><Comment>overstated</Comment><Runtime>cloudfront-js-2.0</Runtime>\
         <KeyValueStoreAssociations><Quantity>2</Quantity><Items>\
         <KeyValueStoreAssociation><KeyValueStoreARN>{first}</KeyValueStoreARN>\
         </KeyValueStoreAssociation></Items></KeyValueStoreAssociations></FunctionConfig>"
    );
    let understated = format!(
        "<FunctionConfig><Comment>understated</Comment><Runtime>cloudfront-js-2.0</Runtime>\
         <KeyValueStoreAssociations><Quantity>0</Quantity><Items>\
         <KeyValueStoreAssociation><KeyValueStoreARN>{first}</KeyValueStoreARN>\
         </KeyValueStoreAssociation></Items></KeyValueStoreAssociations></FunctionConfig>"
    );
    let cases: [(&str, String); 4] = [
        ("a Quantity above the Items length", overstated),
        ("a Quantity below the Items length", understated),
        (
            "more associations than a function may carry",
            Create {
                case: "two associations",
                name: "two_associations".to_string(),
                comment: "two stores".to_string(),
                runtime: "cloudfront-js-2.0",
                code: b"f".to_vec(),
                associations: vec![first.clone(), second.clone()],
                tags: Vec::new(),
            }
            .config_xml(),
        ),
        (
            "an association naming a store that does not exist",
            Create {
                case: "an absent store",
                name: "absent_association".to_string(),
                comment: "no such store".to_string(),
                runtime: "cloudfront-js-2.0",
                code: b"f".to_vec(),
                associations: vec![absent.clone()],
                tags: Vec::new(),
            }
            .config_xml(),
        ),
    ];

    for (case, config) in cases {
        let name = "refused_config";
        let body = format!(
            "<CreateFunctionRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{name}</Name>{config}<FunctionCode>{}</FunctionCode>\
             </CreateFunctionRequest>",
            base64(b"f")
        );
        let answer = create_function(&router, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );

        let described = describe_function(&router, name, None).await;
        assert_eq!(
            described.status, 404,
            "case {case}: the refused create must store nothing, got {} with {}",
            described.status, described.raw
        );
    }
}

/// A tag set no resource can carry is refused with the modelled `InvalidArgument` (HTTP
/// 400) and stores nothing: the service's one tagging rule — at most one tag per key, at
/// most `MAX_TAGS_PER_RESOURCE` tags — holds for a function as it does for every other
/// resource kind this service tags.
#[tokio::test]
async fn a_tag_set_no_resource_can_carry_is_refused() {
    let router = emulator();

    // Rows: one key named twice (so the request describes no tag set at all), and one
    // tag past the per-resource maximum.
    let repeated: Vec<(String, String)> = vec![
        ("stage".to_string(), "prod".to_string()),
        ("stage".to_string(), "staging".to_string()),
    ];
    let too_many: Vec<(String, String)> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key{index}"), index.to_string()))
        .collect();
    let cases: [(&str, Vec<(String, String)>); 2] = [
        ("one key named twice", repeated),
        ("one tag past the per-resource maximum", too_many),
    ];

    for (case, tags) in cases {
        let name = "refused_tags";
        let mut items = String::new();
        for (key, value) in &tags {
            items.push_str(&format!(
                "<Tag><Key>{}</Key><Value>{}</Value></Tag>",
                escape(key),
                escape(value)
            ));
        }
        let body = format!(
            "<CreateFunctionRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{name}</Name>\
             <FunctionConfig><Comment>tagged</Comment>\
             <Runtime>cloudfront-js-2.0</Runtime></FunctionConfig>\
             <FunctionCode>{}</FunctionCode><Tags><Items>{items}</Items></Tags>\
             </CreateFunctionRequest>",
            base64(b"f")
        );

        let answer = create_function(&router, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for {} tags, got {} with {}",
            tags.len(),
            answer.status,
            answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument, got {}",
            answer.raw
        );

        let described = describe_function(&router, name, None).await;
        assert_eq!(
            described.status, 404,
            "case {case}: the refused create must store nothing, got {} with {}",
            described.status, described.raw
        );
    }
}

/// A function's ARN names ONE tag set to every tagging call: a tag written by
/// `TagResource` is read back by `ListTagsForResource`, beside the tags the create
/// carried — so the write path and the read path resolve the ARN to the same record.
#[tokio::test]
async fn a_function_arn_names_one_tag_set_to_every_tagging_call() {
    let router = emulator();

    // Rows: a function created WITH tags and one created without, so the union holds
    // whether or not the create seeded the set.
    struct TaggingCase {
        case: &'static str,
        name: &'static str,
        /// The tags the create carries, beside which the written one must be read back.
        tags: Vec<(&'static str, &'static str)>,
    }
    let cases = [
        TaggingCase {
            case: "created with tags",
            name: "tagged_at_create",
            tags: vec![("stage", "prod")],
        },
        TaggingCase {
            case: "created without tags",
            name: "tagged_afterwards",
            tags: Vec::new(),
        },
    ];

    for TaggingCase { case, name, tags } in cases {
        let create = Create {
            case: "tagging",
            name: name.to_string(),
            comment: "tagged through the tagging surface".to_string(),
            runtime: "cloudfront-js-2.0",
            code: b"f".to_vec(),
            associations: Vec::new(),
            tags: tags.clone(),
        };
        let created = create_function(&router, &create.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );
        let arn = created
            .root()
            .text_at("FunctionMetadata/FunctionARN")
            .unwrap_or_else(|| panic!("case {case}: the create carries an ARN: {}", created.raw))
            .to_string();

        let tagged = tag_resource(&router, &arn, "owner", "edge-team").await;
        assert_eq!(
            tagged.status, 204,
            "case {case}: expected HTTP 204 from the tag write, got {} with {}",
            tagged.status, tagged.raw
        );

        let read = list_tags_for_resource(&router, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from the tag read, got {} with {}",
            read.status, read.raw
        );
        let mut expected: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        expected.push(("owner".to_string(), "edge-team".to_string()));
        expected.sort();
        assert_eq!(
            function_tags(&read),
            expected,
            "case {case}: expected the create's tags plus the written one, got {}",
            read.raw
        );
    }
}

/// The `(Key, Value)` pairs a `Tags` answer reports, in the order the document lists
/// them (the store orders them by key).
fn function_tags(answer: &Answer) -> Vec<(String, String)> {
    answer
        .root()
        .at("Items")
        .map(|items| {
            items
                .children_named("Tag")
                .iter()
                .map(|tag| {
                    (
                        tag.text_at("Key").unwrap_or_default().to_string(),
                        tag.text_at("Value").unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}
