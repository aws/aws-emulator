// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::CodeArtifact::PackageGroup` deploys, and the
//! group it reports created is one CodeArtifact really holds.
//!
//! The rule under test: **a resource type AWS publishes is one CloudFormation's own registry
//! recognises, so a template declaring it is accepted rather than refused as a template-format
//! error, and the resource is provisioned through the owning service's own API — the handle the
//! stack publishes addressing a resource that service really holds, a declaration that service
//! REFUSES failing the resource with its own message inside the resource provider's envelope,
//! and deleting the stack releasing what it created.** Two crates have to agree for that to
//! hold (CloudFormation makes the calls, CodeArtifact serves them), so the suite lives here: a
//! crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: the SHAPE of the pattern that
//! identifies the group (a single segment, a nested path, one carrying characters an ARN does
//! not hold literally), whether the template is written in JSON or YAML, which optional
//! properties it declares (contact information, a description, origin restrictions, tags), how
//! many groups one stack holds, and the Region it is deployed in. The refusal rows differ along
//! WHAT CodeArtifact refuses and therefore which `HandlerErrorCode` class the failure falls in.
//!
//! Every wait here is a poll for the CONDITION — the status a stack reaches on the emulator's
//! own clock — under a deadline generous enough for a loaded host; nothing asserts how long a
//! step took.
//!
//! The awsQuery form encoder and the XML element readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{blocks_of, elements, form_encode, opt_element, outputs_of, physical_ids_of};

/// How long one poll waits before looking again, and how long the looking goes on for.
///
/// A stack's status transitions fall due on CloudFormation's own clock, so every wait below
/// looks for the CONDITION; the deadline is only here so a premise that never holds fails
/// rather than hangs, and it is many times the spans involved so a loaded host cannot decide
/// the outcome.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
/// See [`POLL_INTERVAL`].
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// The account id every ARN this emulator mints carries, and therefore the `domainOwner` of
/// every package group a stack creates.
const ACCOUNT: &str = "000000000000";

/// The `HandlerErrorCode` class a refusal of the REQUEST the declaration asked for falls in,
/// and the class a refusal naming a resource that is not there falls in — both values of the
/// CloudFormation model's own `HandlerErrorCode` enum.
const INVALID_REQUEST: &str = "InvalidRequest";
/// See [`INVALID_REQUEST`].
const NOT_FOUND: &str = "NotFound";

/// The SDK's own name for CodeArtifact, which the diagnostic a resource provider reports a
/// refused call under names the service by: that model's `awsService.cloudFormationName`
/// (`smithy_query(query="service_info")` on
/// `services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/smithy-model/\
/// com-amazonaws-codeartifact-codeartifactcontrolplaneservice.json`: `sdkId: "codeartifact"`,
/// `cloudFormationName: Codeartifact`).
const SDK_SERVICE: &str = "Codeartifact";

/// The requirement CodeArtifact holds a `packageGroup` to beyond what `PackageGroupPattern`
/// models (len 2..520, "no control or whitespace characters"): a package group's identifier is
/// a PATH from the domain root, which is what makes `/*` the ancestor of `/npm/*`.
const PATTERN_REQUIREMENT: &str = "Pattern must start with '/'.";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other suite
/// here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
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
        service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which is
/// what routes the request and what each handler reads its Region from. The signature is never
/// checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
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
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one CodeArtifact operation over its restJson1 wire, signed for `region`: the method and
/// URI its own `@http` binding declares, with the resource it addresses in the query string.
async fn codeartifact(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(document) => {
            Bytes::from(serde_json::to_vec(document).expect("the request body serializes"))
        }
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("codeartifact.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("codeartifact", region))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let document: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, document)
}

/// The `PackageGroupDescription` CodeArtifact reports for one group, and the HTTP status of
/// that read: `DescribePackageGroup` binds the domain and the group's own pattern to query
/// parameters.
async fn described_group(
    router: &MultiServiceRouter,
    region: &str,
    domain: &str,
    pattern: &str,
) -> (u16, Value) {
    let uri = format!(
        "/v1/package-group?domain={}&package-group={}",
        form_encode(domain),
        form_encode(pattern)
    );
    let (status, answered) = codeartifact(router, region, "GET", &uri, None).await;
    let group = answered
        .get("packageGroup")
        .cloned()
        .unwrap_or(Value::Null);
    (status, group)
}

/// The `DomainDescription` CodeArtifact reports for one domain, and the HTTP status of that
/// read: `DescribeDomain` binds the domain's name to a query parameter.
async fn described_domain(
    router: &MultiServiceRouter,
    region: &str,
    domain: &str,
) -> (u16, Value) {
    let uri = format!("/v1/domain?domain={}", form_encode(domain));
    let (status, answered) = codeartifact(router, region, "GET", &uri, None).await;
    (status, answered.get("domain").cloned().unwrap_or(Value::Null))
}

/// The tags CodeArtifact holds on one resource, read through its own `ListTagsForResource`.
async fn tags_on(
    router: &MultiServiceRouter,
    region: &str,
    resource_arn: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let uri = format!("/v1/tags?resourceArn={}", form_encode(resource_arn));
    let (status, listed) = codeartifact(router, region, "POST", &uri, Some(json!({}))).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListTagsForResource on {resource_arn}, got \
         {status} with body {listed}"
    );
    listed
        .get("tags")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|tag| {
            Some((
                tag.get("key").and_then(Value::as_str)?.to_string(),
                tag.get("value").and_then(Value::as_str)?.to_string(),
            ))
        })
        .collect()
}

