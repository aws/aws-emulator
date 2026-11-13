// SPDX-License-Identifier: CC0-1.0

//! The stack lifecycle, end to end over the awsQuery wire.
//!
//! Every case here drives the real request path — form-encoded body in, XML envelope out
//! — through the router, so what it asserts is what a client
//! sees: `CreateStack` persists a stack, and `DescribeStacks`, `ListStacks`,
//! `GetTemplate`, `UpdateStack`, `DeleteStack` and `DescribeStackEvents` read and mutate
//! THAT stack rather than echoing the request back. That router carries the services that
//! own the resource types a stack really creates beside this one on one engine — Amazon S3
//! for an `AWS::S3::Bucket`, Amazon SNS for the `AWS::SNS::Topic` most of these templates
//! declare — because a stack provisions them by calling those services in process (see
//! `router_with_engine`).
//!
//! The wire harness itself lives in `harness.rs`, shared by every suite of this target
//! (`stack_outputs.rs` and `error_envelope.rs` drive their cases through the same
//! helpers).

use std::time::{Duration, Instant, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::{
    self, status, StoredStack, MAX_DELETED_STACKS_RETAINED, MAX_EVENTS_PER_STACK, MAX_STACKS,
    PAGE_SIZE, STATUS_TRANSITION_DELAY, TEMPLATE_BODY_MAX_BYTES,
};
use smithy_server_runtime::MultiServiceRouter;

pub(crate) use crate::harness::{
    call, call_in, create_stack, ec2_tag_value, enroll_as_a_marketplace_seller, get_template,
    is_service_assigned_id, list_imports_ok, member, router, router_with_engine, store,
    wait_for_the_transition, xml_blocks, xml_text, xml_texts, xml_unescape, REGION,
    SERVICE_ID_HEX_DIGITS,
};

/// The emulator's canonical account id, as every ARN it emits carries.
pub(crate) const ACCOUNT_ID: &str = "000000000000";

/// The status a stack must report on a read taken `started` ago: the in-progress one
/// while the transition is not yet due, the settled one once it is.
///
/// The guard is what keeps the case honest on a loaded machine: the emulator owes the
/// in-progress status only until the due instant, so a read that lost the race to the
/// clock must be held to the settled status instead of failing.
pub(crate) fn expected_status(
    started: Instant,
    in_progress: &'static str,
    settled: &'static str,
) -> &'static str {
    if started.elapsed() < STATUS_TRANSITION_DELAY {
        in_progress
    } else {
        settled
    }
}

/// `DescribeStacks` for one stack, by name or by stack id.
pub(crate) async fn describe_stack(
    router: &MultiServiceRouter,
    name_or_id: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeStacks",
        &[member("StackName", name_or_id)],
    )
    .await
}

/// Percent-encode one form value, so a template body carrying `{`, `"`, `&` or a
/// multi-byte character survives the body.
///
/// Everything outside the unreserved set an AWS SDK leaves alone is escaped, which is
/// what the form body an SDK sends looks like.
pub(crate) fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One awsQuery call to IAM over the same router, answered with the HTTP status and the XML
/// body.
///
/// The ONE IAM caller of this crate's suites: the role records a stack reads (a `RoleARN`'s
/// trust policy) and the roles a stack CREATES are both written and read through IAM's own
/// wire, so a change to the calling convention is made here once.
pub(crate) async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed IAM request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// One awsQuery call to Amazon SNS over the same router, answered with the HTTP status and
/// the XML body.
///
/// The ONE SNS caller of this crate's suites: the topics a stack CREATES live in Amazon SNS,
/// so a case that reads one back — its attributes, its tags — goes through SNS's own wire
/// here, signed for `sns` in the same Region the stack was created in, which is the Region
/// its topic ARNs carry.
pub(crate) async fn sns(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    sns_in(router, REGION, action, params).await
}

