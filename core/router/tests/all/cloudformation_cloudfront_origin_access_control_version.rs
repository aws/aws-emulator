// SPDX-License-Identifier: CC0-1.0

//! The origin access control a CloudFormation stack reports created is a full CloudFront
//! resource under the physical id the stack published: CloudFront's own read answers it with
//! the configuration the template declared AND the version token that service issues, and
//! that token is what its conditional update evaluates.
//!
//! The rule under test: **a resource a stack reports created in another service exists in
//! that service under the physical resource id the stack published — so that service's own
//! read answers it with the configuration the template declared and with the current version
//! token the service issues for it, and that token drives the service's own conditional
//! update exactly as it does for a resource created through that service's API: the token the
//! read answered is accepted, and a token the resource has moved past is refused with the
//! service's precondition refusal.** Two crates have to agree for that to hold —
//! CloudFormation makes the create and publishes the id, CloudFront mints the id, holds the
//! configuration and issues the version — so the suite drives both over their own wires.
//!
//! What a client does with it: an origin access control is only usable through CloudFront
//! (an origin names it by id, and rotating its configuration takes `If-Match`), so a stack
//! that published an id that service does not resolve, or a read that answers without a
//! version, leaves the deployment with nothing to rotate.
//!
//! The rows differ along what the rule generalises over: every value of the configuration's
//! three enumerations appears at least once, the optional `Description` spans absent, empty,
//! plain ASCII and text carrying XML metacharacters, `Name` spans an ordinary name and one at
//! its published 64-character bound, and the template spelling spans JSON and YAML short
//! form. Every expected value is computed here from what the row declares, except the id and
//! the version token, which are CloudFront's to mint: of those, what is asserted is the FORM
//! of the id, that one value appears everywhere the stack reports the resource's identity and
//! that CloudFront resolves it, and that the token the read answered is the one the update
//! accepts.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this test crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{
    block_of, elements, form_encode, opt_element, outputs_of, physical_ids_of, xml_escape,
};

/// The Region every call here is signed for. CloudFront is global; the credential scope is
/// what routes each request and what names the Region the stack's resources belong to.
const REGION: &str = "us-east-1";

/// CloudFront's control-plane host, and the API version its routes live under.
const CLOUDFRONT_HOST: &str = "cloudfront.amazonaws.com";
const API_VERSION: &str = "2020-05-31";

/// The resource type every row declares.
const CONTROL_TYPE: &str = "AWS::CloudFront::OriginAccessControl";

/// The `StackStatus` each operation here settles on.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// How long a stack operation is given to reach its terminal status, and how often it is read
/// while waiting. GENEROUS: what is waited for is the CONDITION the stack reports, polled
/// until it holds, so this bound only decides when a deployment that never settles is
/// reported as a failure.
const SETTLES_WITHIN: Duration = Duration::from_secs(90);
const POLL_EVERY: Duration = Duration::from_millis(50);

/// How many characters follow the `E` of a CloudFront-assigned identifier: an origin access
/// control id is an `E` and 13 more uppercase alphanumerics, the form
/// `CloudFrontOriginAccessIdentity$Id` documents by example ("E74FTE3AJFJ256A").
const CLOUDFRONT_ID_BODY: usize = 13;

/// `OriginAccessControlConfig$Name`: "You can specify up to 64 characters" — the bound one
/// row's name sits exactly on.
const MAX_CONTROL_NAME: usize = 64;

/// `PreconditionFailed`'s message on this surface, as CloudFront words it for a well-formed
/// version token that is not the resource's current one.
const PRECONDITION_FAILED_MESSAGE: &str =
    "The request failed because it didn't meet the preconditions in one or more \
     request-header fields.";

/// `NoSuchOriginAccessControl`'s message, as CloudFront words it for a control it does not
/// hold — which is what a read of a deleted stack's control answers.
const NO_SUCH_CONTROL_MESSAGE: &str = "The specified origin access control does not exist.";

