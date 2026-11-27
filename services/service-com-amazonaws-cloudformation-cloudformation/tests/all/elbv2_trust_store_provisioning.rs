// SPDX-License-Identifier: CC0-1.0

//! A stack's `AWS::ElasticLoadBalancingV2::TrustStore` EXISTS in Elastic Load Balancing — and
//! the listener that declares it is really associated with it.
//!
//! The rule under test: **a declared resource type this emulator implements is created in the
//! service that OWNS it, by that service's own create, and the type's `Ref` and `Fn::GetAtt`
//! carry what that create answered** — so the first read a client makes against the identifier
//! the stack published resolves there, a sibling declaration that REFERENCES the resource reaches
//! the resource that service really holds, an update applies the changed declaration through that
//! service's own modification, and the stack's delete takes the resource away again. For a trust
//! store those calls are `CreateTrustStore`, `ModifyTrustStore` and `DeleteTrustStore`, the reads
//! are `DescribeTrustStores` and `DescribeTrustStoreAssociations`, and the referencing sibling is
//! an HTTPS listener whose `MutualAuthentication.TrustStoreArn` is a `Ref` to it.
//!
//! That is what a client does with such a stack: it deploys the mutual-TLS plane in one template,
//! reads the stack's `Outputs` into a map and then asks Elastic Load Balancing what the trust
//! store it deployed is associated with. A `CREATE_COMPLETE` stack whose published trust store
//! ARN that service answers `TrustStoreNotFound` for is not a state AWS can be in.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the trust
//! store or leaves the name to CloudFormation, and how many CA certificates the bundle it is
//! created from holds — the number only the owning service can report, since it is a property of
//! the object's bytes — beside the count the rotation then leaves it with.
//!
//! The certificates are real self-signed CA certificates minted here (`rcgen`) and staged in the
//! emulator's own Amazon S3 with `PutObject`, so the count every assertion below is about is the
//! number of certificates a PARSER accepted. Certificate Manager is deliberately NOT mounted in
//! this target's router, so the listener's own TLS certificate establishes nothing either way —
//! what this suite is about is the trust store, and the ACM-resolved refusals of a listener's
//! certificates are pinned by that service's own suites.

use smithy_server_runtime::MultiServiceRouter;

use crate::elbv2_resource_provisioning::{output, outputs_of, NETWORK};
use crate::stack_lifecycle::{
    call, create_stack, elbv2, member, router, settled_stack, xml_text, ACCOUNT_ID, REGION,
};
use crate::stack_resource_provisioning::s3;
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The bucket every CA certificates bundle below is staged in.
const BUNDLE_BUCKET: &str = "cfn-elbv2-trust-store-bundles";

/// The key the bundle a row's create reads lives under, and the one its update rotates to.
const CREATED_BUNDLE: &str = "created/ca-bundle.pem";
const ROTATED_BUNDLE: &str = "rotated/ca-bundle.pem";

/// The TLS certificate the HTTPS listener presents. Certificate Manager is not mounted beside
/// this router, so the owning service answers nothing about it and nothing is established —
/// which is what lets the listener carry mutual authentication here at all.
const LISTENER_CERTIFICATE: &str =
    "arn:aws:acm:us-east-1:000000000000:certificate/12345678-1234-1234-1234-123456789012";

/// The status Elastic Load Balancing reports a trust store it has finished building in
/// (`TrustStoreStatus`, whose enum values are `ACTIVE` and `CREATING`), which is what the
/// resource's creation waited for and so what the stack publishes.
const TRUST_STORE_ACTIVE: &str = "ACTIVE";

/// The awsQuery error codes Elastic Load Balancing refuses a trust-store call with
/// (`smithy_query(query="operation",operation="DeleteTrustStore,DescribeTrustStores")`: each
/// error's own `awsQueryError` code).
const TRUST_STORE_NOT_FOUND: &str = "TrustStoreNotFound";
const TRUST_STORE_IN_USE: &str = "TrustStoreInUse";

/// How many characters the identifier Elastic Load Balancing assigns inside a trust store's ARN
/// carries: 16 lowercase hexadecimal, the form that service's own `mint_id` produces.
const ASSIGNED_ID_HEX_DIGITS: usize = 16;

/// One row: the stack it deploys, whether its template names the trust store, and how many CA
/// certificates the bundles its create and its update read hold.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    /// The `Name` the template states, or `None` for a row that leaves it to CloudFormation.
    declared_name: Option<&'static str>,
    /// The load balancer's own name, which every row states (it is only the handle the
    /// association is reported under).
    load_balancer_name: &'static str,
    /// How many certificates the staged bundle the create reads holds, and how many the one its
    /// update rotates to holds.
    created_certificates: usize,
    rotated_certificates: usize,
}