/// Look at `DescribeStacks` until the stack reports a TERMINAL status, and answer with that
/// answer. A status still in progress is looked at again until [`POLL_DEADLINE`] passes.
async fn settled_stack(router: &MultiServiceRouter, region: &str, stack: &str) -> String {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let (status, described) =
            cloudformation(router, region, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "DescribeStacks {stack:?}: expected HTTP 200, got {status} with body {described}"
        );
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        if !reported.ends_with("_IN_PROGRESS") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "expected the stack {stack:?} to settle into a terminal status, got {reported:?} \
             with answer {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look at `DescribeStacks` until the stack is GONE, which is what a completed delete leaves.
async fn deleted_stack(router: &MultiServiceRouter, region: &str, stack: &str, case: &str) {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let (_, described) =
            cloudformation(router, region, "DescribeStacks", &[("StackName", stack)]).await;
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        if reported.is_empty() || reported == "DELETE_COMPLETE" {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the stack {stack:?} to finish deleting, got {reported:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `ResourceStatusReason` the stack's events record for `logical_id` in `reported`, looked
/// for until the stack's own clock has recorded it.
async fn failure_reason(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    logical_id: &str,
    reported: &str,
    case: &str,
) -> String {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let (status, events) = cloudformation(
            router,
            region,
            "DescribeStackEvents",
            &[("StackName", stack)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackEvents, got {status} with body \
             {events}"
        );
        let found = blocks_of(&events, "member").into_iter().find_map(|member| {
            let named = elements(member, "LogicalResourceId")
                .into_iter()
                .next()
                .is_some_and(|named| named == logical_id);
            let failed = elements(member, "ResourceStatus")
                .into_iter()
                .next()
                .is_some_and(|status| status == reported);
            (named && failed)
                .then(|| elements(member, "ResourceStatusReason").into_iter().next())
                .flatten()
                .filter(|reason| !reason.is_empty())
        });
        if let Some(reason) = found {
            return reason;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {logical_id} recorded no {reported} reason in {events}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The provider exception a `ResourceStatusReason` quotes, and the `HandlerErrorCode` class it
/// reports beside it.
fn quoted_and_class(case: &str, reason: &str) -> (String, String) {
    const OPENS: &str = "Resource handler returned message: \"";
    const TOKEN: &str = "\" (RequestToken: ";
    const CLASS: &str = "HandlerErrorCode: ";
    let rest = reason.strip_prefix(OPENS).unwrap_or_else(|| {
        panic!("case {case:?}: expected the reason to open with {OPENS:?}, got {reason:?}")
    });
    let (quoted, tail) = rest.split_once(TOKEN).unwrap_or_else(|| {
        panic!("case {case:?}: expected a quoted exception and a RequestToken, got {reason:?}")
    });
    let class = tail
        .split_once(CLASS)
        .and_then(|(_, class)| class.strip_suffix(')'))
        .unwrap_or_else(|| panic!("case {case:?}: expected a HandlerErrorCode, got {reason:?}"));
    (quoted.to_string(), class.to_string())
}

/// Hold one quoted exception to the shape a CodeArtifact SDK raises — that service's own
/// sentence, the diagnostic of the refused call (the SDK's name for the service, the status it
/// answered and the id of the call) and the count of attempts its client made — and answer the
/// sentence.
fn asserted_sentence(case: &str, quoted: &str, status: u16) -> String {
    const DIAGNOSTIC: &str = " (Service: ";
    const ATTEMPTS: &str = " (SDK Attempt Count: ";
    let (head, attempts) = quoted.rsplit_once(ATTEMPTS).unwrap_or_else(|| {
        panic!("case {case:?}: expected the SDK's attempt count to close the exception, got {quoted:?}")
    });
    let counted = attempts.strip_suffix(')').unwrap_or_else(|| {
        panic!("case {case:?}: expected the attempt count to close, got {quoted:?}")
    });
    assert!(
        counted.chars().all(|digit| digit.is_ascii_digit()) && !counted.is_empty(),
        "case {case:?}: expected a count of attempts, got {counted:?}"
    );
    let (sentence, diagnostic) = head.rsplit_once(DIAGNOSTIC).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the SDK diagnostic after the service's own sentence, got \
             {quoted:?}"
        )
    });
    let envelope = diagnostic.strip_suffix(')').unwrap_or_else(|| {
        panic!("case {case:?}: expected the diagnostic to close, got {quoted:?}")
    });
    let fields: Vec<&str> = envelope.split(", ").collect();
    assert_eq!(
        fields.len(),
        3,
        "case {case:?}: expected the service, the status and the call's id and nothing else, got \
         {envelope:?}"
    );
    assert_eq!(
        fields[0], SDK_SERVICE,
        "case {case:?}: expected the SDK's own name for the service, got {:?}",
        fields[0]
    );
    assert_eq!(
        fields[1],
        format!("Status Code: {status}"),
        "case {case:?}: expected the status the owning service answered, got {:?}",
        fields[1]
    );
    assert!(
        fields[2].starts_with("Request ID: ") && fields[2].len() > "Request ID: ".len(),
        "case {case:?}: expected the id of the call, got {:?}",
        fields[2]
    );
    sentence.to_string()
}

/// One row of the round-trip table: the template a stack deploys, and what the rule says the
/// package groups it declares must then be in CodeArtifact.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which the groups' ARNs carry.
    region: &'static str,
    stack_name: &'static str,
    /// The domain the row's template declares, which every group of it lives in.
    domain: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// Each group the template declares: its logical id, the pattern that identifies it, the
    /// `Ref` output the template publishes for it, and the optional members the declaration
    /// asks for.
    groups: &'static [Group],
    /// EVERY output key the template declares. A stack that completed resolves a value for
    /// each of them: an output reported with a key and no value is a published return value
    /// this emulator could not compute.
    declared_outputs: &'static [&'static str],
    /// The output the template publishes for `Fn::GetAtt Domain.EncryptionKey` — a published
    /// return value of `AWS::CodeArtifact::Domain` that is no function of the domain's
    /// identity — and the key the row's declaration names, where it names one. A row that
    /// names none has its domain encrypted with the key CodeArtifact assigned it.
    encryption_key_output: Option<&'static str>,
    /// See [`Row::encryption_key_output`].
    declared_encryption_key: Option<&'static str>,
    /// The tags each group must carry beside the ones CloudFormation gives it.
    expected_tags: &'static [(&'static str, &'static str)],
}

/// One declared package group of a row.
struct Group {
    logical_id: &'static str,
    pattern: &'static str,
    output: &'static str,
    /// The output the template publishes for `Fn::GetAtt <group>.Arn`, which the type
    /// publishes beside `Ref` — and which must be that same ARN. `None` for a group whose row
    /// publishes no such output.
    arn_output: Option<&'static str>,
    contact_info: Option<&'static str>,
    description: Option<&'static str>,
    /// The origin restrictions the declaration asks for, as
    /// `(PackageGroupOriginRestrictionType, PackageGroupOriginRestrictionMode)` — the map
    /// `UpdatePackageGroupOriginConfiguration` takes, which is what the read reports the
    /// group's own `mode` under.
    restrictions: &'static [(&'static str, &'static str)],
}

/// The group with nothing optional declared, written out rather than built by a helper: the
/// rows are `'static` data, so each is a literal.
const NO_RESTRICTIONS: &[(&str, &str)] = &[];

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one named group of a single segment, with contact information, a \
                   description and tags, declared in JSON",
            region: "us-east-1",
            stack_name: "codeartifact-package-group-stack",
            domain: "pkg-group-domain",
            template: || {
                json!({
                    "AWSTemplateFormatVersion": "2010-09-09",
                    "Description": "a CodeArtifact domain and a package group of it",
                    "Resources": {
                        "Domain": {
                            "Type": "AWS::CodeArtifact::Domain",
                            "Properties": { "DomainName": "pkg-group-domain" },
                        },
                        "PackageGroup": {
                            "Type": "AWS::CodeArtifact::PackageGroup",
                            "DependsOn": "Domain",
                            "Properties": {
                                "DomainName": "pkg-group-domain",
                                "Pattern": "/pypi/*",
                                "ContactInfo": "platform@example.test",
                                "Description": "every PyPI package of the domain",
                                "Tags": [{ "Key": "Owner", "Value": "platform" }],
                            },
                        },
                        "Repo": {
                            "Type": "AWS::CodeArtifact::Repository",
                            "DependsOn": "Domain",
                            "Properties": {
                                "DomainName": "pkg-group-domain",
                                "RepositoryName": "pkg-group-releases",
                            },
                        },
                    },
                    "Outputs": {
                        "PackageGroupRef": { "Value": { "Ref": "PackageGroup" } },
                        "PackageGroupArn": {
                            "Value": { "Fn::GetAtt": ["PackageGroup", "Arn"] },
                        },
                        "DomainRef": { "Value": { "Ref": "Domain" } },
                        "DomainName": { "Value": { "Fn::GetAtt": ["Domain", "Name"] } },
                        "DomainOwner": { "Value": { "Fn::GetAtt": ["Domain", "Owner"] } },
                        "DomainEncryptionKey": {
                            "Value": { "Fn::GetAtt": ["Domain", "EncryptionKey"] },
                        },
                        "RepoRef": { "Value": { "Ref": "Repo" } },
                        "RepoName": { "Value": { "Fn::GetAtt": ["Repo", "Name"] } },
                    },
                })
                .to_string()
            },
            groups: &[Group {
                logical_id: "PackageGroup",
                pattern: "/pypi/*",
                output: "PackageGroupRef",
                arn_output: Some("PackageGroupArn"),
                contact_info: Some("platform@example.test"),
                description: Some("every PyPI package of the domain"),
                restrictions: &[],
            }],
            declared_outputs: &[
                "PackageGroupRef",
                "PackageGroupArn",
                "DomainRef",
                "DomainName",
                "DomainOwner",
                "DomainEncryptionKey",
                "RepoRef",
                "RepoName",
            ],
            encryption_key_output: Some("DomainEncryptionKey"),
            declared_encryption_key: None,
            expected_tags: &[("Owner", "platform")],
        },
        Row {
            case: "two groups of one domain, one nested and one carrying characters an ARN \
                   does not hold literally, with origin restrictions on the second",
            region: "eu-west-2",
            stack_name: "codeartifact-nested-package-group-stack",
            domain: "nested-group-domain",
            template: || {
                json!({
                    "Resources": {
                        "Domain": {
                            "Type": "AWS::CodeArtifact::Domain",
                            "Properties": { "DomainName": "nested-group-domain" },
                        },
                        "Npm": {
                            "Type": "AWS::CodeArtifact::PackageGroup",
                            "DependsOn": "Domain",
                            "Properties": {
                                "DomainName": { "Fn::GetAtt": ["Domain", "Name"] },
                                "Pattern": "/npm/internal/tooling/*",
                            },
                        },
                        "Scoped": {
                            "Type": "AWS::CodeArtifact::PackageGroup",
                            "DependsOn": "Domain",
                            "Properties": {
                                "DomainName": { "Fn::GetAtt": ["Domain", "Name"] },
                                "DomainOwner": ACCOUNT,
                                "Pattern": "/npm/@acme~example/*",
                                "OriginConfiguration": {
                                    "Restrictions": {
                                        "Publish": "BLOCK",
                                        "ExternalUpstream": "ALLOW",
                                    },
                                },
                            },
                        },
                    },
                    "Outputs": {
                        "NpmRef": { "Value": { "Ref": "Npm" } },
                        "ScopedRef": { "Value": { "Ref": "Scoped" } },
                        "ScopedArn": { "Value": { "Fn::GetAtt": ["Scoped", "Arn"] } },
                    },
                })
                .to_string()
            },
            groups: &[
                Group {
                    logical_id: "Npm",
                    pattern: "/npm/internal/tooling/*",
                    output: "NpmRef",
                    arn_output: None,
                    contact_info: None,
                    description: None,
                    restrictions: NO_RESTRICTIONS,
                },
                Group {
                    logical_id: "Scoped",
                    pattern: "/npm/@acme~example/*",
                    output: "ScopedRef",
                    arn_output: Some("ScopedArn"),
                    contact_info: None,
                    description: None,
                    restrictions: &[("PUBLISH", "BLOCK"), ("EXTERNAL_UPSTREAM", "ALLOW")],
                },
            ],
            declared_outputs: &["NpmRef", "ScopedRef", "ScopedArn"],
            encryption_key_output: None,
            declared_encryption_key: None,
            expected_tags: &[],
        },
        Row {
            case: "one group declared in YAML, with a description",
            region: "ap-southeast-2",
            stack_name: "codeartifact-yaml-package-group-stack",
            domain: "yaml-group-domain",
            template: || {
                [
                    "AWSTemplateFormatVersion: '2010-09-09'",
                    "Resources:",
                    "  Domain:",
                    "    Type: AWS::CodeArtifact::Domain",
                    "    Properties:",
                    "      DomainName: yaml-group-domain",
                    "      EncryptionKey: arn:aws:kms:ap-southeast-2:000000000000:key/\
                     abcdef01-2345-6789-abcd-ef0123456789",
                    "  Maven:",
                    "    Type: AWS::CodeArtifact::PackageGroup",
                    "    DependsOn: Domain",
                    "    Properties:",
                    "      DomainName: yaml-group-domain",
                    "      Pattern: /maven/com.example/*",
                    "      Description: the organisation's own Maven packages",
                    "Outputs:",
                    "  MavenRef:",
                    "    Value: !Ref Maven",
                    "  DomainKey:",
                    "    Value: !GetAtt Domain.EncryptionKey",
                ]
                .join("\n")
            },
            groups: &[Group {
                logical_id: "Maven",
                pattern: "/maven/com.example/*",
                output: "MavenRef",
                arn_output: None,
                contact_info: None,
                description: Some("the organisation's own Maven packages"),
                restrictions: &[],
            }],
            declared_outputs: &["MavenRef", "DomainKey"],
            encryption_key_output: Some("DomainKey"),
            declared_encryption_key: Some(
                "arn:aws:kms:ap-southeast-2:000000000000:key/\
                 abcdef01-2345-6789-abcd-ef0123456789",
            ),
            expected_tags: &[],
        },
    ]
}

