// SPDX-License-Identifier: CC0-1.0

//! A load balancer, its listeners and their certificate lists DECLARED in a CloudFormation
//! template, read back through the API of the service that owns them — the standard
//! infrastructure-as-code flow, across the four services it spans: CloudFormation deploys the
//! stack, Amazon EC2 owns the network it is placed in, Certificate Manager owns the certificates
//! and Elastic Load Balancing v2 owns the load balancer, its listeners and their certificate
//! lists. One router over one engine, so what is asserted is what a client holding the four SDKs
//! sees.
//!
//! RULE: a resource a stack declares that belongs to another service is CREATED in that
//! service's own state, and the identifier the stack publishes for it (its `Ref`, and so its
//! outputs) is the identifier that service resolves — so from the moment the stack reports the
//! resource created, every read of the owning service answers for it by that identifier, both
//! the reads that address the resource itself and the reads that address what hangs off it; once
//! the stack's delete has taken the resource away, the same read is refused with that service's
//! own not-found refusal for the identifier the request names. A stack that reports
//! `CREATE_COMPLETE` while the owning service knows nothing of its resource is not a state the
//! service pair can be in.
//!
//! For this estate the reads are Elastic Load Balancing's own: `DescribeLoadBalancers` by the ARN
//! the stack published, `DescribeListeners` selected by that `LoadBalancerArn` — "describes the
//! specified listeners or the listeners for the specified Application Load Balancer, Network Load
//! Balancer, or Gateway Load Balancer" — and `DescribeListenerCertificates`, which reports "the
//! default certificate and the certificate list for the specified HTTPS or TLS listener". The
//! refusal the rule ends on is that service's modelled `LoadBalancerNotFoundException`
//! (`awsQueryError` `LoadBalancerNotFound`, HTTP 400).
//!
//! Each test is one row of the table, split so the rows run beside each other, and they differ
//! along what the rule generalises over: the load balancer's FAMILY (application, network) and
//! SCHEME (internal, internet-facing), HOW MANY listeners hang off one load balancer (one, and
//! two — so the listing selected by the load balancer's ARN has more than one page to answer),
//! each listener's TLS-terminating protocol, port and security policy, what its default action
//! DOES (answer a fixed response, forward to a declared target group), and how many certificates
//! a declared `AWS::ElasticLoadBalancingV2::ListenerCertificate` attaches beside the listener's
//! own default one (none, one, two).
//!
//! No expected value is a literal from a recording: every identifier is read back out of the
//! service that minted it — Certificate Manager for the certificates, CloudFormation for what
//! the stack published, Elastic Load Balancing for what it holds — and the not-found sentence is
//! read from the owning service's own wording rather than spelled again here.
//!
//! Every certificate a listener is given is carried to `ISSUED` first, the only status a listener
//! can present a certificate in.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use bytes::Bytes;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::load_balancer_not_found_message;
use smithy_server_runtime::MultiServiceRouter;

// The ONE writer of Elastic Load Balancing's awsQuery wire and of the certificate that reaches
// `ISSUED` this test crate keeps (`all/elbv2_certificate_in_use_by_acm.rs`), so no second suite
// spells either again. Both take the router they are driven against, which is this suite's own.
use crate::elbv2_certificate_in_use_by_acm::{elbv2, elbv2_ok, issued_certificate};
// The ONE percent-encoder and element readers this test crate keeps for query suites
// (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{block_of, elements, form_encode, members_of, opt_element, outputs_of};

/// The Region every request below is addressed to and signed for. The same Region the shared
/// Elastic Load Balancing and Certificate Manager wire helpers use, so the ARNs this estate mints
/// are ARNs those calls can address.
const REGION: &str = "us-west-2";

/// The API version an awsQuery request naming the CloudFormation model carries.
const CFN_VERSION: &str = "2010-05-15";

/// The stack statuses the legs below wait for.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const DELETE_COMPLETE: &str = "DELETE_COMPLETE";

/// The `LoadBalancerState$Code` Elastic Load Balancing reports a load balancer that finished
/// coming up in: "the load balancer is ready".
const ACTIVE: &str = "active";

/// The `awsQueryError` code Elastic Load Balancing refuses an ARN naming a load balancer that
/// does not exist with.
const LOAD_BALANCER_NOT_FOUND: &str = "LoadBalancerNotFound";