const ROWS: &[Row] = &[
    Row {
        case: "a trust store the template names, created from a one-certificate bundle and \
               rotated onto a three-certificate one",
        stack_name: "elbv2-named-trust-store",
        declared_name: Some("cfn-named-trust-store"),
        load_balancer_name: "named-ts-alb",
        created_certificates: 1,
        rotated_certificates: 3,
    },
    Row {
        case: "a trust store CloudFormation names, created from a three-certificate bundle and \
               rotated onto a two-certificate one",
        stack_name: "elbv2-generated-trust-store",
        declared_name: None,
        load_balancer_name: "generated-ts-alb",
        created_certificates: 3,
        rotated_certificates: 2,
    },
];

/// A bundle of `count` real self-signed CA certificates, PEM encoded one after another — what
/// `openssl` produces for a CA bundle and what a caller stages in Amazon S3.
///
/// Each certificate is GENERATED here, so every count asserted below is the number of
/// certificates a parser produced rather than a number this suite and the owning service both
/// guessed.
fn ca_bundle(count: usize) -> String {
    (0..count)
        .map(|index| {
            let key = rcgen::KeyPair::generate().expect("a key pair");
            let mut params = rcgen::CertificateParams::new(vec![format!("ca{index}.example.com")])
                .expect("certificate parameters");
            params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
            params
                .self_signed(&key)
                .expect("a self-signed certificate")
                .pem()
        })
        .collect()
}

/// Create [`BUNDLE_BUCKET`] and stage the two bundles one row reads, through Amazon S3's own
/// wire: the object a trust store is built from is one a caller really wrote.
async fn stage_bundles(router: &MultiServiceRouter, case: &str, row: &Row) {
    let created = s3(router, "PUT", &format!("/{BUNDLE_BUCKET}"), b"").await;
    assert!(
        (200..300).contains(&created.status),
        "case {case:?}: the bundle bucket must be created first, got {} {}",
        created.status,
        created.body
    );
    for (key, certificates) in [
        (CREATED_BUNDLE, row.created_certificates),
        (ROTATED_BUNDLE, row.rotated_certificates),
    ] {
        let staged = s3(
            router,
            "PUT",
            &format!("/{BUNDLE_BUCKET}/{key}"),
            ca_bundle(certificates).as_bytes(),
        )
        .await;
        assert!(
            (200..300).contains(&staged.status),
            "case {case:?}: the bundle {key} must be staged first, got {} {}",
            staged.status,
            staged.body
        );
    }
}

/// The template one row deploys at `bundle_key`: the network, an internal Application Load
/// Balancer, the trust store, and the HTTPS listener that authenticates client certificates
/// against it.
///
/// Built line by line, because YAML is indentation-sensitive and the declared name is a line
/// one row states and the other leaves out altogether.
fn template(row: &Row, bundle_key: &str) -> String {
    let mut yaml = String::from("Resources:\n");
    yaml.push_str(NETWORK);
    let mut push = |line: &str| {
        yaml.push_str(line);
        yaml.push('\n');
    };
    push("  Alb:");
    push("    Type: AWS::ElasticLoadBalancingV2::LoadBalancer");
    push("    Properties:");
    push(&format!("      Name: {}", row.load_balancer_name));
    push("      Type: application");
    push("      Scheme: internal");
    push("      Subnets:");
    push("        - !Ref SubnetA");
    push("        - !Ref SubnetB");
    push("  Store:");
    push("    Type: AWS::ElasticLoadBalancingV2::TrustStore");
    push("    Properties:");
    if let Some(name) = row.declared_name {
        push(&format!("      Name: {name}"));
    }
    push(&format!(
        "      CaCertificatesBundleS3Bucket: {BUNDLE_BUCKET}"
    ));
    push(&format!("      CaCertificatesBundleS3Key: {bundle_key}"));
    push("      Tags:");
    push("        - Key: plane");
    push("          Value: mutual-tls");
    push("  Listener:");
    push("    Type: AWS::ElasticLoadBalancingV2::Listener");
    push("    Properties:");
    push("      LoadBalancerArn: !Ref Alb");
    push("      Port: 443");
    push("      Protocol: HTTPS");
    push("      SslPolicy: ELBSecurityPolicy-2016-08");
    push("      Certificates:");
    push(&format!("        - CertificateArn: {LISTENER_CERTIFICATE}"));
    push("      MutualAuthentication:");
    push("        Mode: verify");
    push("        TrustStoreArn: !Ref Store");
    push("      DefaultActions:");
    push("        - Type: fixed-response");
    push("          FixedResponseConfig:");
    push("            StatusCode: \"404\"");
    push("            ContentType: text/plain");
    push("            MessageBody: no rule matched");
    push("Outputs:");
    push("  TrustStoreRef:");
    push("    Value: !Ref Store");
    push("  TrustStoreArn:");
    push("    Value: !GetAtt Store.TrustStoreArn");
    push("  TrustStoreStatus:");
    push("    Value: !GetAtt Store.Status");
    push("  CaCertificates:");
    push("    Value: !GetAtt Store.NumberOfCaCertificates");
    push("  LoadBalancerArn:");
    push("    Value: !Ref Alb");
    push("  ListenerArn:");
    push("    Value: !Ref Listener");
    drop(push);
    yaml
}