/// [`sns`], signed for `region`: a stack deploys its resources in the Region the call that
/// created it was signed for, and a StackSet's stack instance deploys them in the TARGET
/// Region it was created for — so a case that reads one of those topics back reaches Amazon
/// SNS in that Region, which is the Region its topic ARNs carry.
pub(crate) async fn sns_in(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-03-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sns.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sns/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed SNS request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// One awsQuery call to Amazon RDS over the same router, answered with the HTTP status and
/// the XML body.
///
/// The ONE RDS caller of this crate's suites: a DB instance or DB subnet group a stack
/// CREATES lives in Amazon RDS, so a case that reads one back goes through RDS's own wire
/// here, signed for `rds` in the Region the stack was created in — which is the Region its
/// identifiers are scoped to. The API version is RDS's own (`version: 2014-10-31`).
pub(crate) async fn rds(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2014-10-31");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("rds.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/rds/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed RDS request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// One ec2Query call to Amazon EC2 over the same router, answered with the HTTP status and
/// the XML body.
///
/// The ONE EC2 caller of this crate's suites: a VPC or a subnet a stack CREATES lives in
/// Amazon EC2, so a case that reads one back goes through EC2's own wire here, signed for
/// `ec2` in the Region the stack was created in. The envelope is the same `Action` plus
/// `Version` form body an awsQuery call carries, under EC2's own API version
/// (`version: 2016-11-15`), and a list member is flattened into indexed keys
/// (`VpcId.1`, `SubnetId.1`).
pub(crate) async fn ec2(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2016-11-15");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ec2.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ec2/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed EC2 request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// One awsQuery call to Elastic Load Balancing v2 over the same router, answered with the
/// HTTP status and the XML body.
///
/// The ONE Elastic Load Balancing v2 caller of this crate's suites: the load balancer, target
/// group, listener and rule an `AWS::ElasticLoadBalancingV2::*` resource declares are CREATED
/// in that service, so a case that reads one back — or drives it through the stack's later
/// operations — goes through that service's own wire here, signed for `elasticloadbalancing`
/// in the Region the stack was created in, under its own API version (`version: 2015-12-01`).
///
/// The members are the pairs [`call`] takes, since an Elastic Load Balancing list member is
/// an indexed key whose value a case builds from an identifier it read
/// (`ListenerArns.member.1`).
pub(crate) async fn elbv2(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(String, String)],
) -> (u16, String) {
    let mut form = form_urlencoded::Serializer::new(String::new());
    form.append_pair("Action", action);
    form.append_pair("Version", "2015-12-01");
    for (key, value) in members {
        form.append_pair(key, value);
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://elasticloadbalancing.{REGION}.amazonaws.com/"
        ))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/elasticloadbalancing/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(form.finish()))
        .expect("a well-formed Elastic Load Balancing request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// One awsQuery call to Amazon EC2 Auto Scaling over the same router, answered with the HTTP
/// status and the XML body.
///
/// The ONE Auto Scaling caller of this crate's suites: the group an
/// `AWS::AutoScaling::AutoScalingGroup` declares is CREATED in that service, so a case that
/// reads one back — its zones, its ARN, its tags — goes through that service's own wire here,
/// signed for `autoscaling` in the Region the stack was created in, under its own API version
/// (`version: 2011-01-01`).
pub(crate) async fn autoscaling(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2011-01-01");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("autoscaling.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/autoscaling/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed Auto Scaling request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    (status, body)
}

/// The `ResourceStatusReason` a stack's event stream carries for the event in which
/// `logical_id` reached `resource_status`, or the empty string when it recorded none.
///
/// `StackEvent$ResourceStatusReason` is the "success/failure message associated with the
/// resource", so this is the PER-RESOURCE field a failing resource's own diagnosis — the
/// refusal the service that owns it answered — is reported on, beside the stack-level
/// summary of which logical resources failed. Shared so every suite that reads one asks
/// for it the same way.
pub(crate) async fn resource_event_reason(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
    resource_status: &str,
) -> String {
    let (code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "DescribeStackEvents({stack}) must answer 200, got {code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .find(|entry| {
            xml_text(entry, "LogicalResourceId").as_deref() == Some(logical_id)
                && xml_text(entry, "ResourceStatus").as_deref() == Some(resource_status)
        })
        .and_then(|entry| xml_text(entry, "ResourceStatusReason"))
        .unwrap_or_default()
}

/// Every `ResourceStatus` the stack's event stream carries for `logical_id`, newest first —
/// `DescribeStackEvents` "returns all stack related events for a specified stack in reverse
/// chronological order".
///
/// The statuses counterpart of [`resource_event_reason`], and the ONE reader of them: what a
/// stack's event stream says BECAME of one resource — whether its deletion was skipped, carried
/// out, or reported at all — is read the same way by every suite that asks.
pub(crate) async fn resource_event_statuses(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
) -> Vec<String> {
    let (code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackEvents({stack}) must answer 200, got {code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .filter(|entry| xml_text(entry, "LogicalResourceId").as_deref() == Some(logical_id))
        .filter_map(|entry| xml_text(entry, "ResourceStatus"))
        .collect()
}

/// Wait past the instant a drift detection's own result becomes due.
///
/// The drift clock's counterpart to [`wait_for_the_transition`], shared by every suite that
/// runs a detection so the two cannot disagree about how long one takes to conclude.
pub(crate) async fn wait_for_the_detection() {
    tokio::time::sleep(state::DRIFT_DETECTION_DELAY + Duration::from_millis(250)).await;
}

/// Deploy a template, wait past the instant its create settles, and assert it completed.
///
/// The IAM resources a template may declare are acknowledged, so a row that declares one is
/// accepted whichever of the two capabilities it requires.
pub(crate) async fn deploy(router: &MultiServiceRouter, name: &str, template: &str) {
    let (status, body) = create_stack(router, name, template, &acknowledging_iam(&[])).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateStack answered {status} with {body}"
    );
    wait_for_the_transition().await;
    let (status, body) = describe_stack(router, name).await;
    assert_eq!(
        status, 200,
        "case {name:?}: DescribeStacks answered {status} with {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {name:?}: expected the create to complete, got {body}"
    );
}

/// The fixed form of `CapabilitiesReason`: "the list of resources that generated the values
/// in the Capabilities response element".
///
/// Shared by every suite that reads a template-reporting response (`validate_template.rs`
/// and `template_summary.rs` both report the capabilities a template requires), so the two
/// cannot drift about the form of that one sentence.
pub(crate) const CAPABILITIES_REASON_PREFIX: &str =
    "The following resource(s) require capabilities: [";

/// The resource types a `CapabilitiesReason` names, sorted — the set matters, the order the
/// service happened to walk the `Resources` section in does not.
pub(crate) fn reason_resource_types(reason: &str) -> Vec<String> {
    let inner = reason
        .strip_prefix(CAPABILITIES_REASON_PREFIX)
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or_default();
    let mut types: Vec<String> = inner
        .split(", ")
        .filter(|entry| !entry.is_empty())
        .map(ToString::to_string)
        .collect();
    types.sort();
    types
}

/// The response with its `Parameters` block removed, so reading a TOP-LEVEL element does not
/// pick up a parameter's own `Description` (the envelope carries `Parameters` first).
///
/// Shared for the same reason [`CAPABILITIES_REASON_PREFIX`] is: every response that reports
/// what a template declares carries both a top-level `Description` and one per parameter.
pub(crate) fn outside_parameters(xml: &str) -> String {
    let close = "</Parameters>";
    match (xml.find("<Parameters>"), xml.find(close)) {
        (Some(start), Some(end)) => {
            format!("{}{}", &xml[..start], &xml[end + close.len()..])
        }
        _ => xml.to_string(),
    }
}

/// The top-level `<member>` elements of the first `tag` list in `xml`, each as the XML it
/// holds, in wire order.
///
/// The leaf form of this scan is [`xml_blocks`]; this one is for a LIST, whose entries the
/// query serializer renders as `<member>` and whose first entry carries the envelope's
/// `xmlns`, so the element is matched by name. A member may itself carry a nested list of
/// `<member>`s (a `StackResourceDrift`'s `PropertyDifferences`), so the scan tracks DEPTH:
/// an inner member does not close the outer one, and what comes back is the outer entries.
pub(crate) fn list_members(xml: &str, tag: &str) -> Vec<String> {
    let mut members = Vec::new();
    let Some(block) = xml_blocks(xml, tag).into_iter().next() else {
        return members;
    };
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut index = 0usize;
    while index < block.len() {
        let rest = &block[index..];
        if rest.starts_with("<member") {
            let Some(tag_end) = rest.find('>') else { break };
            index += tag_end + 1;
            if depth == 0 {
                start = index;
            }
            depth += 1;
            continue;
        }
        if rest.starts_with("</member>") {
            if depth == 0 {
                // A close with nothing open: the scan has run past the list it was handed,
                // so it stops with what it read rather than underflowing the depth.
                break;
            }
            depth -= 1;
            if depth == 0 {
                members.push(block[start..index].to_string());
            }
            index += "</member>".len();
            continue;
        }
        index += rest.chars().next().map_or(1, char::len_utf8);
    }
    members
}

/// The `OutputValue` a described stack reports under `key`, read INSIDE the `Outputs` entry
/// that carries that `OutputKey` — so an `OutputKey` and an `OutputValue` belonging to two
/// different outputs cannot be paired.
///
/// Shared so a suite reading one output does not zip two parallel lists itself
/// ([`list_members`] is the scan it is built on). `None` where the stack reports no output
/// under that key — which is also what an output carrying a key and no value looks like, the
/// shape a client's `{o["OutputKey"]: o["OutputValue"]}` read raises on.
pub(crate) fn output_value(xml: &str, key: &str) -> Option<String> {
    list_members(xml, "Outputs")
        .into_iter()
        .find(|entry| xml_text(entry, "OutputKey").as_deref() == Some(key))
        .and_then(|entry| xml_text(&entry, "OutputValue"))
}

/// `extra` plus the `Capabilities` member that acknowledges the IAM resources a template
/// declares.
///
/// `CAPABILITY_NAMED_IAM` acknowledges both forms the gate distinguishes — "If you have IAM
/// resources, you can specify either capability. If you have IAM resources with custom
/// names, you must specify CAPABILITY_NAMED_IAM" — so a row whose template declares an IAM
/// resource is accepted whichever of the two it requires.
pub(crate) fn acknowledging_iam(extra: &[(String, String)]) -> Vec<(String, String)> {
    let mut members = extra.to_vec();
    members.push(member("Capabilities.member.1", "CAPABILITY_NAMED_IAM"));
    members
}

/// A created stack's id is the ARN form CloudFormation mints:
/// `arn:aws:cloudformation:<region>:<account>:stack/<name>/<uuid>`.
pub(crate) fn assert_stack_id_shape(case: &str, stack_id: &str, stack_name: &str) {
    let prefix = format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/{stack_name}/");
    assert!(
        stack_id.starts_with(&prefix),
        "case {case:?}: expected a stack id starting {prefix:?}, got {stack_id:?}"
    );
    let uuid = &stack_id[prefix.len()..];
    assert_eq!(
        uuid.len(),
        36,
        "case {case:?}: expected a 36-character uuid after the stack name, got {uuid:?}"
    );
    assert!(
        uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "case {case:?}: expected a hex-and-hyphen uuid, got {uuid:?}"
    );
}

/// One row of the create/describe/get-template table.
struct TemplateCase {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    /// The `Description` the stack must report: the template's own, when it has one.
    description: Option<&'static str>,
    parameters: Vec<(String, String)>,
    tags: Vec<(String, String)>,
}

/// A borrowed key/value table as owned pairs.
pub(crate) fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The keys of a key/value table, for comparing against a response's elements.
fn keys(pairs: &[(String, String)]) -> Vec<String> {
    pairs.iter().map(|(key, _)| key.clone()).collect()
}

/// The values of a key/value table.
fn values(pairs: &[(String, String)]) -> Vec<String> {
    pairs.iter().map(|(_, value)| value.clone()).collect()
}

/// The class the create/read rule covers: any template body, of any form and size, with
/// any number of parameters and tags. The issue's own case (a one-resource JSON template
/// with one tag) is the second row.
fn template_cases() -> Vec<TemplateCase> {
    vec![
        TemplateCase {
            case: "minimal json object, no parameters, no tags",
            stack_name: "minimal",
            template: "{}".to_string(),
            description: None,
            parameters: vec![],
            tags: vec![],
        },
        TemplateCase {
            case: "described single-resource template with one tag",
            stack_name: "demo",
            template: r#"{"Description":"demo stack","Resources":{"T":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#
                .to_string(),
            description: Some("demo stack"),
            parameters: vec![],
            tags: kv(&[("env", "dev")]),
        },
        TemplateCase {
            case: "three parameters whose values carry spaces, symbols and an empty string",
            stack_name: "params-3",
            template: r#"{"Description":"parameterised","Parameters":{"Alpha":{"Type":"String"},"Beta":{"Type":"String"},"Gamma":{"Type":"String"}}}"#
                .to_string(),
            description: Some("parameterised"),
            parameters: kv(&[("Alpha", "a b&c=d"), ("Beta", ""), ("Gamma", "3")]),
            tags: kv(&[("team", "iam"), ("env", "prod")]),
        },
        TemplateCase {
            case: "yaml body with no Description section",
            stack_name: "yaml-body",
            template: "Resources:\n  T:\n    Type: AWS::CloudFormation::WaitConditionHandle\n"
                .to_string(),
            description: None,
            parameters: vec![],
            tags: vec![],
        },
        TemplateCase {
            // CloudFormation reads a template written in JSON *or* YAML, so the
            // `Description` it reports comes from either spelling.
            case: "yaml body declaring a Description",
            stack_name: "yaml-described",
            template: "Description: a yaml stack\nResources:\n  T:\n    Type: AWS::SNS::Topic\n"
                .to_string(),
            description: Some("a yaml stack"),
            parameters: vec![],
            tags: kv(&[("env", "dev")]),
        },
        TemplateCase {
            case: "unicode, quotes and angle brackets in the template",
            stack_name: "escaped",
            template: r#"{"Description":"café \"quoted\" & <angled>"}"#.to_string(),
            description: Some("café \"quoted\" & <angled>"),
            parameters: vec![],
            tags: kv(&[("note", "a<b&c>d")]),
        },
        TemplateCase {
            case: "large template body and a long parameter value",
            stack_name: "large",
            template: format!(
                r#"{{"Description":"large","Parameters":{{"Pad":{{"Type":"String"}}}},"Metadata":{{"Pad":"{}"}}}}"#,
                "x".repeat(8192)
            ),
            description: Some("large"),
            parameters: vec![("Pad".to_string(), "y".repeat(512))],
            tags: vec![],
        },
    ]
}

#[tokio::test]
async fn create_then_describe_and_get_template_round_trip_every_template_shape() {
    let router = router();

    for case in template_cases() {
        let mut extra = Vec::new();
        for (index, (key, value)) in case.parameters.iter().enumerate() {
            let n = index + 1;
            extra.push(member(&format!("Parameters.member.{n}.ParameterKey"), key));
            extra.push(member(&format!("Parameters.member.{n}.ParameterValue"), value));
        }
        for (index, (key, value)) in case.tags.iter().enumerate() {
            let n = index + 1;
            extra.push(member(&format!("Tags.member.{n}.Key"), key));
            extra.push(member(&format!("Tags.member.{n}.Value"), value));
        }

        let (status, body) =
            create_stack(&router, case.stack_name, &case.template, &extra).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to succeed, got {status} {body}",
            case.case
        );
        let stack_id = xml_text(&body, "StackId")
            .unwrap_or_else(|| panic!("case {:?}: CreateStack answered no StackId: {body}", case.case));
        assert_stack_id_shape(case.case, &stack_id, case.stack_name);

        // Addressed by name and by stack id, a create must be readable back as what was
        // created.
        for identifier in [case.stack_name.to_string(), stack_id.clone()] {
            let (status, body) = describe_stack(&router, &identifier).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected DescribeStacks({identifier}) to succeed, got {status} {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "StackId").as_deref(),
                Some(stack_id.as_str()),
                "case {:?}: DescribeStacks({identifier}) must report the created stack id, got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "StackName").as_deref(),
                Some(case.stack_name),
                "case {:?}: DescribeStacks({identifier}) must report the created name, got {body}",
                case.case
            );
            assert!(
                xml_text(&body, "CreationTime").is_some(),
                "case {:?}: DescribeStacks({identifier}) must report a CreationTime, got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "Description").as_deref(),
                case.description,
                "case {:?}: DescribeStacks({identifier}) must report the template's Description, got {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "StackDriftStatus").as_deref(),
                Some("NOT_CHECKED"),
                "case {:?}: a stack never checked for drift is NOT_CHECKED, got {body}",
                case.case
            );
            assert_eq!(
                xml_texts(&body, "ParameterKey"),
                keys(&case.parameters),
                "case {:?}: DescribeStacks({identifier}) must report the stored parameter keys, got {body}",
                case.case
            );
            assert_eq!(
                xml_texts(&body, "ParameterValue"),
                values(&case.parameters),
                "case {:?}: DescribeStacks({identifier}) must report the stored parameter values, got {body}",
                case.case
            );
            assert_eq!(
                xml_texts(&body, "Key"),
                keys(&case.tags),
                "case {:?}: DescribeStacks({identifier}) must report the stored tag keys, got {body}",
                case.case
            );
            assert_eq!(
                xml_texts(&body, "Value"),
                values(&case.tags),
                "case {:?}: DescribeStacks({identifier}) must report the stored tag values, got {body}",
                case.case
            );

            // The template body is persisted verbatim: a byte-identical round trip.
            let (status, body) = get_template(&router, &identifier, None).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected GetTemplate({identifier}) to succeed, got {status} {body}",
                case.case
            );
            assert_eq!(
                xml_text(&body, "TemplateBody").as_deref(),
                Some(case.template.as_str()),
                "case {:?}: GetTemplate({identifier}) must answer the exact body that was submitted, got {body}",
                case.case
            );
            assert_eq!(
                xml_texts(&body, "member"),
                vec!["Original".to_string(), "Processed".to_string()],
                "case {:?}: both template stages are available for a stack, got {body}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_created_stack_is_in_progress_before_the_transition_and_complete_after_it() {
    let router = router();
    let started = Instant::now();
    let (_, created) = create_stack(&router, "settling", "{}", &[]).await;
    let stack_id = xml_text(&created, "StackId").expect("a stack id");

    let (_, body) = describe_stack(&router, "settling").await;
    let owed = expected_status(started, "CREATE_IN_PROGRESS", "CREATE_COMPLETE");
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(owed),
        "case \"immediately after create\": expected {owed} on a read taken this soon, got {body}"
    );

    wait_for_the_transition().await;

    let (_, body) = describe_stack(&router, &stack_id).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case \"after the transition is due\": the stack must have settled, got {body}"
    );
}