/// Every row: the stack is ACCEPTED (the type is one the registry recognises), reaches
/// `CREATE_COMPLETE`, and the handle it publishes for each declared package group is the ARN
/// CodeArtifact minted for a group that service really holds — configured as the template
/// declared it, carrying the stack's own tags, and released again when the stack is deleted.
#[tokio::test]
async fn a_stack_provisions_the_codeartifact_package_groups_its_template_declares() {
    let router = build_router();

    for row in rows() {
        let case = row.case;
        let region = row.region;
        let stack = row.stack_name;
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", stack),
                ("TemplateBody", &template),
                ("Tags.member.1.Key", "Stack"),
                ("Tags.member.1.Value", "package-group-suite"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack — the type is one AWS publishes \
             — got {status} with body {body}"
        );

        let described = settled_stack(&router, region, stack).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", stack)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical_ids = physical_ids_of(&resources);
        let outputs = outputs_of(&described);
        // Every output the template declares resolved to a value: a stack that reports a key
        // with no value published a return value this emulator could not compute.
        for key in row.declared_outputs {
            let value = outputs.get(*key);
            assert!(
                value.is_some_and(|value| !value.is_empty()),
                "case {case:?}: expected the output {key:?} to carry a value, got {outputs:?}"
            );
        }
        assert_eq!(
            outputs.len(),
            row.declared_outputs.len(),
            "case {case:?}: expected one value per declared output, got {outputs:?}"
        );

        // The domain's own published return value that is no function of its identity: the key
        // CodeArtifact encrypts it with, which is what that service reports for the domain —
        // the key the declaration named, or the one it assigned a domain declared without one.
        if let Some(published) = row.encryption_key_output {
            let (status, domain) = described_domain(&router, region, row.domain).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CodeArtifact to hold the stack's domain, got {status} \
                 with body {domain}"
            );
            let reported = domain.get("encryptionKey").and_then(Value::as_str);
            assert!(
                reported.is_some_and(|key| !key.is_empty()),
                "case {case:?}: expected the domain to report the key it is encrypted with, got \
                 {domain}"
            );
            assert_eq!(
                outputs.get(published).map(String::as_str),
                reported,
                "case {case:?}: expected Fn::GetAtt EncryptionKey to be the key CodeArtifact \
                 reports, got {:?}",
                outputs.get(published)
            );
            if let Some(declared) = row.declared_encryption_key {
                assert_eq!(
                    reported,
                    Some(declared),
                    "case {case:?}: expected the declared key, got {domain}"
                );
            }
        }

        for group in row.groups {
            let pattern = group.pattern;
            let (status, reported) = described_group(&router, region, row.domain, pattern).await;
            assert_eq!(
                status, 200,
                "case {case:?} group {pattern:?}: expected CodeArtifact to hold the group its \
                 own stack reported complete, got {status} with body {reported}"
            );

            let arn = reported
                .get("arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?} group {pattern:?}: expected the group's ARN, got {reported}"
                    )
                })
                .to_string();
            // "Ref returns the Amazon Resource Name (ARN) of the package group": the handle the
            // stack publishes for the resource is the one CodeArtifact keyed the group by.
            assert_eq!(
                physical_ids.get(group.logical_id),
                Some(&arn),
                "case {case:?} group {pattern:?}: expected the physical id to be the ARN \
                 {arn:?}, got {:?}",
                physical_ids.get(group.logical_id)
            );
            assert_eq!(
                outputs.get(group.output),
                Some(&arn),
                "case {case:?} group {pattern:?}: expected Ref to be the ARN {arn:?}, got {:?}",
                outputs.get(group.output)
            );
            if let Some(published) = group.arn_output {
                assert_eq!(
                    outputs.get(published),
                    Some(&arn),
                    "case {case:?} group {pattern:?}: expected Fn::GetAtt Arn to be the ARN \
                     {arn:?}, got {:?}",
                    outputs.get(published)
                );
            }
            let prefix =
                format!("arn:aws:codeartifact:{region}:{ACCOUNT}:package-group/{}/", row.domain);
            assert!(
                arn.starts_with(&prefix),
                "case {case:?} group {pattern:?}: expected a package-group ARN under \
                 {prefix:?}, got {arn:?}"
            );

            assert_eq!(
                reported.get("pattern").and_then(Value::as_str),
                Some(pattern),
                "case {case:?} group {pattern:?}: expected the group to carry the pattern the \
                 template declared, got {reported}"
            );
            assert_eq!(
                reported.get("domainName").and_then(Value::as_str),
                Some(row.domain),
                "case {case:?} group {pattern:?}: expected the group to live in the stack's own \
                 domain, got {reported}"
            );
            assert_eq!(
                reported.get("domainOwner").and_then(Value::as_str),
                Some(ACCOUNT),
                "case {case:?} group {pattern:?}: expected the account that owns the domain, got \
                 {reported}"
            );
            assert_eq!(
                reported.get("contactInfo").and_then(Value::as_str),
                group.contact_info,
                "case {case:?} group {pattern:?}: expected the declared contact information, got \
                 {reported}"
            );
            assert_eq!(
                reported.get("description").and_then(Value::as_str),
                group.description,
                "case {case:?} group {pattern:?}: expected the declared description, got \
                 {reported}"
            );
            for (restriction, mode) in group.restrictions {
                let in_force = reported
                    .pointer(&format!(
                        "/originConfiguration/restrictions/{restriction}/mode"
                    ))
                    .and_then(Value::as_str);
                assert_eq!(
                    in_force,
                    Some(*mode),
                    "case {case:?} group {pattern:?}: expected the declared {restriction} mode \
                     {mode:?}, got {reported}"
                );
            }

            // CloudFormation tags what it creates: the stack-level tags it propagates and the
            // three naming the stack and the resource's place in it, beside the declared ones.
            let held = tags_on(&router, region, &arn, case).await;
            for (key, value) in row.expected_tags {
                assert_eq!(
                    held.get(*key).map(String::as_str),
                    Some(*value),
                    "case {case:?} group {pattern:?}: expected the declared tag {key:?}, got \
                     {held:?}"
                );
            }
            assert_eq!(
                held.get("Stack").map(String::as_str),
                Some("package-group-suite"),
                "case {case:?} group {pattern:?}: expected the stack's own tag on the group, \
                 got {held:?}"
            );
            assert_eq!(
                held.get("aws:cloudformation:logical-id").map(String::as_str),
                Some(group.logical_id),
                "case {case:?} group {pattern:?}: expected the resource's logical id on the \
                 group, got {held:?}"
            );
        }

        // Deleting the stack releases every group it created.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        deleted_stack(&router, region, stack, case).await;
        for group in row.groups {
            let (status, gone) = described_group(&router, region, row.domain, group.pattern).await;
            assert_eq!(
                status, 404,
                "case {case:?} group {:?}: expected the group to be released, got {status} with \
                 body {gone}",
                group.pattern
            );
        }
    }
}