/// The `(name, assigned id)` of a trust store ARN, held to the form that service mints
/// (`arn:aws:elasticloadbalancing:<region>:<account>:truststore/<name>/<16 hex>`).
fn trust_store_parts(case: &str, arn: &str) -> (String, String) {
    let head = format!("arn:aws:elasticloadbalancing:{REGION}:{ACCOUNT_ID}:truststore/");
    let resource = arn.strip_prefix(&head).unwrap_or_else(|| {
        panic!("case {case:?}: expected a trust store ARN under {head:?}, got {arn:?}")
    });
    let (name, assigned) = resource.split_once('/').unwrap_or_else(|| {
        panic!("case {case:?}: expected <name>/<assigned id> after the ARN's type, got {arn:?}")
    });
    assert!(
        assigned.len() == ASSIGNED_ID_HEX_DIGITS
            && assigned
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "case {case:?}: expected the {ASSIGNED_ID_HEX_DIGITS}-hexadecimal identifier Elastic \
         Load Balancing assigns, got {assigned:?} in {arn:?}"
    );
    (name.to_string(), assigned.to_string())
}

/// What `DescribeTrustStores` reports for one ARN: its name, status and certificate count, or
/// the refusal code where that service holds no trust store under it.
async fn described_trust_store(
    router: &MultiServiceRouter,
    arn: &str,
) -> Result<(String, String, String), (u16, String)> {
    let (code, body) = elbv2(
        router,
        "DescribeTrustStores",
        &[member("TrustStoreArns.member.1", arn)],
    )
    .await;
    if code != 200 {
        return Err((code, xml_text(&body, "Code").unwrap_or_default()));
    }
    Ok((
        xml_text(&body, "Name").unwrap_or_default(),
        xml_text(&body, "Status").unwrap_or_default(),
        xml_text(&body, "NumberOfCaCertificates").unwrap_or_default(),
    ))
}