/// The logical id of the load balancer every row declares, and of the target group a row that
/// forwards declares.
const ALB: &str = "Alb";
const TARGETS: &str = "Targets";

/// The output key the load balancer's `Ref` is published under; a listener's is its own logical
/// id with this suffix.
const ALB_REF: &str = "AlbRef";
const REF_SUFFIX: &str = "Ref";

/// The `AWS::ElasticLoadBalancingV2::LoadBalancer` families a row declares (`Type` property).
const APPLICATION: &str = "application";
const NETWORK: &str = "network";

/// The segment the ARN of a load balancer of each family carries.
const APPLICATION_SEGMENT: &str = "/app/";
const NETWORK_SEGMENT: &str = "/net/";

/// How many listeners one page of the listing selected by a load balancer's ARN answers, so a
/// row declaring more than one listener is read over several pages — the way an SDK paginator
/// reads it.
const PAGE_SIZE: &str = "1";

/// How long a wait gives a state the clock owes (a stack operation's outcome, a load balancer
/// coming up, a deleted resource going away) and how often it looks again. Generous, and only
/// here so something nothing will ever satisfy is a readable failure rather than a hang: every
/// wait below waits for its CONDITION, never for a span of time.
const DEADLINE: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(50);

/// The whole emulator over one engine, exactly as the binary assembles it — which is what makes
/// the hand-off real: CloudFormation's provisioning calls are served by Amazon EC2, Elastic Load
/// Balancing and Certificate Manager themselves.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification is the front door's concern, not this suite's.
    router.set_request_authentication(false);
    router
}