#[tokio::test]
async fn create_stack_enforces_the_documented_stack_name_rule() {
    let router = router();

    // The rule, from `CreateStackInput$StackName`: alphanumeric characters and hyphens
    // only, starting with a letter, at most 128 characters. Each bound is tested at the
    // published value and one past it.
    let long_128 = "a".repeat(128);
    let long_129 = "a".repeat(129);
    let accepted: Vec<(&str, &str)> = vec![
        ("single letter", "a"),
        ("letters, digits and hyphens", "Stack-9-x"),
        ("at the published length bound", &long_128),
    ];
    let refused: Vec<(&str, &str)> = vec![
        ("leading digit", "1stack"),
        ("leading hyphen", "-stack"),
        ("underscore", "my_stack"),
        ("space", "my stack"),
        ("dot", "my.stack"),
        ("one past the published length bound", &long_129),
    ];

    for (case, name) in accepted {
        let (status, body) = create_stack(&router, name, "{}", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {name:?} satisfies the stack name rule, got {status} {body}"
        );
    }
    for (case, name) in refused {
        let (status, body) = create_stack(&router, name, "{}", &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {name:?} breaks the stack name rule, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: a refused stack name is a ValidationError, got {body}"
        );
        let (status, _) = describe_stack(&router, name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create must not have stored a stack"
        );
    }
}