#[tokio::test]
async fn a_stacks_declared_trust_store_is_associated_with_its_listeners_load_balancer() {
    for row in ROWS {
        let case = row.case;
        let router = router();
        stage_bundles(&router, case, row).await;

        let (code, body) = create_stack(
            &router,
            row.stack_name,
            &template(row, CREATED_BUNDLE),
            &[],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: CreateStack must answer 200, got {code} {body}"
        );
        let described = settled_stack(&router, case, row.stack_name).await;
        if xml_text(&described, "StackStatus").as_deref() != Some(status::CREATE_COMPLETE) {
            let (_, events) = call(
                &router,
                "DescribeStackEvents",
                &[member("StackName", row.stack_name)],
            )
            .await;
            panic!(
                "case {case:?}: the stack must reach CREATE_COMPLETE, got {:?} ({:?}); its \
                 events: {events}",
                xml_text(&described, "StackStatus"),
                xml_text(&described, "StackStatusReason"),
            );
        }

        // 1. The identity the stack published is the ARN the create answered, and the attribute
        //    that carries the same ARN agrees with it.
        let values = outputs_of(case, &described);
        let published = output(case, &values, "TrustStoreRef");
        let (name, _) = trust_store_parts(case, &published);
        assert_eq!(
            output(case, &values, "TrustStoreArn"),
            published,
            "case {case:?}: expected the TrustStoreArn attribute to be the ARN the Ref resolves \
             to, got {values:?}"
        );
        match row.declared_name {
            Some(declared) => assert_eq!(
                name, declared,
                "case {case:?}: expected the trust store the template named, got {published:?}"
            ),
            // "CloudFormation generates a unique physical ID and uses that ID for the resource
            // name", in the `<stack>-<logical id>-<unique id>` form, held to the 32 characters
            // and the character set `TrustStoreName` admits (`@length(max: 32)`, pattern
            // `^([a-zA-Z0-9]+-)*[a-zA-Z0-9]+$`).
            None => assert!(
                name.contains("-Store-")
                    && name.len() <= 32
                    && !name.starts_with('-')
                    && !name.ends_with('-')
                    && !name.contains("--")
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "case {case:?}: expected CloudFormation's generated name for the resource, got \
                 {name:?}"
            ),
        }

        // 2. The read the issue filed: the ARN the stack published resolves in the service that
        //    owns the trust store, which reports it ready and carrying the certificates it
        //    counted in the staged bundle.
        let reported = described_trust_store(&router, &published).await;
        assert_eq!(
            reported,
            Ok((
                name.clone(),
                TRUST_STORE_ACTIVE.to_string(),
                row.created_certificates.to_string()
            )),
            "case {case:?}: expected Elastic Load Balancing to hold {published:?} as \
             {name:?}/{TRUST_STORE_ACTIVE} with {} certificates",
            row.created_certificates
        );
        // …and the attributes the stack publishes are the values that service reports.
        assert_eq!(
            (
                output(case, &values, "TrustStoreStatus"),
                output(case, &values, "CaCertificates")
            ),
            (
                TRUST_STORE_ACTIVE.to_string(),
                row.created_certificates.to_string()
            ),
            "case {case:?}: expected the published Status and NumberOfCaCertificates to be the \
             ones Elastic Load Balancing reports, got {values:?}"
        );

        // 3. The listener the same template declares REACHES that trust store: the resource the
        //    trust store is associated with is the load balancer whose listener authenticates
        //    client certificates against it.
        let load_balancer_arn = output(case, &values, "LoadBalancerArn");
        let (code, associations) = elbv2(
            &router,
            "DescribeTrustStoreAssociations",
            &[member("TrustStoreArn", &published)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected DescribeTrustStoreAssociations on {published:?} to answer \
             200, got {code} {associations}"
        );
        assert_eq!(
            xml_text(&associations, "ResourceArn").as_deref(),
            Some(load_balancer_arn.as_str()),
            "case {case:?}: expected the trust store to be associated with the load balancer the \
             stack published, got {associations}"
        );

        // …so the owning service refuses a delete of it while that association stands, which is
        // what makes the association real rather than reported.
        let (code, refused) = elbv2(
            &router,
            "DeleteTrustStore",
            &[member("TrustStoreArn", &published)],
        )
        .await;
        assert_eq!(
            (code, xml_text(&refused, "Code").unwrap_or_default()),
            (400, TRUST_STORE_IN_USE.to_string()),
            "case {case:?}: expected an associated trust store's delete to be refused \
             {TRUST_STORE_IN_USE}, got {code} {refused}"
        );

        // 4. An update that re-points the declaration at another bundle is applied through the
        //    owning service's own modification: the trust store keeps its identity and reports
        //    the certificates of the bundle it now reads.
        let (code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", row.stack_name),
                member("TemplateBody", &template(row, ROTATED_BUNDLE)),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: UpdateStack must answer 200, got {code} {body}"
        );
        let described = settled_stack(&router, case, row.stack_name).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::UPDATE_COMPLETE),
            "case {case:?}: the rotation must complete, got {:?} ({:?})",
            xml_text(&described, "StackStatus"),
            xml_text(&described, "StackStatusReason"),
        );
        let values = outputs_of(case, &described);
        assert_eq!(
            output(case, &values, "TrustStoreRef"),
            published,
            "case {case:?}: expected the rotation to leave the trust store's identity where it \
             was, got {values:?}"
        );
        let reported = described_trust_store(&router, &published).await;
        assert_eq!(
            reported,
            Ok((
                name.clone(),
                TRUST_STORE_ACTIVE.to_string(),
                row.rotated_certificates.to_string()
            )),
            "case {case:?}: expected the rotated trust store to report the certificates of the \
             bundle the update named ({})",
            row.rotated_certificates
        );
        assert_eq!(
            output(case, &values, "CaCertificates"),
            row.rotated_certificates.to_string(),
            "case {case:?}: expected the published NumberOfCaCertificates to follow the \
             rotation, got {values:?}"
        );

        // 5. The teardown takes the trust store away again: a resource the stack created and no
        //    longer records must not be left behind in the service that holds it.
        let (code, body) = call(
            &router,
            "DeleteStack",
            &[member("StackName", row.stack_name)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: DeleteStack must answer 200, got {code} {body}"
        );
        let described = settled_stack(&router, case, row.stack_name).await;
        assert!(
            described.is_empty()
                || xml_text(&described, "StackStatus").as_deref() == Some(status::DELETE_COMPLETE),
            "case {case:?}: the teardown must complete, got {described}"
        );
        let reported = described_trust_store(&router, &published).await;
        assert_eq!(
            reported,
            Err((400, TRUST_STORE_NOT_FOUND.to_string())),
            "case {case:?}: expected the torn-down trust store to be gone from Elastic Load \
             Balancing, got {reported:?}"
        );
    }
}