/// One CloudFormation operation over its awsQuery wire, answered with its status and XML body.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={CFN_VERSION}");
    for (name, value) in members {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/cloudformation/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One CloudFormation operation that must succeed.
async fn cfn(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = cloudformation(router, action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CloudFormation {action} must answer 200, got {status} with body {body}"
    );
    body
}

/// Wait until the stack reports `wanted`, and answer the body of the read that saw it — or of the
/// last read either way, so the caller's assertion names the case and the actual status.
async fn wait_for_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    wanted: &str,
) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (status, described) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DescribeStacks while waiting for {wanted}, got \
             {status} with {described}"
        );
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        if reported == wanted || Instant::now() >= deadline {
            return described;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Wait until the stack is GONE: CloudFormation reports it `DELETE_COMPLETE`, or no longer
/// resolves the name at all — a deleted stack is readable by name only for as long as that
/// service keeps it, so either answer means the delete is done.
async fn wait_for_deleted(router: &MultiServiceRouter, case: &str, stack: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (status, described) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        let gone = status != 200
            || opt_element(&described, "StackStatus").as_deref() == Some(DELETE_COMPLETE);
        if gone {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the stack's delete to finish, last seen {described}"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// What one listener the listing reported carries, as a client reads it.
#[derive(Debug, PartialEq, Eq)]
struct Reported {
    load_balancer_arn: String,
    port: String,
    protocol: String,
    ssl_policy: String,
}

/// Every listener `DescribeListeners` reports for the load balancer `arn` names, in the order the
/// pages answer them, read over EVERY page of a listing asked for ONE listener at a time — the
/// way an SDK paginator reads the listing selected by a load balancer's ARN.
///
/// Each page carries at most one listener, so the identifier is read off the page itself.
async fn listener_arns_of(router: &MultiServiceRouter, case: &str, arn: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut members: Vec<(&str, &str)> =
            vec![("LoadBalancerArn", arn), ("PageSize", PAGE_SIZE)];
        if let Some(marker) = marker.as_deref() {
            members.push(("Marker", marker));
        }
        let body = elbv2_ok(router, case, "DescribeListeners", &members).await;
        if let Some(listener) = opt_element(&body, "ListenerArn") {
            found.push(listener);
        }
        marker = opt_element(&body, "NextMarker").filter(|next| !next.is_empty());
        if marker.is_none() {
            return found;
        }
    }
}

/// What `DescribeListeners` reports for ONE listener, selected by its own ARN — a body carrying
/// that listener alone, so each member is read off it without scoping.
async fn described_listener(router: &MultiServiceRouter, case: &str, arn: &str) -> Reported {
    let body = elbv2_ok(
        router,
        case,
        "DescribeListeners",
        &[("ListenerArns.member.1", arn)],
    )
    .await;
    assert_eq!(
        opt_element(&body, "ListenerArn").as_deref(),
        Some(arn),
        "case {case:?}: expected the read selected by {arn} to answer that listener, got {body}"
    );
    Reported {
        load_balancer_arn: opt_element(&body, "LoadBalancerArn").unwrap_or_default(),
        port: opt_element(&body, "Port").unwrap_or_default(),
        protocol: opt_element(&body, "Protocol").unwrap_or_default(),
        ssl_policy: opt_element(&body, "SslPolicy").unwrap_or_default(),
    }
}

/// What `DescribeListenerCertificates` reports for one listener, over every page: each
/// certificate paired with its own `IsDefault`.
async fn certificates_of(
    router: &MultiServiceRouter,
    case: &str,
    listener: &str,
) -> BTreeMap<String, bool> {
    let mut reported = BTreeMap::new();
    let mut marker: Option<String> = None;
    loop {
        let mut members: Vec<(&str, &str)> = vec![("ListenerArn", listener)];
        if let Some(marker) = marker.as_deref() {
            members.push(("Marker", marker));
        }
        let body = elbv2_ok(router, case, "DescribeListenerCertificates", &members).await;
        let page = block_of(&body, "Certificates").unwrap_or_default();
        for entry in members_of(page) {
            let Some(arn) = opt_element(entry, "CertificateArn") else {
                continue;
            };
            let is_default = opt_element(entry, "IsDefault").as_deref() == Some("true");
            reported.insert(arn, is_default);
        }
        marker = opt_element(&body, "NextMarker").filter(|next| !next.is_empty());
        if marker.is_none() {
            return reported;
        }
    }
}

/// One listener a row declares on its load balancer.
struct Declared {
    /// The logical id the listener carries, and the stem of the output its `Ref` is published
    /// under.
    logical_id: &'static str,
    /// A TLS-terminating protocol a certificate is bracketed to, and the port it listens on.
    protocol: &'static str,
    port: &'static str,
    /// A security policy `DescribeSSLPolicies` publishes.
    ssl_policy: &'static str,
    /// How many certificates a declared `ListenerCertificate` attaches beside the listener's own
    /// default one.
    attached: usize,
}

/// One row: the load balancer a stack declares, and the listeners that hang off it.
struct Row {
    case: &'static str,
    /// The stack's name, and the `10.<x>` prefix of the network it declares, so rows running
    /// beside each other do not collide in the engine they are each given.
    stack: &'static str,
    prefix: &'static str,
    /// The load balancer's family and scheme.
    family: &'static str,
    scheme: &'static str,
    listeners: &'static [Declared],
}

/// The ARN segment a load balancer of this row's family carries, which is read off the row rather
/// than off the value under test.
fn family_segment(row: &Row) -> &'static str {
    if row.family == NETWORK {
        NETWORK_SEGMENT
    } else {
        APPLICATION_SEGMENT
    }
}

/// The whole template of one row: the network, the load balancer, every listener, the certificate
/// attachments and the `Ref` outputs the rule is read through.
///
/// `attached` holds, per listener, the certificate ARNs its declared attachment names — values
/// Certificate Manager minted, interpolated into the declaration the way a client writes an ARN
/// it holds into a template.
fn template(row: &Row, attached: &[Vec<String>]) -> String {
    let prefix = row.prefix;
    let mut body = String::new();
    // The listener's own default certificate is passed as a stack PARAMETER, which is how a
    // client that holds an ARN hands it to a template.
    body.push_str("Parameters:\n  DefaultCertificateArn:\n    Type: String\n");
    body.push_str("Resources:\n");
    body.push_str(&format!(
        "  Vpc:\n    Type: AWS::EC2::VPC\n    Properties:\n      CidrBlock: {prefix}.0.0/16\n"
    ));
    body.push_str("  Igw:\n    Type: AWS::EC2::InternetGateway\n");
    body.push_str(
        "  IgwAttach:\n    Type: AWS::EC2::VPCGatewayAttachment\n    Properties:\n      \
         VpcId: !Ref Vpc\n      InternetGatewayId: !Ref Igw\n",
    );
    for (ordinal, zone) in ["a", "b"].into_iter().enumerate() {
        body.push_str(&format!(
            "  Subnet{upper}:\n    Type: AWS::EC2::Subnet\n    Properties:\n      \
             VpcId: !Ref Vpc\n      CidrBlock: {prefix}.{octet}.0/24\n      \
             AvailabilityZone: {REGION}{zone}\n",
            upper = zone.to_uppercase(),
            octet = ordinal + 1,
        ));
    }
    body.push_str(
        "  Sg:\n    Type: AWS::EC2::SecurityGroup\n    Properties:\n      \
         GroupDescription: the group the declared load balancer is placed in\n      \
         VpcId: !Ref Vpc\n",
    );
    body.push_str(&format!(
        "  {ALB}:\n    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n    Properties:\n      \
         Type: {family}\n      Scheme: {scheme}\n      Subnets:\n        - !Ref SubnetA\n        \
         - !Ref SubnetB\n",
        family = row.family,
        scheme = row.scheme,
    ));
    if row.family == APPLICATION {
        body.push_str("      SecurityGroups:\n        - !Ref Sg\n");
    }
    // A row whose listeners forward needs the group they forward to; one that answers a fixed
    // response needs nothing behind it.
    let forwards = row.family == NETWORK;
    if forwards {
        body.push_str(&format!(
            "  {TARGETS}:\n    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n    \
             Properties:\n      Port: 443\n      Protocol: TCP\n      TargetType: ip\n      \
             VpcId: !Ref Vpc\n"
        ));
    }
    for (listener, certificates) in row.listeners.iter().zip(attached) {
        let id = listener.logical_id;
        body.push_str(&format!(
            "  {id}:\n    Type: AWS::ElasticLoadBalancingV2::Listener\n    Properties:\n      \
             LoadBalancerArn: !Ref {ALB}\n      Port: {port}\n      Protocol: {protocol}\n      \
             SslPolicy: {policy}\n      Certificates:\n        - CertificateArn: !Ref \
             DefaultCertificateArn\n",
            port = listener.port,
            protocol = listener.protocol,
            policy = listener.ssl_policy,
        ));
        if forwards {
            body.push_str(&format!(
                "      DefaultActions:\n        - Type: forward\n          \
                 TargetGroupArn: !Ref {TARGETS}\n"
            ));
        } else {
            body.push_str(
                "      DefaultActions:\n        - Type: fixed-response\n          \
                 FixedResponseConfig:\n            StatusCode: \"200\"\n            \
                 ContentType: text/plain\n            MessageBody: served by the stack\n",
            );
        }
        if !certificates.is_empty() {
            body.push_str(&format!(
                "  {id}Certs:\n    Type: AWS::ElasticLoadBalancingV2::ListenerCertificate\n    \
                 Properties:\n      ListenerArn: !Ref {id}\n      Certificates:\n"
            ));
            for arn in certificates {
                body.push_str(&format!("        - CertificateArn: {arn}\n"));
            }
        }
    }
    body.push_str(&format!("Outputs:\n  {ALB_REF}:\n    Value: !Ref {ALB}\n"));
    for listener in row.listeners {
        let id = listener.logical_id;
        body.push_str(&format!("  {id}{REF_SUFFIX}:\n    Value: !Ref {id}\n"));
    }
    body
}

/// Deploy one row, and hold the owning service to the rule: its own reads answer for the
/// identifiers the stack published, and the same reads are refused once the stack's delete has
/// taken the resources away.
async fn deploy_and_read_back(row: &Row) {
    let case = row.case;
    let router = router();

    // The certificates the estate presents, every one carried to `ISSUED` by the service that
    // owns them: the listeners' shared default one, and the ones each declared attachment names.
    let default_certificate = issued_certificate(&router, case, "default.example.com").await;
    let mut attached: Vec<Vec<String>> = Vec::new();
    for (index, listener) in row.listeners.iter().enumerate() {
        let mut certificates = Vec::new();
        for ordinal in 0..listener.attached {
            let domain = format!("sni-{index}-{ordinal}.example.com");
            certificates.push(issued_certificate(&router, case, &domain).await);
        }
        attached.push(certificates);
    }

    cfn(
        &router,
        case,
        "CreateStack",
        &[
            ("StackName", row.stack),
            ("TemplateBody", &template(row, &attached)),
            ("Parameters.member.1.ParameterKey", "DefaultCertificateArn"),
            ("Parameters.member.1.ParameterValue", &default_certificate),
        ],
    )
    .await;
    let described = wait_for_stack(&router, case, row.stack, CREATE_COMPLETE).await;
    let reported = opt_element(&described, "StackStatus").unwrap_or_default();
    assert_eq!(
        reported, CREATE_COMPLETE,
        "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {reported:?} from \
         {described}"
    );

    // What the stack PUBLISHED: the load balancer's `Ref` and each listener's.
    let outputs = outputs_of(&described);
    let load_balancer = outputs
        .get(ALB_REF)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no {ALB_REF} output in {described}"));
    let listener_refs: Vec<String> = row
        .listeners
        .iter()
        .map(|listener| {
            let key = format!("{}{REF_SUFFIX}", listener.logical_id);
            outputs
                .get(&key)
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {described}"))
        })
        .collect();

    // 1. The `Ref` of the load balancer is the ARN the OWNING service resolves, and that
    //    service's own read answers for it with the configuration the template declared.
    let segment = family_segment(row);
    assert!(
        load_balancer.starts_with(&format!("arn:aws:elasticloadbalancing:{REGION}:"))
            && load_balancer.contains(segment),
        "case {case:?}: expected the published {ALB_REF} to be an Elastic Load Balancing ARN in \
         {REGION} carrying {segment:?}, got {load_balancer:?}"
    );
    let deadline = Instant::now() + DEADLINE;
    let described_balancer = loop {
        let body = elbv2_ok(
            &router,
            case,
            "DescribeLoadBalancers",
            &[("LoadBalancerArns.member.1", &load_balancer)],
        )
        .await;
        // The one state this estate genuinely waits for: a load balancer answers its create
        // `provisioning` and reaches `active` on the clock.
        if opt_element(&body, "Code").as_deref() == Some(ACTIVE) || Instant::now() >= deadline {
            break body;
        }
        tokio::time::sleep(POLL).await;
    };
    assert_eq!(
        opt_element(&described_balancer, "LoadBalancerArn").as_deref(),
        Some(load_balancer.as_str()),
        "case {case:?}: expected Elastic Load Balancing to hold the load balancer the stack \
         published, got {described_balancer}"
    );
    for (member, declared) in [("Type", row.family), ("Scheme", row.scheme), ("Code", ACTIVE)] {
        let actual = opt_element(&described_balancer, member).unwrap_or_default();
        assert_eq!(
            actual, declared,
            "case {case:?}: expected the held load balancer's {member} to be {declared:?}, got \
             {actual:?} in {described_balancer}"
        );
    }

    // 2. The listing SELECTED BY that ARN answers exactly the listeners the stack declared, each
    //    one as the declaration states it, and each one on the published load balancer.
    let listed = listener_arns_of(&router, case, &load_balancer).await;
    let found: BTreeSet<&str> = listed.iter().map(String::as_str).collect();
    let published: BTreeSet<&str> = listener_refs.iter().map(String::as_str).collect();
    assert_eq!(
        found, published,
        "case {case:?}: expected the listeners selected by the published load balancer's ARN to \
         be exactly the ones the stack published, got {found:?}"
    );
    assert_eq!(
        listed.len(),
        row.listeners.len(),
        "case {case:?}: expected the paged listing to answer each declared listener once, got \
         {listed:?}"
    );
    for (listener, arn) in row.listeners.iter().zip(&listener_refs) {
        let actual = described_listener(&router, case, arn).await;
        let expected = Reported {
            load_balancer_arn: load_balancer.clone(),
            port: listener.port.to_string(),
            protocol: listener.protocol.to_string(),
            ssl_policy: listener.ssl_policy.to_string(),
        };
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the listener {arn} to be reported as {expected:?}, got \
             {actual:?}"
        );
    }

    // 3. Each listener's certificate list is its own: the default certificate the listener
    //    declared, and every certificate the declared attachment named.
    for (listener, arn) in row.listeners.iter().zip(&listener_refs) {
        let mut expected = BTreeMap::new();
        expected.insert(default_certificate.clone(), true);
        let index = row
            .listeners
            .iter()
            .position(|other| other.logical_id == listener.logical_id)
            .expect("the listener is one of the row's");
        for certificate in &attached[index] {
            expected.insert(certificate.clone(), false);
        }
        let deadline = Instant::now() + DEADLINE;
        let actual = loop {
            let actual = certificates_of(&router, case, arn).await;
            if actual == expected || Instant::now() >= deadline {
                break actual;
            }
            tokio::time::sleep(POLL).await;
        };
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the listener {arn} to present the default certificate and \
             the attached ones, got {actual:?}"
        );
    }

    // 4. Once the stack's delete has taken the load balancer away, the SAME read is refused with
    //    the owning service's own not-found refusal for the ARN the request names.
    cfn(&router, case, "DeleteStack", &[("StackName", row.stack)]).await;
    wait_for_deleted(&router, case, row.stack).await;
    let deadline = Instant::now() + DEADLINE;
    let (status, refusal) = loop {
        let answered = elbv2(
            &router,
            "DescribeListeners",
            &[("LoadBalancerArn", &load_balancer)],
        )
        .await;
        if answered.0 != 200 || Instant::now() >= deadline {
            break answered;
        }
        tokio::time::sleep(POLL).await;
    };
    assert_eq!(
        status, 400,
        "case {case:?}: expected the read of a deleted load balancer to be refused 400, got \
         {status} with {refusal}"
    );
    assert_eq!(
        opt_element(&refusal, "Code").as_deref(),
        Some(LOAD_BALANCER_NOT_FOUND),
        "case {case:?}: expected the refusal to carry {LOAD_BALANCER_NOT_FOUND}, got {refusal}"
    );
    let sentence = load_balancer_not_found_message(&load_balancer);
    assert_eq!(
        elements(&refusal, "Message"),
        vec![sentence.clone()],
        "case {case:?}: expected the refusal to read {sentence:?}, got {refusal}"
    );
}