#[tokio::test]
async fn create_stack_needs_exactly_one_template_source() {
    let router = router();
    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        ("neither TemplateBody nor TemplateURL", vec![]),
        (
            "both TemplateBody and TemplateURL",
            vec![
                member("TemplateBody", "{}"),
                member("TemplateURL", "https://example.test/t.json"),
            ],
        ),
    ];
    for (case, extra) in cases {
        let mut members = vec![member("StackName", "no-template")];
        members.extend(extra);
        let (status, body) = call(&router, "CreateStack", &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the request names no single template source, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected a ValidationError, got {body}"
        );
    }
}

#[tokio::test]
async fn a_live_stack_name_is_taken_and_is_free_again_once_its_deletion_completes() {
    let router = router();
    let (status, first) = create_stack(&router, "unique", "{}", &[]).await;
    assert_eq!(status, 200, "the first create must succeed, got {first}");
    let first_id = xml_text(&first, "StackId").expect("a stack id");

    let (status, body) = create_stack(&router, "unique", "{}", &[]).await;
    assert_eq!(
        status, 400,
        "case \"name of a live stack\": the second create must be refused, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("AlreadyExistsException"),
        "case \"name of a live stack\": expected AlreadyExistsException, got {body}"
    );

    wait_for_the_transition().await;
    let (status, body) = call(&router, "DeleteStack", &[member("StackName", "unique")]).await;
    assert_eq!(status, 200, "DeleteStack must succeed, got {status} {body}");
    wait_for_the_transition().await;

    let (status, second) = create_stack(&router, "unique", "{}", &[]).await;
    assert_eq!(
        status, 200,
        "case \"name of a deleted stack\": the name is free again, got {status} {second}"
    );
    let second_id = xml_text(&second, "StackId").expect("a stack id");
    assert_ne!(
        first_id, second_id,
        "case \"name of a deleted stack\": the new stack must have its own stack id"
    );
}

#[tokio::test]
async fn an_identifier_that_names_no_live_stack_is_refused_with_validation_error() {
    let router = router();
    let (_, created) = create_stack(&router, "gone", "{}", &[]).await;
    let stack_id = xml_text(&created, "StackId").expect("a stack id");
    wait_for_the_transition().await;
    call(&router, "DeleteStack", &[member("StackName", "gone")]).await;
    wait_for_the_transition().await;

    let cases = vec![
        ("a name never created", "no-such-stack".to_string()),
        ("the NAME of a deleted stack", "gone".to_string()),
        (
            "a stack id no stack carries",
            format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/ghost/00000000-0000-0000-0000-000000000000"),
        ),
    ];
    for (case, identifier) in cases {
        // The reads of the stack's own record and of its template introduce the name with
        // "with id"; the read of its EVENT LOG brackets it instead — the template belongs to
        // the operation (`stack_not_found_wording.rs` holds the bracketed side in depth).
        for (action, expected) in [
            ("DescribeStacks", format!("Stack with id {identifier} does not exist")),
            ("GetTemplate", format!("Stack with id {identifier} does not exist")),
            ("DescribeStackEvents", format!("Stack [{identifier}] does not exist")),
        ] {
            let (status, body) =
                call(&router, action, &[member("StackName", &identifier)]).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {action} must refuse {identifier:?}, got {status} {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: {action} must answer ValidationError for {identifier:?}, got {body}"
            );
            assert_eq!(
                xml_text(&body, "Message").as_deref(),
                Some(expected.as_str()),
                "case {case:?}: {action} must name the stack it could not find, got {body}"
            );
        }
    }

    // The deleted stack stays addressable by its stack ID, which is the one difference
    // between the two spellings of its identifier.
    let (status, body) = describe_stack(&router, &stack_id).await;
    assert_eq!(
        status, 200,
        "case \"the ID of a deleted stack\": it stays addressable, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("DELETE_COMPLETE"),
        "case \"the ID of a deleted stack\": it reports DELETE_COMPLETE, got {body}"
    );
    assert!(
        xml_text(&body, "DeletionTime").is_some(),
        "case \"the ID of a deleted stack\": it reports a DeletionTime, got {body}"
    );
}

#[tokio::test]
async fn describe_stacks_without_a_stack_name_answers_every_live_stack_in_creation_order() {
    let router = router();
    for name in ["first", "second", "third"] {
        create_stack(&router, name, "{}", &[]).await;
    }
    wait_for_the_transition().await;
    call(&router, "DeleteStack", &[member("StackName", "second")]).await;
    wait_for_the_transition().await;

    let (status, body) = call(&router, "DescribeStacks", &[]).await;
    assert_eq!(status, 200, "expected DescribeStacks to succeed, got {status} {body}");
    assert_eq!(
        xml_texts(&body, "StackName"),
        vec!["first".to_string(), "third".to_string()],
        "case \"one of three stacks deleted\": only the live stacks, in creation order, got {body}"
    );
}

#[tokio::test]
async fn list_stacks_filters_on_stack_status_filter() {
    let router = router();
    // One stack per terminal status this slice produces: created, updated, deleted.
    for name in ["kept", "updated", "removed"] {
        create_stack(&router, name, "{}", &[]).await;
    }
    wait_for_the_transition().await;
    call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "updated"),
            member("TemplateBody", r#"{"Description":"v2"}"#),
        ],
    )
    .await;
    call(&router, "DeleteStack", &[member("StackName", "removed")]).await;
    wait_for_the_transition().await;

    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("no filter: every stack, deleted ones included", vec![], vec!["kept", "updated", "removed"]),
        ("one status", vec!["CREATE_COMPLETE"], vec!["kept"]),
        ("a different status", vec!["UPDATE_COMPLETE"], vec!["updated"]),
        ("deleted stacks are still summarised", vec!["DELETE_COMPLETE"], vec!["removed"]),
        (
            "two statuses",
            vec!["CREATE_COMPLETE", "DELETE_COMPLETE"],
            vec!["kept", "removed"],
        ),
        ("a status no stack is in", vec!["ROLLBACK_FAILED"], vec![]),
    ];

    for (case, filter, expected) in cases {
        let members: Vec<(String, String)> = filter
            .iter()
            .enumerate()
            .map(|(index, status)| {
                member(&format!("StackStatusFilter.member.{}", index + 1), status)
            })
            .collect();
        let (status, body) = call(&router, "ListStacks", &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListStacks to succeed, got {status} {body}"
        );
        assert_eq!(
            xml_texts(&body, "StackName"),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: ListStacks must report exactly the matching stacks, got {body}"
        );
    }

    // A deleted stack's summary carries the instant its deletion completed.
    let (_, body) = call(
        &router,
        "ListStacks",
        &[member("StackStatusFilter.member.1", "DELETE_COMPLETE")],
    )
    .await;
    assert!(
        xml_text(&body, "DeletionTime").is_some(),
        "case \"deleted stacks are still summarised\": the summary carries a DeletionTime, got {body}"
    );
    assert_eq!(
        xml_text(&body, "TemplateDescription").as_deref(),
        None,
        "case \"deleted stacks are still summarised\": this template declares no Description, got {body}"
    );
}

/// One row of the update table: what the request restates, and what the stack must look
/// like afterwards.
struct UpdateCase {
    case: &'static str,
    stack_name: &'static str,
    members: Vec<(String, String)>,
    expected_template: &'static str,
    expected_parameters: Vec<(String, String)>,
    expected_tags: Vec<(String, String)>,
}