/// One refusal row: a declaration CodeArtifact will not carry out, and what the resource
/// provider must report for it.
struct Refusal {
    case: &'static str,
    /// The pattern the template declares for the group, and the domain it declares it in.
    pattern: &'static str,
    /// Whether the template declares the domain too — a row that does not names a domain
    /// nothing holds, which is a DIFFERENT refusal class.
    declares_domain: bool,
    /// The HTTP status CodeArtifact answers the refused call with.
    status: u16,
    /// The `HandlerErrorCode` class the failure falls in.
    class: &'static str,
    /// The status a READ of that pattern answers once nothing holds the group: a pattern no
    /// package group can ever have is a bad REQUEST at every operation that takes one (400),
    /// while a well-formed pattern nothing holds is a resource that is not there (404).
    absent_status: u16,
}

const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a pattern that is no path from the domain root",
        pattern: "not-a-valid-pattern-format",
        declares_domain: true,
        status: 400,
        class: INVALID_REQUEST,
        absent_status: 400,
    },
    Refusal {
        case: "a pattern whose first segment is spelled without the leading separator",
        pattern: "npm/*",
        declares_domain: true,
        status: 400,
        class: INVALID_REQUEST,
        absent_status: 400,
    },
    Refusal {
        case: "a group of a domain nothing holds",
        pattern: "/npm/*",
        declares_domain: false,
        status: 404,
        class: NOT_FOUND,
        absent_status: 404,
    },
];