/// RULE: the load balancer and the HTTPS listener a stack declares are created in Elastic Load
/// Balancing's own state, and the ARNs the stack publishes resolve there — the estate the
/// infrastructure-as-code flow deploys and then drives through that service's API.
#[tokio::test]
async fn an_internal_applications_declared_listener_resolves_by_the_published_arn() {
    deploy_and_read_back(&Row {
        case: "an internal application load balancer with one HTTPS listener and one attached \
               certificate",
        stack: "declared-internal-alb",
        prefix: "10.11",
        family: APPLICATION,
        scheme: "internal",
        listeners: &[Declared {
            logical_id: "Listener",
            protocol: "HTTPS",
            port: "443",
            // A policy `DescribeSSLPolicies` publishes.
            ssl_policy: "ELBSecurityPolicy-2016-08",
            attached: 1,
        }],
    })
    .await;
}

/// The same rule where ONE load balancer carries SEVERAL listeners on different ports and
/// policies, and only one of them has certificates attached: the listing selected by the load
/// balancer's ARN answers both over its pages, and each certificate list is the listener's own.
#[tokio::test]
async fn an_internet_facing_applications_two_declared_listeners_both_resolve() {
    deploy_and_read_back(&Row {
        case: "an internet-facing application load balancer with two HTTPS listeners",
        stack: "declared-public-alb",
        prefix: "10.12",
        family: APPLICATION,
        scheme: "internet-facing",
        listeners: &[
            Declared {
                logical_id: "Listener",
                protocol: "HTTPS",
                port: "443",
                ssl_policy: "ELBSecurityPolicy-TLS13-1-2-2021-06",
                attached: 2,
            },
            Declared {
                logical_id: "AltListener",
                protocol: "HTTPS",
                port: "8443",
                ssl_policy: "ELBSecurityPolicy-2016-08",
                attached: 0,
            },
        ],
    })
    .await;
}

/// The same rule for the other family, whose TLS listener forwards to a target group the stack
/// declares beside it rather than answering a fixed response.
#[tokio::test]
async fn a_networks_declared_tls_listener_resolves_by_the_published_arn() {
    deploy_and_read_back(&Row {
        case: "an internal network load balancer with one TLS listener forwarding to a declared \
               target group",
        stack: "declared-internal-nlb",
        prefix: "10.13",
        family: NETWORK,
        scheme: "internal",
        listeners: &[Declared {
            logical_id: "Listener",
            protocol: "TLS",
            port: "443",
            ssl_policy: "ELBSecurityPolicy-TLS13-1-2-2021-06",
            attached: 1,
        }],
    })
    .await;
}