#[tokio::test]
async fn update_stack_mutates_the_same_stack_and_is_observable_through_the_reads() {
    const ORIGINAL: &str =
        r#"{"Description":"v1","Parameters":{"Alpha":{"Type":"String"},"Beta":{"Type":"String"}}}"#;
    const REPLACEMENT: &str = r#"{"Description":"v2","Resources":{"T2":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

    let cases = vec![
        UpdateCase {
            case: "a new template body, parameters and tags left alone",
            stack_name: "upd-body",
            members: vec![member("TemplateBody", REPLACEMENT)],
            expected_template: REPLACEMENT,
            expected_parameters: kv(&[("Alpha", "1"), ("Beta", "2")]),
            expected_tags: kv(&[("env", "dev")]),
        },
        UpdateCase {
            case: "the previous template, one parameter changed and one kept",
            stack_name: "upd-params",
            members: vec![
                member("UsePreviousTemplate", "true"),
                member("Parameters.member.1.ParameterKey", "Alpha"),
                member("Parameters.member.1.ParameterValue", "99"),
                member("Parameters.member.2.ParameterKey", "Beta"),
                member("Parameters.member.2.UsePreviousValue", "true"),
            ],
            expected_template: ORIGINAL,
            expected_parameters: kv(&[("Alpha", "99"), ("Beta", "2")]),
            expected_tags: kv(&[("env", "dev")]),
        },
        UpdateCase {
            case: "a new template body and the tags replaced",
            stack_name: "upd-tags",
            members: vec![
                member("TemplateBody", REPLACEMENT),
                member("Tags.member.1.Key", "env"),
                member("Tags.member.1.Value", "staging"),
                member("Tags.member.2.Key", "owner"),
                member("Tags.member.2.Value", "platform"),
            ],
            expected_template: REPLACEMENT,
            expected_parameters: kv(&[("Alpha", "1"), ("Beta", "2")]),
            expected_tags: kv(&[("env", "staging"), ("owner", "platform")]),
        },
    ];

    let router = router();
    for case in &cases {
        let (status, created) = create_stack(
            &router,
            case.stack_name,
            ORIGINAL,
            &[
                member("Parameters.member.1.ParameterKey", "Alpha"),
                member("Parameters.member.1.ParameterValue", "1"),
                member("Parameters.member.2.ParameterKey", "Beta"),
                member("Parameters.member.2.ParameterValue", "2"),
                member("Tags.member.1.Key", "env"),
                member("Tags.member.1.Value", "dev"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the stack under test must be created, got {created}",
            case.case
        );
    }
    wait_for_the_transition().await;

    for case in &cases {
        let (_, created) = describe_stack(&router, case.stack_name).await;
        let stack_id = xml_text(&created, "StackId").expect("a stack id");

        let mut members = vec![member("StackName", case.stack_name)];
        members.extend(case.members.iter().cloned());
        let started = Instant::now();
        let (status, body) = call(&router, "UpdateStack", &members).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected UpdateStack to succeed, got {status} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "StackId").as_deref(),
            Some(stack_id.as_str()),
            "case {:?}: an update must mutate the SAME stack, got {body}",
            case.case
        );

        let (_, body) = describe_stack(&router, case.stack_name).await;
        let owed = expected_status(started, "UPDATE_IN_PROGRESS", "UPDATE_COMPLETE");
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(owed),
            "case {:?}: expected {owed} on a read taken this soon after the update, got {body}",
            case.case
        );
        wait_for_the_transition().await;

        let (_, body) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("UPDATE_COMPLETE"),
            "case {:?}: the update must have settled, got {body}",
            case.case
        );
        assert!(
            xml_text(&body, "LastUpdatedTime").is_some(),
            "case {:?}: an updated stack reports a LastUpdatedTime, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "ParameterKey"),
            keys(&case.expected_parameters),
            "case {:?}: the merged parameter keys, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "ParameterValue"),
            values(&case.expected_parameters),
            "case {:?}: the merged parameter values, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "Key"),
            keys(&case.expected_tags),
            "case {:?}: the stack's tags after the update, got {body}",
            case.case
        );
        assert_eq!(
            xml_texts(&body, "Value"),
            values(&case.expected_tags),
            "case {:?}: the stack's tag values after the update, got {body}",
            case.case
        );

        let (_, body) = get_template(&router, case.stack_name, None).await;
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(case.expected_template),
            "case {:?}: GetTemplate must answer the template the update left in place, got {body}",
            case.case
        );
    }
}

#[tokio::test]
async fn update_stack_refuses_what_it_cannot_apply() {
    let router = router();
    let started = Instant::now();
    create_stack(&router, "refusals", r#"{"Description":"v1"}"#, &[]).await;

    // While the create is still in flight, an update cannot start. Once the create has
    // settled the same call is accepted, so the case is only asserted when the request
    // really did reach the service before the due instant.
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "refusals"),
            member("TemplateBody", r#"{"Description":"v2"}"#),
        ],
    )
    .await;
    if started.elapsed() < STATUS_TRANSITION_DELAY {
        assert_eq!(
            status, 400,
            "case \"while the stack is CREATE_IN_PROGRESS\": the update is refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case \"while the stack is CREATE_IN_PROGRESS\": expected ValidationError, got {body}"
        );
    }

    wait_for_the_transition().await;

    // Whatever the race above did, the stack is back on `{"Description":"v1"}` for the
    // table below: an update that went through is undone by updating back.
    if status == 200 {
        call(
            &router,
            "UpdateStack",
            &[
                member("StackName", "refusals"),
                member("TemplateBody", r#"{"Description":"v1"}"#),
            ],
        )
        .await;
        wait_for_the_transition().await;
    }

    let cases: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "an update that changes nothing",
            vec![member("TemplateBody", r#"{"Description":"v1"}"#)],
        ),
        (
            "the previous template with nothing else restated",
            vec![member("UsePreviousTemplate", "true")],
        ),
        ("no template source at all", vec![]),
        (
            "two template sources",
            vec![
                member("TemplateBody", "{}"),
                member("UsePreviousTemplate", "true"),
            ],
        ),
        (
            "a previous value for a parameter the stack does not have",
            vec![
                member("TemplateBody", "{}"),
                member("Parameters.member.1.ParameterKey", "Nope"),
                member("Parameters.member.1.UsePreviousValue", "true"),
            ],
        ),
    ];
    for (case, extra) in cases {
        let mut members = vec![member("StackName", "refusals")];
        members.extend(extra);
        let (status, body) = call(&router, "UpdateStack", &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the update must be refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
    }

    // Every refusal above left the stack exactly as it was.
    let (_, body) = get_template(&router, "refusals", None).await;
    assert_eq!(
        xml_text(&body, "TemplateBody").as_deref(),
        Some(r#"{"Description":"v1"}"#),
        "a refused update must not have changed the stored template, got {body}"
    );
    let (_, body) = describe_stack(&router, "refusals").await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "a refused update must not have moved the stack's status, got {body}"
    );
}

#[tokio::test]
async fn delete_stack_is_refused_while_termination_protection_is_enabled() {
    let router = router();
    create_stack(
        &router,
        "protected",
        "{}",
        &[member("EnableTerminationProtection", "true")],
    )
    .await;
    wait_for_the_transition().await;

    let (status, body) = call(&router, "DeleteStack", &[member("StackName", "protected")]).await;
    assert_eq!(
        status, 400,
        "case \"termination protection enabled\": the delete is refused, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ValidationError"),
        "case \"termination protection enabled\": expected ValidationError, got {body}"
    );
    assert!(
        xml_text(&body, "Message").is_some_and(|m| m.contains("TerminationProtection")),
        "case \"termination protection enabled\": the message must say why, got {body}"
    );

    wait_for_the_transition().await;
    let (status, body) = describe_stack(&router, "protected").await;
    assert_eq!(
        status, 200,
        "case \"termination protection enabled\": the stack remains unchanged, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case \"termination protection enabled\": the refused delete moved no status, got {body}"
    );
    assert_eq!(
        xml_text(&body, "EnableTerminationProtection").as_deref(),
        Some("true"),
        "case \"termination protection enabled\": the flag is read back from the store, got {body}"
    );
}

