// SPDX-License-Identifier: CC0-1.0

//! The CloudFront CONNECTION FUNCTION control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are:
//!
//! * a connection function is PERSISTED by the call that creates it, which MINTS its
//!   identity (its `Id`, the ARN built from that id, and the version token of its
//!   `DEVELOPMENT` stage), so a later `DescribeConnectionFunction` reports that record's
//!   configuration, status, stage and timestamps rather than the request being served —
//!   whether the read addresses the resource by the minted id or by the name it was created
//!   under;
//! * a created connection function is in the `DEVELOPMENT` stage and the `LIVE` stage
//!   exists only once a publish has copied `DEVELOPMENT` into it, so a read of the `LIVE`
//!   stage of a connection function that was only created is an entity that is not found;
//! * "The entity already exists. You must provide a unique entity": a name identifies ONE
//!   connection function for its whole life, so a repeated name is refused and leaves the
//!   record that holds it untouched;
//! * a create the service cannot hold — code past the published function-code quota, more
//!   connection functions than the account quota admits, a list wrapper whose `Quantity`
//!   disagrees with its `Items`, more key value store associations than one function may
//!   carry, an association naming a key value store this service does not hold, or a tag set
//!   no resource can carry — is refused with the error the model names for it, and stores
//!   nothing.
//!
//! The table rows differ along everything these rules generalise over: the name's length
//! (both published bounds of `FunctionName`, and the character classes its pattern admits),
//! the runtime (every value the modelled `FunctionRuntime` enum admits), the comment's
//! length and whether it is ASCII, the code's size (the modelled minimum, a real script,
//! and the published quota bound) and whether the code is text at all (a `FunctionBlob` is
//! a blob, so arbitrary bytes have to survive the round trip), whether the configuration
//! associates a key value store, and whether the create carries tags.

use std::collections::BTreeSet;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    FUNCTION_STAGE_DEVELOPMENT, FUNCTION_STAGE_LIVE, FUNCTION_STATUS_UNPUBLISHED,
    MAX_CONNECTION_FUNCTIONS, MAX_CONNECTION_FUNCTION_CODE_BYTES,
    MAX_KEY_VALUE_STORE_ASSOCIATIONS, MAX_TAGS_PER_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    call, emulator, is_version_token, Answer, Node, ACCOUNT_ID, API_VERSION, HOST,
};
// The tagging helpers every suite of this crate shares: one copy, in the suite that owns
// the tagging surface, rather than a copy per suite.
use crate::distribution_tags::{list_tags_for_resource, reported_tags, tags_xml, SubmittedTag};
// The key value store create a configuration's association needs, from the suite that owns
// that surface.
use crate::key_value_store_lifecycle::create_key_value_store;
// The XML-entity escaper every suite of this crate sends request documents through.
use crate::response_headers_policy_lifecycle::escape;