/// Every value of `OriginAccessControlSigningProtocols`
/// (`smithy_query(query="shape",shape="OriginAccessControlSigningProtocols")`).
const SIGNING_PROTOCOLS: [&str; 2] = ["sigv4", "sigv4a"];

/// Every value of `OriginAccessControlSigningBehaviors`
/// (`smithy_query(query="shape",shape="OriginAccessControlSigningBehaviors")`).
const SIGNING_BEHAVIORS: [&str; 4] = ["never", "always", "no-override", "always-amz-auth"];

/// Every value of `OriginAccessControlOriginTypes`
/// (`smithy_query(query="shape",shape="OriginAccessControlOriginTypes")`).
const ORIGIN_TYPES: [&str; 4] = ["s3", "mediastore", "mediapackagev2", "lambda"];

/// The two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process call CloudFormation makes to CloudFront signs itself either way
/// (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in [`REGION`], which
/// is what routes the request and what each handler reads its Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// One answer: the status, the `ETag` header and the body as text.
struct Answer {
    status: u16,
    etag: String,
    body: String,
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    Answer {
        status: response.status().as_u16(),
        etag: response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string(),
        body: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let answer = dispatch(
        router,
        "POST",
        &format!("cloudformation.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization("cloudformation")),
        ],
        Bytes::from(body),
    )
    .await;
    (answer.status, answer.body)
}

/// Call one CloudFront control-plane operation over its restXml wire, optionally with a
/// request document and the `If-Match` version a conditional write presents.
async fn cloudfront(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    document: Option<&str>,
    if_match: Option<&str>,
) -> Answer {
    let authorization = authorization("cloudfront");
    let mut headers: Vec<(&str, &str)> = vec![("authorization", authorization.as_str())];
    if document.is_some() {
        headers.push(("content-type", "application/xml"));
    }
    if let Some(version) = if_match {
        headers.push(("if-match", version));
    }
    dispatch(
        router,
        method,
        CLOUDFRONT_HOST,
        path,
        &headers,
        document
            .map(|body| Bytes::from(body.to_string()))
            .unwrap_or_default(),
    )
    .await
}

/// `GET /2020-05-31/origin-access-control/{Id}` — `GetOriginAccessControl`.
async fn get_control(router: &MultiServiceRouter, id: &str) -> Answer {
    cloudfront(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-access-control/{id}"),
        None,
        None,
    )
    .await
}

/// `GET /2020-05-31/origin-access-control/{Id}/config` — `GetOriginAccessControlConfig`.
async fn get_control_config(router: &MultiServiceRouter, id: &str) -> Answer {
    cloudfront(
        router,
        "GET",
        &format!("/{API_VERSION}/origin-access-control/{id}/config"),
        None,
        None,
    )
    .await
}

/// `PUT /2020-05-31/origin-access-control/{Id}/config` — `UpdateOriginAccessControl`, with the
/// whole configuration and the version the caller read.
async fn update_control(
    router: &MultiServiceRouter,
    id: &str,
    document: &str,
    if_match: &str,
) -> Answer {
    cloudfront(
        router,
        "PUT",
        &format!("/{API_VERSION}/origin-access-control/{id}/config"),
        Some(document),
        Some(if_match),
    )
    .await
}

/// Poll `DescribeStacks` until the stack first reports `wanted`, answering the last body it
/// was read from. The wait is on that CONDITION, looked for again until it holds or
/// [`SETTLES_WITHIN`] passes.
async fn settled_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    wanted: &str,
) -> String {
    let giving_up_at = Instant::now() + SETTLES_WITHIN;
    loop {
        let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks({stack}), got {status} with \
             body {body}"
        );
        let reported = opt_element(&body, "StackStatus").unwrap_or_default();
        if reported == wanted {
            return body;
        }
        assert!(
            Instant::now() < giving_up_at,
            "case {case:?}: expected {stack} to reach {wanted}, last reported {reported:?} in \
             {body}"
        );
        tokio::time::sleep(POLL_EVERY).await;
    }
}