#[tokio::test]
async fn deleting_a_stack_that_is_not_in_the_live_set_succeeds_and_changes_nothing() {
    let router = router();
    let cases = vec![
        ("a name no stack ever had", "never-existed".to_string()),
        (
            "a stack id no stack carries",
            format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/ghost/00000000-0000-0000-0000-000000000000"),
        ),
    ];
    for (case, identifier) in cases {
        let (status, body) =
            call(&router, "DeleteStack", &[member("StackName", &identifier)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: deleting what is already absent succeeds, got {status} {body}"
        );
    }

    let (status, body) = call(&router, "ListStacks", &[]).await;
    assert_eq!(status, 200, "expected ListStacks to succeed, got {status} {body}");
    assert!(
        xml_texts(&body, "StackName").is_empty(),
        "a delete of an absent stack must not have invented one, got {body}"
    );
}

#[tokio::test]
async fn describe_stack_events_reports_the_recorded_events_newest_first() {
    let router = router();
    let started = Instant::now();
    let (_, created) = create_stack(&router, "eventful", r#"{"Description":"v1"}"#, &[]).await;
    let stack_id = xml_text(&created, "StackId").expect("a stack id");

    let (status, body) = call(
        &router,
        "DescribeStackEvents",
        &[member("StackName", "eventful")],
    )
    .await;
    assert_eq!(status, 200, "expected DescribeStackEvents to succeed, got {status} {body}");
    // Newest first: the opening event, preceded by the settled one once the transition is
    // due — which is the only thing the clock can have added by now.
    let owed_events: Vec<String> = if started.elapsed() < STATUS_TRANSITION_DELAY {
        vec!["CREATE_IN_PROGRESS".to_string()]
    } else {
        vec![
            "CREATE_COMPLETE".to_string(),
            "CREATE_IN_PROGRESS".to_string(),
        ]
    };
    assert_eq!(
        xml_texts(&body, "ResourceStatus"),
        owed_events,
        "case \"immediately after create\": expected {owed_events:?}, got {body}"
    );
    assert_eq!(
        xml_text(&body, "ResourceType").as_deref(),
        Some("AWS::CloudFormation::Stack"),
        "case \"immediately after create\": the stack's own resource type, got {body}"
    );
    assert_eq!(
        xml_text(&body, "LogicalResourceId").as_deref(),
        Some("eventful"),
        "case \"immediately after create\": the stack's logical id is its name, got {body}"
    );
    assert_eq!(
        xml_text(&body, "PhysicalResourceId").as_deref(),
        Some(stack_id.as_str()),
        "case \"immediately after create\": the stack's physical id is its stack id, got {body}"
    );
    assert!(
        xml_text(&body, "EventId").is_some(),
        "case \"immediately after create\": every event carries an EventId, got {body}"
    );

    wait_for_the_transition().await;
    call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "eventful"),
            member("TemplateBody", r#"{"Description":"v2"}"#),
        ],
    )
    .await;
    wait_for_the_transition().await;
    call(&router, "DeleteStack", &[member("StackName", "eventful")]).await;
    wait_for_the_transition().await;

    let (_, body) = call(
        &router,
        "DescribeStackEvents",
        &[member("StackName", &stack_id)],
    )
    .await;
    assert_eq!(
        xml_texts(&body, "ResourceStatus"),
        vec![
            "DELETE_COMPLETE".to_string(),
            "DELETE_IN_PROGRESS".to_string(),
            "UPDATE_COMPLETE".to_string(),
            "UPDATE_IN_PROGRESS".to_string(),
            "CREATE_COMPLETE".to_string(),
            "CREATE_IN_PROGRESS".to_string(),
        ],
        "case \"the whole lifecycle\": every event, newest first, got {body}"
    );
    let event_ids = xml_texts(&body, "EventId");
    let mut unique = event_ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        event_ids.len(),
        "case \"the whole lifecycle\": every EventId is distinct, got {event_ids:?}"
    );
}

#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let router = router();
    create_stack(&router, "paged", "{}", &[]).await;

    for action in ["DescribeStacks", "ListStacks", "DescribeStackEvents"] {
        let mut members = vec![member("NextToken", "not-a-token-we-issued")];
        if action == "DescribeStackEvents" {
            members.push(member("StackName", "paged"));
        }
        let (status, body) = call(&router, action, &members).await;
        assert_eq!(
            status, 400,
            "case {action:?}: a token this service never issued is refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {action:?}: expected ValidationError, got {body}"
        );
    }
}

/// A JSON template body of exactly `bytes` bytes: `{"Metadata":{"Pad":"<pad>"}}`, whose
/// wrapper is 23 bytes (`{"Metadata":{"Pad":"` plus `"}}`). `pad` must be one ASCII byte.
///
/// The padding goes in `Metadata` — a section whose content the template format bounds by
/// nothing — rather than in `Description`, which a template may carry only 1024 bytes of.
fn template_of_size(bytes: usize, pad: &str) -> String {
    const WRAPPER_BYTES: usize = 23;
    assert_eq!(pad.len(), 1, "the padding must be a single-byte string");
    format!(
        "{{\"Metadata\":{{\"Pad\":\"{}\"}}}}",
        pad.repeat(bytes - WRAPPER_BYTES)
    )
}

#[tokio::test]
async fn a_stored_template_body_is_bounded_by_its_documented_maximum() {
    let router = router();

    // `CreateStackInput$TemplateBody`: "a minimum length of 1 byte and a maximum length of
    // 51,200 bytes". The published bound and one byte past it, on create and on update
    // alike (the model carries no `@length` maximum, so the handler is what enforces it).
    let cases: Vec<(&str, usize, bool)> = vec![
        ("a small body", 64, true),
        ("at the published maximum", TEMPLATE_BODY_MAX_BYTES, true),
        (
            "one byte past the published maximum",
            TEMPLATE_BODY_MAX_BYTES + 1,
            false,
        ),
    ];

    for (case, size, within_the_bound) in cases {
        let name = format!("sized-{size}");
        let (status_code, body) =
            create_stack(&router, &name, &template_of_size(size, "d"), &[]).await;
        if within_the_bound {
            assert_eq!(
                status_code, 200,
                "case {case:?}: a {size}-byte body is within the bound, got {status_code} {body}"
            );
            wait_for_the_transition().await;
            let (update_status, update_body) = call(
                &router,
                "UpdateStack",
                &[
                    member("StackName", &name),
                    member("TemplateBody", &template_of_size(size, "e")),
                ],
            )
            .await;
            assert_eq!(
                update_status, 200,
                "case {case:?}: UpdateStack accepts the same {size}-byte body, got \
                 {update_status} {update_body}"
            );
        } else {
            assert_eq!(
                status_code, 400,
                "case {case:?}: a {size}-byte body is past the bound, got {status_code} {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?}: expected ValidationError, got {body}"
            );
            create_stack(&router, "sized-target", "{}", &[]).await;
            wait_for_the_transition().await;
            let (update_status, update_body) = call(
                &router,
                "UpdateStack",
                &[
                    member("StackName", "sized-target"),
                    member("TemplateBody", &template_of_size(size, "d")),
                ],
            )
            .await;
            assert_eq!(
                update_status, 400,
                "case {case:?}: UpdateStack refuses the same {size}-byte body, got \
                 {update_status} {update_body}"
            );
        }
    }
}