// ---------------------------------------------------------------------------
// Calling the connection function surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/connection-function` with the request document `body`.
pub(crate) async fn create_connection_function(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/connection-function"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/connection-function/{Identifier}/describe`, naming `stage` when it is
/// `Some` and leaving the optional query parameter out entirely when it is `None`.
pub(crate) async fn describe_connection_function(
    router: &MultiServiceRouter,
    identifier: &str,
    stage: Option<&str>,
) -> Answer {
    let uri = match stage {
        Some(stage) => {
            format!("/{API_VERSION}/connection-function/{identifier}/describe?Stage={stage}")
        }
        None => format!("/{API_VERSION}/connection-function/{identifier}/describe"),
    };
    call(router, "GET", &uri, None, None).await
}

/// Create a key value store and answer the ARN its create minted, so a connection function
/// configuration can name a store this service actually holds.
pub(crate) async fn key_value_store_arn(router: &MultiServiceRouter, name: &str) -> String {
    let body = format!(
        "<CreateKeyValueStoreRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name></CreateKeyValueStoreRequest>"
    );
    let answer = create_key_value_store(router, &body).await;
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

// ---------------------------------------------------------------------------
// The creates under test
// ---------------------------------------------------------------------------

/// One `CreateConnectionFunction` request a client submits.
#[derive(Debug, Clone)]
pub(crate) struct Create {
    pub(crate) case: &'static str,
    pub(crate) name: String,
    /// `FunctionConfig.Comment`, which is required on the wire.
    pub(crate) comment: String,
    /// `FunctionConfig.Runtime`.
    pub(crate) runtime: &'static str,
    /// `ConnectionFunctionCode`, as the bytes a client submits.
    pub(crate) code: Vec<u8>,
    /// The `KeyValueStoreARN` of each association the configuration names.
    pub(crate) associations: Vec<String>,
    /// `Tags.Items`.
    pub(crate) tags: Vec<SubmittedTag>,
}

impl Create {
    /// The `CreateConnectionFunctionRequest` XML a client sends for this create.
    pub(crate) fn xml(&self) -> String {
        let mut out = format!(
            "<CreateConnectionFunctionRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
        );
        out.push_str(&format!("<Name>{}</Name>", escape(&self.name)));
        out.push_str(&self.config_xml());
        out.push_str(&format!(
            "<ConnectionFunctionCode>{}</ConnectionFunctionCode>",
            // The padded standard base64 a restXml blob MEMBER travels as, through the
            // runtime's own encoder.
            ::aws_smithy_types::base64::encode(&self.code)
        ));
        if !self.tags.is_empty() {
            out.push_str(&tags_xml(Some(&self.tags)));
        }
        out.push_str("</CreateConnectionFunctionRequest>");
        out
    }

    /// The `ConnectionFunctionConfig` element this create carries.
    fn config_xml(&self) -> String {
        let mut out = format!(
            "<ConnectionFunctionConfig><Comment>{}</Comment><Runtime>{}</Runtime>",
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
        out.push_str("</ConnectionFunctionConfig>");
        out
    }
}

/// A connection function body — the shape of code this surface carries, including the
/// characters XML predefines, so the round trip is tested on code a client really sends.
const CONNECTION_CODE: &str = "function handler(event) {\n\
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
/// `cloudfront-js-2.0` (all three from `smithy_query` on `CreateConnectionFunction` and
/// `FunctionRuntime`). So the rows sit at both name bounds, draw from each character class
/// the name pattern admits, use each runtime the enum admits, and carry the smallest code
/// the model allows, a real script, code that is not UTF-8 at all, and code exactly at the
/// published function-code quota.
///
/// `kvs_arn` is the ARN of a key value store this service holds, so the row that associates
/// one names a store that really exists.
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
            case: "the longest admissible name (64), a real connection script",
            name: "n".repeat(64),
            comment: "rewrites the URI and a header".to_string(),
            runtime: "cloudfront-js-2.0",
            code: CONNECTION_CODE.as_bytes().to_vec(),
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
            name: "Connection42".to_string(),
            comment: "c".repeat(128),
            runtime: "cloudfront-js-2.0",
            code: vec![b'x'; MAX_CONNECTION_FUNCTION_CODE_BYTES],
            associations: Vec::new(),
            tags: Vec::new(),
        },
        Create {
            case: "a key value store association, with tags",
            name: "catalog_lookup".to_string(),
            comment: "reads the catalog from a key value store".to_string(),
            runtime: "cloudfront-js-2.0",
            code: CONNECTION_CODE.as_bytes().to_vec(),
            associations: vec![kvs_arn.to_string()],
            tags: vec![
                ("stage".to_string(), Some("prod".to_string())),
                ("owner".to_string(), Some("edge-team".to_string())),
            ],
        },
    ]
}

// ---------------------------------------------------------------------------
// Reading what the service stored
// ---------------------------------------------------------------------------