/// Poll `DescribeStacks` until this service no longer reports the stack at all, which is what
/// a completed deletion leaves behind.
async fn deleted_stack(router: &MultiServiceRouter, case: &str, stack: &str) {
    let giving_up_at = Instant::now() + SETTLES_WITHIN;
    loop {
        let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        if status != 200 {
            return;
        }
        let reported = opt_element(&body, "StackStatus").unwrap_or_default();
        assert_ne!(
            reported, "DELETE_FAILED",
            "case {case:?}: expected {stack} to be deleted, got {body}"
        );
        assert!(
            Instant::now() < giving_up_at,
            "case {case:?}: expected {stack} to be deleted, last reported {reported:?} in {body}"
        );
        tokio::time::sleep(POLL_EVERY).await;
    }
}

/// How a row writes its template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelling {
    Json,
    /// YAML with the short-form intrinsics (`!Ref`, `!GetAtt`).
    Yaml,
}

/// One origin access control a template declares.
#[derive(Debug, Clone)]
struct Row {
    case: String,
    /// The logical id the declaration carries, and the stem of its two outputs' keys.
    logical_id: String,
    name: String,
    /// `None` leaves the optional member out of the declaration entirely.
    description: Option<String>,
    signing_protocol: &'static str,
    signing_behavior: &'static str,
    origin_type: &'static str,
    spelling: Spelling,
}

/// The rows: every value of each of the configuration's three enumerations at least once,
/// the description forms (absent, empty, plain ASCII, XML metacharacters) cycled across them,
/// and both template spellings. The XML-metacharacter description is declared in the JSON
/// spelling, where quoting it is the serializer's job rather than this file's.
fn rows() -> Vec<Row> {
    let descriptions: [Option<&str>; 4] = [
        None,
        Some(""),
        Some("the stack's origin access control"),
        Some("markup <oac> & \"quotes\" and 'apostrophes'"),
    ];
    let mut rows: Vec<Row> = Vec::new();
    for (index, behavior) in SIGNING_BEHAVIORS.iter().enumerate() {
        let origin_type = ORIGIN_TYPES[index % ORIGIN_TYPES.len()];
        let protocol = SIGNING_PROTOCOLS[index % SIGNING_PROTOCOLS.len()];
        let description = descriptions[index % descriptions.len()];
        // A description carrying the characters XML marks up is declared in JSON; every
        // other row alternates the spelling, so both carry rows of each shape.
        let spelling = if description.is_some_and(|text| text.contains('"')) || index % 2 == 1 {
            Spelling::Json
        } else {
            Spelling::Yaml
        };
        rows.push(Row {
            case: format!("{behavior} over {origin_type} signed with {protocol}"),
            logical_id: format!("Control{index}"),
            name: format!("stack-oac-{index}-{behavior}-{origin_type}"),
            description: description.map(str::to_string),
            signing_protocol: protocol,
            signing_behavior: behavior,
            origin_type,
            spelling,
        });
    }
    // The published name bound, in the YAML spelling.
    rows.push(Row {
        case: "a name at the published 64-character bound".to_string(),
        logical_id: "ControlAtTheBound".to_string(),
        name: "n".repeat(MAX_CONTROL_NAME),
        description: Some("a name of exactly the documented length".to_string()),
        signing_protocol: "sigv4",
        signing_behavior: "always",
        origin_type: "s3",
        spelling: Spelling::Yaml,
    });
    rows
}