#[tokio::test]
async fn a_paginated_read_carries_one_bounded_page_and_a_token_that_resumes_after_it() {
    let router = router();
    let total = PAGE_SIZE + 3;
    for index in 0..total {
        let name = format!("page-{index:04}");
        let (status_code, body) = create_stack(&router, &name, "{}", &[]).await;
        assert_eq!(
            status_code, 200,
            "case {name:?}: the stack must be created, got {status_code} {body}"
        );
    }

    // Both `@paginated` stack reads: one response carries at most PAGE_SIZE items and a
    // token, and following the token enumerates the rest exactly once.
    for action in ["ListStacks", "DescribeStacks"] {
        let (status_code, first) = call(&router, action, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {action:?}: the first page is served, got {status_code} {first}"
        );
        let first_names = xml_texts(&first, "StackName");
        assert_eq!(
            first_names.len(),
            PAGE_SIZE,
            "case {action:?}: one page carries at most {PAGE_SIZE} items, got {}",
            first_names.len()
        );
        let token = xml_text(&first, "NextToken").unwrap_or_else(|| {
            panic!("case {action:?}: a page with more behind it must carry a NextToken, got {first}")
        });

        let (status_code, second) = call(&router, action, &[member("NextToken", &token)]).await;
        assert_eq!(
            status_code, 200,
            "case {action:?}: the token resumes the read, got {status_code} {second}"
        );
        let second_names = xml_texts(&second, "StackName");
        assert_eq!(
            second_names.len(),
            total - PAGE_SIZE,
            "case {action:?}: the last page carries the remainder, got {second_names:?}"
        );
        assert_eq!(
            xml_text(&second, "NextToken"),
            None,
            "case {action:?}: the last page carries no NextToken, got {second}"
        );

        let mut paged = first_names.clone();
        paged.extend(second_names);
        let distinct: std::collections::BTreeSet<String> = paged.iter().cloned().collect();
        assert_eq!(
            distinct.len(),
            total,
            "case {action:?}: the pages enumerate every stack exactly once, got {} names",
            paged.len()
        );
    }

    // A cursor past the end of the store is not one this service handed out for it, and
    // neither is one issued for a different collection.
    let beyond_the_end = state::encode_next_token(state::PageOf::StackSummaries, total + 1);
    let other_collection = state::encode_next_token(state::PageOf::StackEvents, 0);
    for (case, token) in [
        ("a cursor past the end", beyond_the_end),
        ("a cursor issued for another collection", other_collection),
    ] {
        let (status_code, body) = call(&router, "ListStacks", &[member("NextToken", &token)]).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: refused, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
    }
}

#[tokio::test]
async fn create_stack_refuses_a_stack_past_the_stores_stack_quota() {
    let (router, engine) = router_with_engine();
    let now = SystemTime::now();
    {
        let state = store(&engine);
        let mut held = state.lock();
        for index in 0..MAX_STACKS {
            let name = format!("filler-{index:05}");
            held.stacks.push(StoredStack::new(
                format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/{name}/{index:05}"),
                name,
                Some("{}".to_string()),
                now,
            ));
        }
    }

    let (status_code, body) = create_stack(&router, "one-too-many", "{}", &[]).await;
    assert_eq!(
        status_code, 400,
        "case \"the store is at its stack quota\": the create is refused, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("LimitExceededException"),
        "case \"the store is at its stack quota\": expected LimitExceededException, got {body}"
    );
    assert_eq!(
        store(&engine).lock().stacks.len(),
        MAX_STACKS,
        "case \"the store is at its stack quota\": the refused create stored nothing"
    );
}