/// A connection function id's shape: `cf_` followed by 27 base62 characters, the width and
/// alphabet CloudFront writes an opaque namespaced id in (`dt_3JO1OQa51Jb3BZh7XQh6pbvF3Wa`
/// is one of its siblings). Checked here rather than through the crate's own constant so a
/// wrong constant cannot make this test pass. `ResourceId` is `length 1..64`, which the
/// shape obeys.
fn is_connection_function_id(value: &str) -> bool {
    match value.strip_prefix("cf_") {
        Some(body) => {
            value.len() <= 64
                && body.len() == 27
                && body.chars().all(|c| c.is_ascii_alphanumeric())
        }
        None => false,
    }
}

/// The text of one member of the `ConnectionFunctionSummary` document, or a panic naming
/// the case.
pub(crate) fn member(summary: &Node, path: &str, case: &str, raw: &str) -> String {
    summary
        .text_at(path)
        .unwrap_or_else(|| panic!("case {case}: the answer carries no {path}: {raw}"))
        .to_string()
}

/// Assert the `ConnectionFunctionSummary` document `answer` carries is the connection
/// function `create` asked for, in `stage`, and answer the `(Id, ARN)` it reports.
fn assert_summary(
    answer: &Answer,
    create: &Create,
    stage: &str,
    context: &str,
) -> (String, String) {
    let case = create.case;
    let summary = answer.root();
    let raw = answer.raw.as_str();

    let name = member(summary, "Name", case, raw);
    assert_eq!(
        name, create.name,
        "case {case} ({context}): expected the submitted Name, got {name:?}"
    );

    let id = member(summary, "Id", case, raw);
    assert!(
        is_connection_function_id(&id),
        "case {case} ({context}): expected a minted connection function id, got {id:?}"
    );

    let status = member(summary, "Status", case, raw);
    assert_eq!(
        status, FUNCTION_STATUS_UNPUBLISHED,
        "case {case} ({context}): expected Status {FUNCTION_STATUS_UNPUBLISHED}, got {status:?}"
    );

    let comment = summary
        .text_at("ConnectionFunctionConfig/Comment")
        .unwrap_or("")
        .to_string();
    assert_eq!(
        comment, create.comment,
        "case {case} ({context}): expected the submitted Comment, got {comment:?}"
    );

    let runtime = member(summary, "ConnectionFunctionConfig/Runtime", case, raw);
    assert_eq!(
        runtime, create.runtime,
        "case {case} ({context}): expected the submitted Runtime, got {runtime:?}"
    );

    let associated: Vec<String> = summary
        .at("ConnectionFunctionConfig/KeyValueStoreAssociations/Items")
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

    let reported_stage = member(summary, "Stage", case, raw);
    assert_eq!(
        reported_stage, stage,
        "case {case} ({context}): expected Stage {stage}, got {reported_stage:?}"
    );

    let created = member(summary, "CreatedTime", case, raw);
    let modified = member(summary, "LastModifiedTime", case, raw);
    assert!(
        !created.is_empty() && !modified.is_empty(),
        "case {case} ({context}): expected both timestamps, got {created:?} and {modified:?}"
    );

    let arn = member(summary, "ConnectionFunctionArn", case, raw);
    assert_eq!(
        arn,
        format!("arn:aws:cloudfront::{ACCOUNT_ID}:connection-function/{id}"),
        "case {case} ({context}): expected the ARN built from the minted Id, got {arn:?}"
    );
    (id, arn)
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create PERSISTS the connection function it mints, and the describe answers from that
/// one record: the submitted configuration, the `DEVELOPMENT` stage, the unpublished status
/// and the version token the create handed back — read by the minted id and by the name
/// alike. Across the table, no two connection functions share an id, an ARN or a version
/// token.
#[tokio::test]
async fn create_persists_the_connection_function_and_the_describe_reports_it() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_function_table_store").await;
    let mut ids = BTreeSet::new();
    let mut arns = BTreeSet::new();
    let mut etags = BTreeSet::new();

    for create in create_table(&kvs_arn) {
        let case = create.case;
        let created = create_connection_function(&router, &create.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );

        let (id, arn) = assert_summary(&created, &create, FUNCTION_STAGE_DEVELOPMENT, "the create");
        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token in the ETag header, got {etag:?}"
        );
        let location = created.header("location").to_string();
        assert_eq!(
            location,
            format!("https://{HOST}/{API_VERSION}/connection-function/{id}"),
            "case {case}: expected the new connection function's URI in Location, got \
             {location:?}"
        );
        assert!(ids.insert(id.clone()), "case {case}: the id {id} repeated");
        assert!(arns.insert(arn.clone()), "case {case}: the ARN {arn} repeated");
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );

        // The describe answers from the stored record, not from any request — addressed by
        // the minted id and by the name the create was given.
        let described = describe_connection_function(&router, &id, None).await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected HTTP 200 from the describe, got {} with {}",
            described.status, described.raw
        );
        let (described_id, described_arn) =
            assert_summary(&described, &create, FUNCTION_STAGE_DEVELOPMENT, "the describe");
        assert_eq!(
            (described_id.as_str(), described_arn.as_str()),
            (id.as_str(), arn.as_str()),
            "case {case}: the describe must report the identity the create minted"
        );
        assert_eq!(
            described.header("etag"),
            etag,
            "case {case}: expected the create's version token from the describe"
        );

        let by_name = describe_connection_function(&router, &create.name, None).await;
        assert_eq!(
            by_name.raw, described.raw,
            "case {case}: naming the connection function by name must read the same record"
        );

        // Naming the stage explicitly is the same read as naming none.
        let explicit =
            describe_connection_function(&router, &id, Some(FUNCTION_STAGE_DEVELOPMENT)).await;
        assert_eq!(
            explicit.raw, described.raw,
            "case {case}: naming the DEVELOPMENT stage must read the same record"
        );

        // The tags submitted with the create are the connection function's tags, reached
        // through the ARN the create minted.
        let tags = list_tags_for_resource(&router, &arn).await;
        assert_eq!(
            tags.status, 200,
            "case {case}: expected HTTP 200 from the tag read, got {} with {}",
            tags.status, tags.raw
        );
        let read = reported_tags(tags.root());
        let expected: std::collections::BTreeMap<String, Option<String>> =
            create.tags.iter().cloned().collect();
        assert_eq!(
            read, expected,
            "case {case}: expected the submitted tags on the connection function, got {read:?}"
        );
    }
}