impl Row {
    /// The `OriginAccessControlConfig` members this row declares, as the template writes them
    /// — the names the API takes, which a template spells identically.
    fn declared(&self, description: Option<&str>) -> Vec<(&'static str, String)> {
        let mut members: Vec<(&'static str, String)> = vec![("Name", self.name.clone())];
        if let Some(text) = description {
            members.push(("Description", text.to_string()));
        }
        members.push(("SigningProtocol", self.signing_protocol.to_string()));
        members.push(("SigningBehavior", self.signing_behavior.to_string()));
        members.push((
            "OriginAccessControlOriginType",
            self.origin_type.to_string(),
        ));
        members
    }

    /// The `OriginAccessControlConfig` document a client sends CloudFront's own update for
    /// this control, carrying `description` in place of the declared one.
    fn update_document(&self, description: &str) -> String {
        let members: String = self
            .declared(Some(description))
            .into_iter()
            .map(|(member, value)| format!("<{member}>{}</{member}>", xml_escape(&value)))
            .collect();
        format!(
            "<OriginAccessControlConfig \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">{members}\
             </OriginAccessControlConfig>"
        )
    }

    /// Assert that `config` — an `OriginAccessControlConfig` element CloudFront answered —
    /// reports exactly the configuration `description` goes with.
    fn assert_reports(&self, context: &str, config: &str, description: Option<&str>) {
        for (member, expected) in self.declared(None) {
            let actual = text_of(config, member);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {:?} ({context}): expected {member} {expected:?}, got {actual:?} in \
                 {config}",
                self.case
            );
        }
        // An optional member a declaration left out is reported as the record has it:
        // absent, rather than as an empty element the read invented.
        let actual = optional_text(config, "Description");
        assert_eq!(
            actual.as_deref(),
            description,
            "case {:?} ({context}): expected Description {description:?}, got {actual:?} in \
             {config}",
            self.case
        );
    }
}

/// The text of the first `<name>` element, with the serializer's entity escaping undone — so
/// a value carrying an apostrophe or an ampersand compares as the text a client sent.
fn text_of(xml: &str, name: &str) -> Option<String> {
    elements(xml, name).into_iter().next()
}

/// The text of an element that may be EMPTY, which a serializer may write either as a pair
/// of tags or as one self-closing tag: `Some("")` for both, `None` only where the document
/// carries no such element at all.
fn optional_text(xml: &str, name: &str) -> Option<String> {
    if xml.contains(&format!("<{name}/>")) || xml.contains(&format!("<{name} />")) {
        return Some(String::new());
    }
    text_of(xml, name)
}

/// The `OriginAccessControlConfig` of an answer, read as the block it is so the element's own
/// namespace attribute (which it carries when the configuration IS the payload) is skipped.
fn answered_config(case: &str, context: &str, answer: &Answer) -> String {
    block_of(&answer.body, "OriginAccessControlConfig")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?} ({context}): no OriginAccessControlConfig in {}",
                answer.body
            )
        })
        .to_string()
}

/// The `Code` of a restXml refusal envelope.
fn refusal_code(answer: &Answer) -> String {
    text_of(&answer.body, "Code").unwrap_or_default()
}

/// The `Message` of a restXml refusal envelope, as the sentence the service worded rather
/// than as its escaping.
fn refusal_message(answer: &Answer) -> String {
    text_of(&answer.body, "Message").unwrap_or_default()
}