/// Every row: the stack is ACCEPTED — the type is recognised, so nothing is refused at
/// template-format time — and the refusal happens in the resource HANDLER, which reports
/// CodeArtifact's own message inside the provider's envelope and under the
/// `HandlerErrorCode` class the refusal falls in; the stack rolls back, leaving nothing the
/// failed deployment created.
#[tokio::test]
async fn a_declaration_the_owning_service_refuses_rolls_the_stack_back() {
    let router = build_router();
    let region = "us-west-2";

    for (index, row) in REFUSALS.iter().enumerate() {
        let case = row.case;
        let stack = format!("codeartifact-package-group-refusal-{index}");
        let domain = format!("refused-group-domain-{index}");
        let mut resources = serde_json::Map::new();
        let mut group = json!({
            "Type": "AWS::CodeArtifact::PackageGroup",
            "Properties": { "DomainName": domain, "Pattern": row.pattern },
        });
        if row.declares_domain {
            resources.insert(
                "Domain".to_string(),
                json!({
                    "Type": "AWS::CodeArtifact::Domain",
                    "Properties": { "DomainName": domain },
                }),
            );
            group
                .as_object_mut()
                .expect("the declaration is an object")
                .insert("DependsOn".to_string(), json!("Domain"));
        }
        resources.insert("PackageGroup".to_string(), group);
        let template = json!({
            "Resources": Value::Object(resources),
            "Outputs": { "PackageGroupRef": { "Value": { "Ref": "PackageGroup" } } },
        })
        .to_string();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateStack to be ACCEPTED — the refusal belongs to the \
             resource handler, not to template validation — got {status} with body {body}"
        );

        let described = settled_stack(&router, region, &stack).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("ROLLBACK_COMPLETE"),
            "case {case:?}: expected the failed create to be rolled back, got {described}"
        );

        let reason =
            failure_reason(&router, region, &stack, "PackageGroup", "CREATE_FAILED", case).await;
        let (quoted, class) = quoted_and_class(case, &reason);
        assert_eq!(
            class, row.class,
            "case {case:?}: expected the failure under HandlerErrorCode {}, got {class:?}",
            row.class
        );
        let sentence = asserted_sentence(case, &quoted, row.status);
        if row.class == INVALID_REQUEST {
            // CodeArtifact states a member-level constraint failure in its own envelope,
            // naming the member and the requirement the submitted value failed.
            assert_eq!(
                sentence,
                format!(
                    "1 validation error detected: Value '{}' at 'packageGroup' failed to \
                     satisfy constraint: {PATTERN_REQUIREMENT}",
                    row.pattern
                ),
                "case {case:?}: expected CodeArtifact's own validation sentence, got \
                 {sentence:?}"
            );
        } else {
            assert!(
                sentence.contains(&domain),
                "case {case:?}: expected the sentence to name the domain nothing holds, got \
                 {sentence:?}"
            );
        }

        // Nothing the failed deployment created is left behind: the group is not there, and
        // neither is the domain the rollback removed.
        let (status, gone) = described_group(&router, region, &domain, row.pattern).await;
        assert_eq!(
            status, row.absent_status,
            "case {case:?}: expected no package group to be left behind, got {status} with body \
             {gone}"
        );
        let (status, domain_read) = codeartifact(
            &router,
            region,
            "GET",
            &format!("/v1/domain?domain={}", form_encode(&domain)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the rolled-back domain to be gone, got {status} with body \
             {domain_read}"
        );
    }
}