/// A created connection function is in the `DEVELOPMENT` stage: the `LIVE` stage exists
/// only once a publish has copied `DEVELOPMENT` into it, so every read of the `LIVE` stage
/// of a connection function that was only created is an entity that is not found — while
/// the same read of `DEVELOPMENT` answers.
#[tokio::test]
async fn the_live_stage_exists_only_after_a_publish() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_live_stage_store").await;

    for create in create_table(&kvs_arn) {
        let case = create.case;
        let created = create_connection_function(&router, &create.xml()).await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );
        let id = created
            .root()
            .text_at("Id")
            .unwrap_or_else(|| panic!("case {case}: the create carries an Id: {}", created.raw))
            .to_string();

        let live = describe_connection_function(&router, &id, Some(FUNCTION_STAGE_LIVE)).await;
        assert_eq!(
            live.status, 404,
            "case {case}: expected HTTP 404 describing the LIVE stage, got {} with {}",
            live.status, live.raw
        );
        assert_eq!(
            live.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound, got {}",
            live.raw
        );

        // The DEVELOPMENT stage is there all the same.
        let development =
            describe_connection_function(&router, &id, Some(FUNCTION_STAGE_DEVELOPMENT)).await;
        assert_eq!(
            development.status, 200,
            "case {case}: expected HTTP 200 describing DEVELOPMENT, got {} with {}",
            development.status, development.raw
        );
    }
}