/// Whether `id` has the form CloudFront assigns an identifier in: `E` followed by
/// [`CLOUDFRONT_ID_BODY`] uppercase alphanumerics.
fn is_cloudfront_id(id: &str) -> bool {
    id.len() == CLOUDFRONT_ID_BODY + 1
        && id.starts_with('E')
        && id[1..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
}

/// The template declaring `rows`, in the JSON spelling with long-form intrinsics: each
/// control, and the two outputs that read its identity back (`Ref` and `Fn::GetAtt … Id`).
fn json_template(rows: &[&Row], description_of: impl Fn(&Row) -> Option<String>) -> String {
    let mut resources = Map::new();
    let mut outputs = Map::new();
    for row in rows {
        let mut config = Map::new();
        for (member, value) in row.declared(description_of(row).as_deref()) {
            config.insert(member.to_string(), json!(value));
        }
        resources.insert(
            row.logical_id.clone(),
            json!({
                "Type": CONTROL_TYPE,
                "Properties": {"OriginAccessControlConfig": Value::Object(config)},
            }),
        );
        outputs.insert(
            format!("{}Ref", row.logical_id),
            json!({"Value": {"Ref": row.logical_id}}),
        );
        outputs.insert(
            format!("{}Id", row.logical_id),
            json!({"Value": {"Fn::GetAtt": [row.logical_id, "Id"]}}),
        );
    }
    json!({"Resources": Value::Object(resources), "Outputs": Value::Object(outputs)}).to_string()
}

/// The template declaring `rows`, in YAML with the short-form intrinsics.
///
/// Every scalar is written as a double-quoted YAML string, so a value carrying a character
/// YAML would otherwise read as syntax travels as the text it is; no row declared in this
/// spelling carries a double quote or a backslash of its own.
fn yaml_template(rows: &[&Row], description_of: impl Fn(&Row) -> Option<String>) -> String {
    let mut template = String::from("Resources:\n");
    for row in rows {
        template.push_str(&format!(
            "  {}:\n    Type: {CONTROL_TYPE}\n    Properties:\n      \
             OriginAccessControlConfig:\n",
            row.logical_id
        ));
        for (member, value) in row.declared(description_of(row).as_deref()) {
            assert!(
                !value.contains('"') && !value.contains('\\'),
                "case {:?}: a YAML-spelled row may not declare {member} {value:?}",
                row.case
            );
            template.push_str(&format!("        {member}: \"{value}\"\n"));
        }
    }
    template.push_str("Outputs:\n");
    for row in rows {
        template.push_str(&format!(
            "  {id}Ref:\n    Value: !Ref {id}\n  {id}Id:\n    Value: !GetAtt {id}.Id\n",
            id = row.logical_id
        ));
    }
    template
}

/// The template for one spelling's rows, with each row's declared description.
fn template_for(rows: &[&Row], spelling: Spelling) -> String {
    match spelling {
        Spelling::Json => json_template(rows, |row| row.description.clone()),
        Spelling::Yaml => yaml_template(rows, |row| row.description.clone()),
    }
}

/// The stack each spelling deploys under.
fn stack_name(suite: &str, spelling: Spelling) -> String {
    match spelling {
        Spelling::Json => format!("{suite}-json"),
        Spelling::Yaml => format!("{suite}-yaml"),
    }
}

/// Deploy every row of one spelling and answer the physical id each logical id got, having
/// held the stack to reporting one id for each of them, everywhere it reports their identity.
async fn deployed(
    router: &MultiServiceRouter,
    suite: &str,
    rows: &[Row],
    spelling: Spelling,
) -> Vec<(Row, String)> {
    let declared: Vec<&Row> = rows.iter().filter(|row| row.spelling == spelling).collect();
    let stack = stack_name(suite, spelling);
    let template = template_for(&declared, spelling);
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", &stack), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {stack:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    let described = settled_stack(router, &stack, &stack, CREATE_COMPLETE).await;
    let outputs = outputs_of(&described);

    let (status, resources) =
        cloudformation(router, "DescribeStackResources", &[("StackName", &stack)]).await;
    assert_eq!(
        status, 200,
        "case {stack:?}: expected HTTP 200 from DescribeStackResources, got {status} with body \
         {resources}"
    );
    let physical = physical_ids_of(&resources);

    let mut deployed = Vec::new();
    for row in declared {
        let case = &row.case;
        let id = physical.get(&row.logical_id).cloned().unwrap_or_else(|| {
            panic!("case {case:?}: no physical id for {} in {resources}", row.logical_id)
        });
        assert!(
            is_cloudfront_id(&id),
            "case {case:?}: expected the physical id to be one CloudFront minted (E and \
             {CLOUDFRONT_ID_BODY} more uppercase alphanumerics), got {id:?}"
        );
        for key in [
            format!("{}Ref", row.logical_id),
            format!("{}Id", row.logical_id),
        ] {
            assert_eq!(
                outputs.get(&key),
                Some(&id),
                "case {case:?}: expected output {key} to be the physical id {id:?}, got {:?}",
                outputs.get(&key)
            );
        }
        deployed.push((row.clone(), id));
    }
    deployed
}

/// A stack-created origin access control is readable in CloudFront by the id the stack
/// published, with the configuration the template declared and a version token — and that
/// token is the one that service's conditional update accepts, while the token the control has
/// moved past is refused with its precondition refusal.
#[tokio::test]
async fn a_stack_created_control_reads_back_with_a_usable_version_in_cloudfront() {
    let router = build_router();
    let rows = rows();
    let mut deployed_rows = Vec::new();
    for spelling in [Spelling::Yaml, Spelling::Json] {
        deployed_rows.extend(deployed(&router, "oac-version", &rows, spelling).await);
    }
    assert_eq!(
        deployed_rows.len(),
        rows.len(),
        "every row must be deployed once, got {}",
        deployed_rows.len()
    );

    for (row, id) in &deployed_rows {
        let case = &row.case;
        let declared = row.description.as_deref();

        // The read answers the control under the stack's own id, with the declared
        // configuration and the version CloudFront issues for it.
        let read = get_control(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetOriginAccessControl({id}), got {} with \
             body {}",
            read.status, read.body
        );
        assert_eq!(
            opt_element(&read.body, "Id").as_deref(),
            Some(id.as_str()),
            "case {case:?}: expected the read to report the id the stack published, got {}",
            read.body
        );
        assert!(
            !read.etag.is_empty(),
            "case {case:?}: expected an ETag on the read of {id}, got {}",
            read.body
        );
        row.assert_reports(
            "GetOriginAccessControl",
            &answered_config(case, "GetOriginAccessControl", &read),
            declared,
        );

        // The configuration read answers the same configuration and the same version: both
        // reads report the ONE record.
        let config_read = get_control_config(&router, id).await;
        assert_eq!(
            config_read.status, 200,
            "case {case:?}: expected HTTP 200 from GetOriginAccessControlConfig({id}), got {} \
             with body {}",
            config_read.status, config_read.body
        );
        assert_eq!(
            config_read.etag, read.etag,
            "case {case:?}: expected both reads to report one version, got {:?} and {:?}",
            read.etag, config_read.etag
        );
        row.assert_reports(
            "GetOriginAccessControlConfig",
            &answered_config(case, "GetOriginAccessControlConfig", &config_read),
            declared,
        );

        // The token the read answered is the one the conditional update takes.
        let rotated = "the description after a conditional rotation";
        let updated = update_control(&router, id, &row.update_document(rotated), &read.etag).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateOriginAccessControl({id}) with the \
             version the read answered, got {} with body {}",
            updated.status, updated.body
        );
        assert!(
            !updated.etag.is_empty() && updated.etag != read.etag,
            "case {case:?}: expected the accepted update to answer a new version, got {:?} \
             after {:?}",
            updated.etag,
            read.etag
        );
        row.assert_reports(
            "UpdateOriginAccessControl",
            &answered_config(case, "UpdateOriginAccessControl", &updated),
            Some(rotated),
        );

        // The token the control has moved past is a failed precondition, and the accepted
        // rotation stands.
        let stale = update_control(
            &router,
            id,
            &row.update_document("a rotation that must not land"),
            &read.etag,
        )
        .await;
        assert_eq!(
            stale.status, 412,
            "case {case:?}: expected HTTP 412 from UpdateOriginAccessControl({id}) with the \
             superseded version, got {} with body {}",
            stale.status, stale.body
        );
        assert_eq!(
            refusal_code(&stale),
            "PreconditionFailed",
            "case {case:?}: expected PreconditionFailed, got {:?} with body {}",
            refusal_code(&stale),
            stale.body
        );
        assert_eq!(
            refusal_message(&stale),
            PRECONDITION_FAILED_MESSAGE,
            "case {case:?}: expected the refusal to state {PRECONDITION_FAILED_MESSAGE:?}, got \
             {}",
            stale.body
        );

        let after = get_control(&router, id).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: expected the refused update to leave {id} readable, got {} with \
             body {}",
            after.status, after.body
        );
        assert_eq!(
            after.etag, updated.etag,
            "case {case:?}: expected the version of the accepted rotation, got {:?} after {:?}",
            after.etag, updated.etag
        );
        row.assert_reports(
            "GetOriginAccessControl after a refused rotation",
            &answered_config(case, "GetOriginAccessControl after a refused rotation", &after),
            Some(rotated),
        );
    }
}