/// A CHANGED declaration of what a package group that exists can be given a new value for —
/// its contact information, its description, its origin restrictions and its TAGS — reaches
/// the group CodeArtifact already holds rather than replacing it: the ARN the stack publishes
/// is the one it published before, and the group reports the new values.
#[tokio::test]
async fn a_changed_declaration_reaches_the_group_the_stack_already_has() {
    let router = build_router();
    let region = "eu-central-1";
    let case = "an updated description, contact information, publish restriction and tag set";
    let stack = "codeartifact-package-group-update-stack";
    let domain = "updated-group-domain";
    let pattern = "/nuget/*";

    let template = |description: &str, contact: &str, publish: &str, tags: Value| {
        json!({
            "Resources": {
                "Domain": {
                    "Type": "AWS::CodeArtifact::Domain",
                    "Properties": { "DomainName": domain },
                },
                "PackageGroup": {
                    "Type": "AWS::CodeArtifact::PackageGroup",
                    "DependsOn": "Domain",
                    "Properties": {
                        "DomainName": domain,
                        "Pattern": pattern,
                        "ContactInfo": contact,
                        "Description": description,
                        "OriginConfiguration": {
                            "Restrictions": { "Publish": publish },
                        },
                        "Tags": tags,
                    },
                },
            },
            "Outputs": { "PackageGroupRef": { "Value": { "Ref": "PackageGroup" } } },
        })
        .to_string()
    };

    let first = template(
        "the NuGet packages",
        "first@example.test",
        "ALLOW",
        json!([
            { "Key": "Owner", "Value": "platform" },
            { "Key": "Retired", "Value": "no" },
        ]),
    );
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", &first)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    let described = settled_stack(&router, region, stack).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
    );
    let created = outputs_of(&described)
        .get("PackageGroupRef")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no PackageGroupRef output in {described}"));

    let second = template(
        "every NuGet package of the domain",
        "second@example.test",
        "BLOCK",
        json!([
            { "Key": "Owner", "Value": "release-engineering" },
            { "Key": "Tier", "Value": "internal" },
        ]),
    );
    let (status, body) = cloudformation(
        &router,
        region,
        "UpdateStack",
        &[("StackName", stack), ("TemplateBody", &second)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    let described = settled_stack(&router, region, stack).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the update to complete, got {described}"
    );
    assert_eq!(
        outputs_of(&described).get("PackageGroupRef"),
        Some(&created),
        "case {case:?}: expected the group to keep the ARN it was created under, got {described}"
    );

    let (status, group) = described_group(&router, region, domain, pattern).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CodeArtifact to still hold the group, got {status} with body \
         {group}"
    );
    assert_eq!(
        group.get("description").and_then(Value::as_str),
        Some("every NuGet package of the domain"),
        "case {case:?}: expected the updated description, got {group}"
    );
    assert_eq!(
        group.get("contactInfo").and_then(Value::as_str),
        Some("second@example.test"),
        "case {case:?}: expected the updated contact information, got {group}"
    );
    assert_eq!(
        group
            .pointer("/originConfiguration/restrictions/PUBLISH/mode")
            .and_then(Value::as_str),
        Some("BLOCK"),
        "case {case:?}: expected the updated publish restriction, got {group}"
    );

    // The tags the new declaration asks for are the ones the group carries: a key it re-values
    // is re-valued, a key it no longer states is removed, and a key it adds is added — through
    // CodeArtifact's own tagging calls, since the update call carries no tag member.
    let arn = group
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected the group's ARN, got {group}"));
    let held = tags_on(&router, region, arn, case).await;
    assert_eq!(
        held.get("Owner").map(String::as_str),
        Some("release-engineering"),
        "case {case:?}: expected the re-valued tag, got {held:?}"
    );
    assert_eq!(
        held.get("Tier").map(String::as_str),
        Some("internal"),
        "case {case:?}: expected the added tag, got {held:?}"
    );
    assert!(
        !held.contains_key("Retired"),
        "case {case:?}: expected the tag the declaration no longer states to be removed, got \
         {held:?}"
    );
    // The three CloudFormation gives every resource it creates survive the reconciliation.
    assert_eq!(
        held.get("aws:cloudformation:logical-id").map(String::as_str),
        Some("PackageGroup"),
        "case {case:?}: expected the resource's logical id to still be on the group, got \
         {held:?}"
    );
}