/// "The entity already exists. You must provide a unique entity": the second create under a
/// name is refused with `EntityAlreadyExists` (HTTP 409), and the connection function that
/// holds the name keeps the configuration and the version token its own create stored.
#[tokio::test]
async fn a_name_names_at_most_one_connection_function() {
    let router = emulator();
    let kvs_arn = key_value_store_arn(&router, "connection_duplicate_name_store").await;

    for create in create_table(&kvs_arn) {
        let case = create.case;
        let first = create_connection_function(&router, &create.xml()).await;
        assert_eq!(
            first.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            first.status, first.raw
        );
        let etag = first.header("etag").to_string();
        let id = first
            .root()
            .text_at("Id")
            .unwrap_or_else(|| panic!("case {case}: the create carries an Id: {}", first.raw))
            .to_string();

        // The same name again — with a different comment, runtime and code, so a record
        // that was replaced rather than refused would be visible.
        let repeat = Create {
            comment: "a second create under a name already taken".to_string(),
            runtime: "cloudfront-js-1.0",
            code: b"function handler(event) { return event.request; }".to_vec(),
            associations: Vec::new(),
            tags: Vec::new(),
            ..create.clone()
        };
        let second = create_connection_function(&router, &repeat.xml()).await;
        assert_eq!(
            second.status, 409,
            "case {case}: expected HTTP 409 for a repeated name, got {} with {}",
            second.status, second.raw
        );
        assert_eq!(
            second.error_code(),
            "EntityAlreadyExists",
            "case {case}: expected EntityAlreadyExists, got {}",
            second.raw
        );

        let described = describe_connection_function(&router, &id, None).await;
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
    }
}

/// An identifier no connection function answers to is not found — `EntityNotFound` (HTTP
/// 404) — whatever shape the identifier takes, and a create under one name leaves every
/// other identifier unfound.
#[tokio::test]
async fn an_identifier_no_connection_function_answers_to_is_not_found() {
    let router = emulator();
    let present = Create {
        case: "the one connection function that exists",
        name: "present_connection_function".to_string(),
        comment: "the only one".to_string(),
        runtime: "cloudfront-js-2.0",
        code: CONNECTION_CODE.as_bytes().to_vec(),
        associations: Vec::new(),
        tags: Vec::new(),
    };
    let created = create_connection_function(&router, &present.xml()).await;
    assert_eq!(
        created.status, 201,
        "case {}: expected HTTP 201, got {} with {}",
        present.case, created.status, created.raw
    );

    // Rows: a name never created, the same name in another case (identifiers are compared
    // as given), a prefix of the name that exists, the longest name the model admits, and
    // an id in the shape this service mints that was never allocated.
    let absent: [(&str, String); 5] = [
        ("a name never created", "absent_connection".to_string()),
        (
            "the same name in another case",
            "PRESENT_CONNECTION_FUNCTION".to_string(),
        ),
        ("a prefix of the existing name", "present".to_string()),
        ("the longest admissible name", "z".repeat(64)),
        ("a minted-shaped id never allocated", "cf_0000000000000".to_string()),
    ];
    for (case, identifier) in absent {
        let described = describe_connection_function(&router, &identifier, None).await;
        assert_eq!(
            described.status, 404,
            "case {case}: expected HTTP 404 for {identifier:?}, got {} with {}",
            described.status, described.raw
        );
        assert_eq!(
            described.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound for {identifier:?}, got {}",
            described.raw
        );
    }
}