#[tokio::test]
async fn one_stacks_event_log_is_bounded_and_read_back_one_page_at_a_time() {
    let (router, engine) = router_with_engine();
    create_stack(&router, "bounded-events", "{}", &[]).await;

    let appended = MAX_EVENTS_PER_STACK + 25;
    {
        let state = store(&engine);
        let mut held = state.lock();
        let stack = held
            .resolve_mut(REGION, "bounded-events")
            .expect("the stack just created");
        for index in 0..appended {
            stack.record_event(
                SystemTime::now(),
                "UPDATE_IN_PROGRESS",
                Some(format!("event {index}")),
                None,
                None,
            );
        }
        assert_eq!(
            stack.events.len(),
            MAX_EVENTS_PER_STACK,
            "case \"past the event bound\": the log holds at most {MAX_EVENTS_PER_STACK} events"
        );
        assert!(
            stack
                .events
                .iter()
                .all(|event| event.resource_status_reason.as_deref() != Some("event 0")),
            "case \"past the event bound\": the oldest events are the ones reclaimed"
        );
    }

    let (status_code, first) = call(
        &router,
        "DescribeStackEvents",
        &[member("StackName", "bounded-events")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"a long event log\": the first page is served, got {status_code} {first}"
    );
    assert_eq!(
        xml_texts(&first, "EventId").len(),
        PAGE_SIZE,
        "case \"a long event log\": one response carries at most {PAGE_SIZE} events"
    );
    let token = xml_text(&first, "NextToken")
        .unwrap_or_else(|| panic!("case \"a long event log\": expected a NextToken, got {first}"));
    let (status_code, second) = call(
        &router,
        "DescribeStackEvents",
        &[
            member("StackName", "bounded-events"),
            member("NextToken", &token),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"a long event log\": the token resumes the read, got {status_code} {second}"
    );
    assert_eq!(
        xml_texts(&second, "EventId").len(),
        PAGE_SIZE,
        "case \"a long event log\": the second page is bounded too, got {second}"
    );
    let first_ids = xml_texts(&first, "EventId");
    assert!(
        xml_texts(&second, "EventId")
            .iter()
            .all(|id| !first_ids.contains(id)),
        "case \"a long event log\": the pages do not repeat an event"
    );
}

#[tokio::test]
async fn completed_deletions_are_retained_only_up_to_the_retention_bound() {
    let (router, engine) = router_with_engine();
    let retired = MAX_DELETED_STACKS_RETAINED + 10;
    let base = SystemTime::now() - Duration::from_secs(10_000);
    {
        let state = store(&engine);
        let mut held = state.lock();
        for index in 0..retired {
            let name = format!("retired-{index:05}");
            let deleted_at = base + Duration::from_secs(index as u64);
            let mut stack = StoredStack::new(
                format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/{name}/{index:05}"),
                name,
                Some("{}".to_string()),
                deleted_at,
            );
            stack.status = status::DELETE_COMPLETE.to_string();
            stack.deletion_time = Some(deleted_at);
            held.stacks.push(stack);
        }
    }

    // Any call settles the store, which is also where the retention bound is enforced.
    let (status_code, body) = call(&router, "ListStacks", &[]).await;
    assert_eq!(
        status_code, 200,
        "case \"past the retention bound\": the list is served, got {status_code} {body}"
    );
    assert_eq!(
        store(&engine).lock().stacks.len(),
        MAX_DELETED_STACKS_RETAINED,
        "case \"past the retention bound\": the store keeps at most \
         {MAX_DELETED_STACKS_RETAINED} completed deletions"
    );
    let kept: Vec<String> = store(&engine)
        .lock()
        .stacks
        .iter()
        .map(|stack| stack.stack_name.clone())
        .collect();
    assert!(
        !kept.contains(&"retired-00000".to_string()),
        "case \"past the retention bound\": the oldest deletion is reclaimed first"
    );
    assert!(
        kept.contains(&format!("retired-{:05}", retired - 1)),
        "case \"past the retention bound\": the newest deletion is retained, got {kept:?}"
    );
}

#[tokio::test]
async fn a_stack_policy_round_trips_and_a_policy_url_is_read_from_amazon_s3() {
    let router = router();

    // The class: any policy document a caller supplies on create or update is the one
    // `GetStackPolicy` reports back; a stack with no policy reports none ("If a stack
    // doesn't have a policy, a null value is returned").
    let created = r#"{"Statement":[{"Effect":"Deny","Action":"Update:*","Principal":"*","Resource":"*"}]}"#;
    let updated = r#"{"Statement":[{"Effect":"Allow","Action":"Update:Modify","Principal":"*","Resource":"*"}]}"#;

    let (status_code, body) = create_stack(
        &router,
        "policed",
        "{}",
        &[member("StackPolicyBody", created)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"create with a policy\": expected success, got {status_code} {body}"
    );
    let (status_code, body) = call(&router, "GetStackPolicy", &[member("StackName", "policed")]).await;
    assert_eq!(
        status_code, 200,
        "case \"create with a policy\": GetStackPolicy answers, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackPolicyBody").as_deref(),
        Some(created),
        "case \"create with a policy\": the stored policy is answered verbatim, got {body}"
    );

    wait_for_the_transition().await;
    let (status_code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "policed"),
            member("TemplateBody", r#"{"Description":"v2"}"#),
            member("StackPolicyBody", updated),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"update with a new policy\": expected success, got {status_code} {body}"
    );
    let (_, body) = call(&router, "GetStackPolicy", &[member("StackName", "policed")]).await;
    assert_eq!(
        xml_text(&body, "StackPolicyBody").as_deref(),
        Some(updated),
        "case \"update with a new policy\": the update replaced the stored policy, got {body}"
    );

    create_stack(&router, "unpoliced", "{}", &[]).await;
    let (status_code, body) = call(
        &router,
        "GetStackPolicy",
        &[member("StackName", "unpoliced")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"a stack with no policy\": GetStackPolicy answers, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackPolicyBody"),
        None,
        "case \"a stack with no policy\": no policy is reported, got {body}"
    );

    let (status_code, body) = call(
        &router,
        "GetStackPolicy",
        &[member("StackName", "no-such-stack")],
    )
    .await;
    assert_eq!(
        status_code, 400,
        "case \"a stack that does not exist\": refused, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ValidationError"),
        "case \"a stack that does not exist\": expected ValidationError, got {body}"
    );

    // `SetStackPolicy` is the other way the policy in force is set, and the rule is the
    // same: the document set LAST is the one `GetStackPolicy` reports. The rows span the
    // documents a caller may set, on a stack that already has a policy and on one that
    // does not.
    let set_cases: Vec<(&str, &str, &str)> = vec![
        (
            "a deny-everything policy on a stack that had none",
            "unpoliced",
            r#"{"Statement":[{"Effect":"Deny","Action":"Update:*","Principal":"*","Resource":"*"}]}"#,
        ),
        (
            "a policy that replaces the one an update installed",
            "policed",
            r#"{"Statement":[{"Effect":"Allow","Action":"Update:*","Principal":"*","Resource":"*"}]}"#,
        ),
        (
            "several statements, a condition and a resource path",
            "unpoliced",
            r#"{"Statement":[{"Effect":"Allow","Action":"Update:Modify","Principal":"*","Resource":"*"},{"Effect":"Deny","Action":"Update:Replace","Principal":"*","Resource":"LogicalResourceId/Db","Condition":{"StringEquals":{"ResourceType":["AWS::SSM::Parameter"]}}}]}"#,
        ),
        (
            "a document carrying unicode, quotes and an ampersand",
            "unpoliced",
            r#"{"Statement":[],"Note":"café \"quoted\" & <angled>"}"#,
        ),
        (
            "a one-character document, the modelled minimum length",
            "policed",
            "x",
        ),
    ];
    for (case, stack, policy) in set_cases {
        let (status_code, body) = call(
            &router,
            "SetStackPolicy",
            &[
                member("StackName", stack),
                member("StackPolicyBody", policy),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected SetStackPolicy to succeed, got {status_code} {body}"
        );
        let (status_code, body) = call(&router, "GetStackPolicy", &[member("StackName", stack)]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: GetStackPolicy answers, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "StackPolicyBody").as_deref(),
            Some(policy),
            "case {case:?}: the policy that was set is the one reported back, got {body}"
        );
    }

    // A `SetStackPolicy` the service cannot carry out changes nothing: the policy set last
    // is still the one in force after each refusal.
    let in_force = "x";
    /// One refusal row: the request, the status it must answer and the error code it must
    /// carry (none for the 501, whose body is the scaffold's).
    struct PolicyRefusal {
        case: &'static str,
        members: Vec<(String, String)>,
        status: u16,
        code: Option<&'static str>,
    }
    let refusals = vec![
        PolicyRefusal {
            case: "both StackPolicyBody and StackPolicyURL",
            members: vec![
                member("StackName", "policed"),
                member("StackPolicyBody", "{}"),
                member("StackPolicyURL", "https://example.s3.amazonaws.com/policy.json"),
            ],
            status: 400,
            code: Some("ValidationError"),
        },
        PolicyRefusal {
            case: "neither StackPolicyBody nor StackPolicyURL",
            members: vec![member("StackName", "policed")],
            status: 400,
            code: Some("ValidationError"),
        },
        PolicyRefusal {
            case: "a stack that does not exist",
            members: vec![
                member("StackName", "no-such-stack"),
                member("StackPolicyBody", "{}"),
            ],
            status: 400,
            code: Some("ValidationError"),
        },
        PolicyRefusal {
            // The document is read from this emulator's own Amazon S3, so a URL addressing
            // an object no bucket holds is the `ValidationError` CloudFormation answers for
            // the reference it was given — the same answer CreateStack and UpdateStack give
            // that member (`stack_policy_by_reference.rs` is the suite that drives the
            // reference end to end).
            case: "a policy named by a URL that addresses no readable object",
            members: vec![
                member("StackName", "policed"),
                member("StackPolicyURL", "https://example.s3.amazonaws.com/policy.json"),
            ],
            status: 400,
            code: Some("ValidationError"),
        },
    ];
    for PolicyRefusal {
        case,
        members,
        status,
        code,
    } in refusals
    {
        let (status_code, body) = call(&router, "SetStackPolicy", &members).await;
        assert_eq!(
            status_code, status,
            "case {case:?}: expected {status}, got {status_code} {body}"
        );
        if let Some(code) = code {
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some(code),
                "case {case:?}: expected {code}, got {body}"
            );
        }
        let (_, body) = call(&router, "GetStackPolicy", &[member("StackName", "policed")]).await;
        assert_eq!(
            xml_text(&body, "StackPolicyBody").as_deref(),
            Some(in_force),
            "case {case:?}: a refused call left the policy in force alone, got {body}"
        );
    }

    // A stack policy governs updates to a LIVE stack, so a completed deletion is not a
    // stack a policy can be set on or read from — even by its stack id.
    let (_, created) = create_stack(&router, "policy-deleted", "{}", &[]).await;
    let deleted_id = xml_text(&created, "StackId").expect("a stack id");
    call(
        &router,
        "DeleteStack",
        &[member("StackName", "policy-deleted")],
    )
    .await;
    wait_for_the_transition().await;
    for action in ["SetStackPolicy", "GetStackPolicy"] {
        let mut members = vec![member("StackName", &deleted_id)];
        if action == "SetStackPolicy" {
            members.push(member("StackPolicyBody", "{}"));
        }
        let (status_code, body) = call(&router, action, &members).await;
        assert_eq!(
            status_code, 400,
            "case \"a completed deletion\": {action} is refused, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case \"a completed deletion\": expected ValidationError from {action}, got {body}"
        );
    }

    // A policy named by URL is READ from this emulator's own Amazon S3, so a URL addressing
    // an object no bucket holds is the `ValidationError` CloudFormation answers for that
    // reference — never a 200 that discards the policy. What a READABLE object does is the
    // whole of `stack_policy_by_reference.rs`.
    struct UrlCase {
        case: &'static str,
        action: &'static str,
        members: Vec<(String, String)>,
    }
    let url_cases = vec![
        UrlCase {
            case: "CreateStack with StackPolicyURL",
            action: "CreateStack",
            members: vec![
                member("StackName", "policy-by-url"),
                member("TemplateBody", "{}"),
                member("StackPolicyURL", "https://example.s3.amazonaws.com/policy.json"),
            ],
        },
        UrlCase {
            case: "UpdateStack with StackPolicyURL",
            action: "UpdateStack",
            members: vec![
                member("StackName", "unpoliced"),
                member("TemplateBody", r#"{"Description":"v3"}"#),
                member("StackPolicyURL", "https://example.s3.amazonaws.com/policy.json"),
            ],
        },
        UrlCase {
            case: "UpdateStack with StackPolicyDuringUpdateURL",
            action: "UpdateStack",
            members: vec![
                member("StackName", "unpoliced"),
                member("TemplateBody", r#"{"Description":"v4"}"#),
                member(
                    "StackPolicyDuringUpdateURL",
                    "https://example.s3.amazonaws.com/policy.json",
                ),
            ],
        },
    ];
    for UrlCase {
        case,
        action,
        members,
    } in url_cases
    {
        let (status_code, body) = call(&router, action, &members).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected the reference to be refused, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected a ValidationError for the unreadable object, got {body}"
        );
    }
}