/// A changed declaration reaches the control through CloudFront's own update — so it keeps the
/// id its create minted, reports the newly declared configuration under a new version, and the
/// version it moved past is refused; deleting the stack takes the control away in CloudFront,
/// which is what its read then answers.
#[tokio::test]
async fn a_stack_revision_moves_the_version_and_a_deletion_takes_the_control_away() {
    let router = build_router();
    let rows = rows();
    let mut deployed_rows = Vec::new();
    for spelling in [Spelling::Yaml, Spelling::Json] {
        deployed_rows.extend(deployed(&router, "oac-revision", &rows, spelling).await);
    }

    // The versions the controls carry as the stacks created them.
    let mut created = Vec::new();
    for (row, id) in &deployed_rows {
        let read = get_control(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected HTTP 200 from GetOriginAccessControl({id}), got {} with body {}",
            row.case, read.status, read.body
        );
        created.push(read.etag);
    }

    // The revision each spelling's stack deploys: every control's description replaced, which
    // is a change CloudFront's own update applies in place.
    let revised = |row: &Row| Some(format!("{} after the stack's revision", row.name));
    for spelling in [Spelling::Yaml, Spelling::Json] {
        let declared: Vec<&Row> = rows.iter().filter(|row| row.spelling == spelling).collect();
        let stack = stack_name("oac-revision", spelling);
        let template = match spelling {
            Spelling::Json => json_template(&declared, revised),
            Spelling::Yaml => yaml_template(&declared, revised),
        };
        let (status, body) = cloudformation(
            &router,
            "UpdateStack",
            &[("StackName", &stack), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {stack:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
        );
        settled_stack(&router, &stack, &stack, UPDATE_COMPLETE).await;
    }

    for ((row, id), before) in deployed_rows.iter().zip(&created) {
        let case = &row.case;
        let read = get_control(&router, id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the revised control to keep its id, got {} with body {}",
            read.status, read.body
        );
        row.assert_reports(
            "GetOriginAccessControl after the stack's revision",
            &answered_config(case, "after the stack's revision", &read),
            revised(row).as_deref(),
        );
        assert_ne!(
            &read.etag, before,
            "case {case:?}: expected the revision to move the version past {before:?}"
        );

        // The version the revision moved past is a failed precondition, as it is for any
        // superseded one.
        let stale = update_control(
            &router,
            id,
            &row.update_document("a rotation on the version the revision replaced"),
            before,
        )
        .await;
        assert_eq!(
            stale.status, 412,
            "case {case:?}: expected HTTP 412 for the version the revision replaced, got {} \
             with body {}",
            stale.status, stale.body
        );
        assert_eq!(
            refusal_code(&stale),
            "PreconditionFailed",
            "case {case:?}: expected PreconditionFailed, got {:?} with body {}",
            refusal_code(&stale),
            stale.body
        );
    }

    // Deleting each stack takes its controls away in CloudFront.
    for spelling in [Spelling::Yaml, Spelling::Json] {
        let stack = stack_name("oac-revision", spelling);
        let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", &stack)]).await;
        assert_eq!(
            status, 200,
            "case {stack:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        deleted_stack(&router, &stack, &stack).await;
    }
    for (row, id) in &deployed_rows {
        let gone = get_control(&router, id).await;
        assert_eq!(
            gone.status, 404,
            "case {:?}: expected the deleted stack to leave no control at {id}, got {} with \
             body {}",
            row.case, gone.status, gone.body
        );
        assert_eq!(
            refusal_code(&gone),
            "NoSuchOriginAccessControl",
            "case {:?}: expected NoSuchOriginAccessControl, got {:?} with body {}",
            row.case,
            refusal_code(&gone),
            gone.body
        );
        assert_eq!(
            refusal_message(&gone),
            NO_SUCH_CONTROL_MESSAGE,
            "case {:?}: expected the refusal to state {NO_SUCH_CONTROL_MESSAGE:?}, got {}",
            row.case,
            gone.body
        );
    }
}