/// CloudFront's published quota bounds the connection function code: code exactly at the
/// bound is accepted, code past it is refused with `EntitySizeLimitExceeded` (HTTP 413), and
/// code past the MODELLED bound (`FunctionBlob`, `length 1..40960`) is refused at the
/// boundary with HTTP 400. Every refusal stores nothing, so the name it named stays unused.
#[tokio::test]
async fn code_past_the_published_quota_is_refused() {
    let router = emulator();

    // Rows: the published bound itself, one byte past it, and one byte past the modelled
    // maximum of `FunctionBlob`.
    let cases: [(&str, usize, u16, &str); 3] = [
        (
            "exactly the published quota",
            MAX_CONNECTION_FUNCTION_CODE_BYTES,
            201,
            "",
        ),
        (
            "one byte past the published quota",
            MAX_CONNECTION_FUNCTION_CODE_BYTES + 1,
            413,
            "EntitySizeLimitExceeded",
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
        let answer = create_connection_function(&router, &create.xml()).await;
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

        let described = describe_connection_function(&router, &name, None).await;
        let expected_read = if status == 201 { 200 } else { 404 };
        assert_eq!(
            described.status, expected_read,
            "case {case}: expected HTTP {expected_read} from the describe, got {} with {}",
            described.status, described.raw
        );
    }
}

/// The account quota bounds how many connection functions one account holds: the create at
/// the bound is accepted and the one past it is refused with `EntityLimitExceeded`, which
/// stores nothing — so the name it named stays unused and every record already stored is
/// untouched.
#[tokio::test]
async fn a_create_past_the_account_quota_is_refused() {
    let router = emulator();

    // The quota is an entry count, so the rows that matter are the last create within it
    // and the first one past it.
    for index in 0..MAX_CONNECTION_FUNCTIONS {
        let create = Create {
            case: "filling the account quota",
            name: format!("quota_{index}"),
            comment: format!("connection function {index}"),
            runtime: "cloudfront-js-2.0",
            code: b"f".to_vec(),
            associations: Vec::new(),
            tags: Vec::new(),
        };
        let answer = create_connection_function(&router, &create.xml()).await;
        assert_eq!(
            answer.status, 201,
            "case within the quota: expected HTTP 201 for connection function {index}, got {} \
             with {}",
            answer.status, answer.raw
        );
    }

    let over = Create {
        case: "one connection function past the account quota",
        name: "quota_past".to_string(),
        comment: "one too many".to_string(),
        runtime: "cloudfront-js-2.0",
        code: b"f".to_vec(),
        associations: Vec::new(),
        tags: Vec::new(),
    };
    let answer = create_connection_function(&router, &over.xml()).await;
    assert_eq!(
        answer.status, 400,
        "case {}: expected HTTP 400, got {} with {}",
        over.case, answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "EntityLimitExceeded",
        "case {}: expected EntityLimitExceeded, got {}",
        over.case,
        answer.raw
    );

    let described = describe_connection_function(&router, &over.name, None).await;
    assert_eq!(
        described.status, 404,
        "case {}: the refused create must store nothing, got {} with {}",
        over.case, described.status, described.raw
    );
    // The connection functions already stored are still there, unchanged.
    let first = describe_connection_function(&router, "quota_0", None).await;
    assert_eq!(
        first.status, 200,
        "case {}: the refused create must leave the stored records alone, got {} with {}",
        over.case, first.status, first.raw
    );
}

/// A configuration this service cannot hold is refused with the modelled `InvalidArgument`
/// (HTTP 400) and stores nothing: a `KeyValueStoreAssociations` wrapper whose `Quantity`
/// disagrees with its `Items`, more associations than one function may carry, and an
/// association naming a key value store this service does not hold.
#[tokio::test]
async fn a_configuration_the_service_cannot_hold_is_refused() {
    let router = emulator();
    let held = key_value_store_arn(&router, "connection_config_store").await;
    // One more store than a function may be associated with, so the row past the cap names
    // stores that all really exist and is refused for its COUNT alone.
    let mut over_the_cap = Vec::new();
    for index in 0..=MAX_KEY_VALUE_STORE_ASSOCIATIONS {
        over_the_cap.push(key_value_store_arn(&router, &format!("connection_cap_{index}")).await);
    }
    // A well-formed key value store ARN that no store in this service holds.
    let absent = format!(
        "arn:aws:cloudfront::{ACCOUNT_ID}:key-value-store/\
         00000000-0000-4000-8000-000000000000"
    );

    // Rows: a wrapper that overstates its length, one that understates it, one association
    // past the published per-function cap, and one association naming a store that does not
    // exist.
    let overstated = format!(
        "<ConnectionFunctionConfig><Comment>overstated</Comment>\
         <Runtime>cloudfront-js-2.0</Runtime>\
         <KeyValueStoreAssociations><Quantity>2</Quantity><Items>\
         <KeyValueStoreAssociation><KeyValueStoreARN>{held}</KeyValueStoreARN>\
         </KeyValueStoreAssociation></Items></KeyValueStoreAssociations>\
         </ConnectionFunctionConfig>"
    );
    let understated = format!(
        "<ConnectionFunctionConfig><Comment>understated</Comment>\
         <Runtime>cloudfront-js-2.0</Runtime>\
         <KeyValueStoreAssociations><Quantity>0</Quantity><Items>\
         <KeyValueStoreAssociation><KeyValueStoreARN>{held}</KeyValueStoreARN>\
         </KeyValueStoreAssociation></Items></KeyValueStoreAssociations>\
         </ConnectionFunctionConfig>"
    );
    let cases: [(&str, String); 4] = [
        ("a Quantity above the Items length", overstated),
        ("a Quantity below the Items length", understated),
        (
            "one association past the published per-function cap",
            Create {
                case: "too many associations",
                name: "too_many_associations".to_string(),
                comment: "one store too many".to_string(),
                runtime: "cloudfront-js-2.0",
                code: b"f".to_vec(),
                associations: over_the_cap.clone(),
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
            "<CreateConnectionFunctionRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{name}</Name>{config}\
             <ConnectionFunctionCode>{}</ConnectionFunctionCode>\
             </CreateConnectionFunctionRequest>",
            ::aws_smithy_types::base64::encode(b"f")
        );
        let answer = create_connection_function(&router, &body).await;
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

        let described = describe_connection_function(&router, name, None).await;
        assert_eq!(
            described.status, 404,
            "case {case}: the refused create must store nothing, got {} with {}",
            described.status, described.raw
        );
    }
}

/// A tag set no resource can carry is refused with the modelled `InvalidTagging` (HTTP 400)
/// and stores nothing: the service's one tagging rule — at most one tag per key, at most
/// `MAX_TAGS_PER_RESOURCE` tags — holds for a connection function as it does for every
/// other resource kind this service tags.
#[tokio::test]
async fn a_tag_set_no_resource_can_carry_is_refused() {
    let router = emulator();

    // Rows: one key named twice (so the request describes no tag set at all), and one tag
    // past the per-resource maximum.
    let repeated: Vec<SubmittedTag> = vec![
        ("stage".to_string(), Some("prod".to_string())),
        ("stage".to_string(), Some("staging".to_string())),
    ];
    let too_many: Vec<SubmittedTag> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key{index}"), Some(index.to_string())))
        .collect();
    let cases: [(&str, Vec<SubmittedTag>); 2] = [
        ("one key named twice", repeated),
        ("one tag past the per-resource maximum", too_many),
    ];

    for (case, tags) in cases {
        let name = "refused_tags";
        let create = Create {
            case: "a refused tag set",
            name: name.to_string(),
            comment: "tagged".to_string(),
            runtime: "cloudfront-js-2.0",
            code: b"f".to_vec(),
            associations: Vec::new(),
            tags: tags.clone(),
        };

        let answer = create_connection_function(&router, &create.xml()).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for {} tags, got {} with {}",
            tags.len(),
            answer.status,
            answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidTagging",
            "case {case}: expected InvalidTagging, got {}",
            answer.raw
        );

        let described = describe_connection_function(&router, name, None).await;
        assert_eq!(
            described.status, 404,
            "case {case}: the refused create must store nothing, got {} with {}",
            described.status, described.raw
        );
    }
}
