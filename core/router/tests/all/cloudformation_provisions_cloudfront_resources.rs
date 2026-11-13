// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::CloudFront::Distribution` and an
//! `AWS::CloudFront::OriginAccessControl` really creates them in Amazon CloudFront, and the
//! ids the stack hands back are the ones CloudFront answers to.
//!
//! The rule under test: **a stack's physical resource id is the identifier the service that
//! owns the resource assigned — for a type whose identifier that service MINTS, the one its
//! create answered with — so `Ref` and `Fn::GetAtt` resolve to a value the owning service's
//! own read resolves to the resource the template configured.** Two crates have to agree for
//! that to hold (CloudFormation makes the calls; CloudFront serves them, mints the ids and
//! holds the configuration), so the suite lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: the distribution's connection mode
//! (a multi-tenant one, which serves no domain name of its own, and a standard one, which
//! does), whether the configuration carries the list-valued members CloudFront's model wraps
//! in `{Quantity, Items}` containers (aliases, cache behaviours, custom error responses, the
//! geo restriction's locations, an origin's custom headers) or none of them, whether the
//! template is written in JSON or YAML, whether it carries `Tags`, and which intrinsic each
//! output reads the resource with (`Ref`, `Fn::GetAtt … Id`, `Fn::GetAtt … DomainName`).
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the two ids and the domain name, which are CloudFront's
//! to mint: what is checked about an id is that ONE value appears everywhere the stack
//! reports the resource's identity, that it has the form CloudFront mints, and that
//! CloudFront's own `GetDistribution` / `GetOriginAccessControl` resolve it to the resource
//! the template configured.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{element, elements, form_encode, opt_element, outputs_of, physical_ids_of};

/// How long a stack operation is given to reach its terminal status. GENEROUS: what is waited
/// for is the CONDITION (the status the stack reports), polled until it holds, so the only
/// thing this bound decides is when a stack that never settles is reported as a failure.
const SETTLES_WITHIN: Duration = Duration::from_secs(60);

/// How often the stack's status is read while waiting for it to settle.
const POLL_EVERY: Duration = Duration::from_millis(50);

/// The Region every row deploys in. CloudFront is a global service, and the Region is what
/// the credential scope of each call names.
const REGION: &str = "us-east-1";

/// The logical ids the rows declare their resources under.
const BUCKET_LOGICAL_ID: &str = "OriginBucket";
const OAC_LOGICAL_ID: &str = "TenantOAC";
const DISTRIBUTION_LOGICAL_ID: &str = "CdnDistribution";

/// The one value the UPDATE of each row changes: the distribution's `Comment` and the
/// control's `Description`, so a changed declaration of both reaches CloudFront through that
/// service's own update rather than replacing either resource.
const UPDATED_COMMENT: &str = "the estate after its update";

/// How many characters follow the `E` of a CloudFront-assigned identifier: a distribution id
/// and an origin access control id are both an `E` followed by 13 uppercase alphanumerics,
/// the form `CloudFrontOriginAccessIdentity$Id` documents by example ("E74FTE3AJFJ256A").
const CLOUDFRONT_ID_BODY: usize = 13;

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way (the
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
        service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in [`REGION`], which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
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
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one CloudFront operation over its restXml wire.
async fn cloudfront(router: &MultiServiceRouter, method: &str, path: &str) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("cloudfront.{REGION}.amazonaws.com"))
        .header("authorization", authorization("cloudfront"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Read the stack's status until it is one `DescribeStacks` reports as terminal, or until
/// [`SETTLES_WITHIN`] has passed.
///
/// The CONDITION is what is waited for — a status that is not still in progress — rather than
/// a fixed span, so the suite answers as soon as the stack has settled however fast or slow
/// the host is.
async fn settled_stack(router: &MultiServiceRouter, stack_name: &str, case: &str) -> String {
    let deadline = Instant::now() + SETTLES_WITHIN;
    loop {
        let (status, described) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        if !reported.ends_with("_IN_PROGRESS") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the stack was still {reported:?} when the wait ran out"
        );
        tokio::time::sleep(POLL_EVERY).await;
    }
}

/// Read the stack until `DescribeStacks` no longer reports it at all, or until
/// [`SETTLES_WITHIN`] has passed — which is how a client watches a deletion through.
async fn deleted_stack(router: &MultiServiceRouter, stack_name: &str, case: &str) {
    let deadline = Instant::now() + SETTLES_WITHIN;
    loop {
        let (_, described) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        if reported == "DELETE_COMPLETE" || reported.is_empty() {
            return;
        }
        assert_ne!(
            reported, "DELETE_FAILED",
            "case {case:?}: expected the stack's resources to be deleted, got {described}"
        );
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the stack was still {reported:?} when the wait ran out"
        );
        tokio::time::sleep(POLL_EVERY).await;
    }
}

/// One row of the table: a template, and what the rule says the distribution it declares must
/// then be in CloudFront.
struct Row {
    case: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML), with the one value
    /// the UPDATE below changes — the distribution's `Comment` and the control's
    /// `Description` — passed in, so the create and the update of a row are the SAME
    /// declaration but for that value.
    template: fn(&str, &str) -> String,
    /// `ConnectionMode` the stored configuration must report, or `None` for a configuration
    /// that states none — the member is not one the service supplies a value for.
    expected_connection_mode: Option<&'static str>,
    /// `Comment` the stored configuration must report.
    expected_comment: &'static str,
    /// The `DomainName` of the origin the configuration must report, built from the bucket
    /// the same stack creates.
    expected_origin_domain: fn(&str) -> String,
    /// The `Aliases` the stored configuration must report, in order.
    expected_aliases: &'static [&'static str],
    /// The `PathPattern` of each ordered cache behaviour the configuration must report.
    expected_cache_behaviors: &'static [&'static str],
    /// Whether the row's origin reaches its bucket with the origin access control the same
    /// stack declares, so the id the origin carries must be the one CloudFront minted for it.
    origin_names_the_control: bool,
    /// Whether the distribution publishes a domain name of its own: a multi-tenant one serves
    /// no traffic directly and reports none.
    publishes_a_domain_name: bool,
    /// The output keys whose value must be the distribution's id, each written with a
    /// different intrinsic over the same resource.
    distribution_id_outputs: &'static [&'static str],
    /// The output keys whose value must be the origin access control's id.
    oac_id_outputs: &'static [&'static str],
}

/// The template of the multi-tenant row: a bucket, an origin access control, and a
/// `tenant-only` distribution whose single S3 origin reaches the bucket with that control.
///
/// This is the estate shape a SaaS-manager deployment declares: the distribution serves its
/// tenants rather than traffic of its own, so it states none of the members only a standard
/// distribution has.
fn multi_tenant_template(bucket: &str, comment: &str) -> String {
    serde_json::json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            BUCKET_LOGICAL_ID: {
                "Type": "AWS::S3::Bucket",
                "Properties": {"BucketName": bucket}
            },
            OAC_LOGICAL_ID: {
                "Type": "AWS::CloudFront::OriginAccessControl",
                "Properties": {"OriginAccessControlConfig": {
                    "Name": "tenant-origin-access",
                    "Description": comment,
                    "SigningProtocol": "sigv4",
                    "SigningBehavior": "always",
                    "OriginAccessControlOriginType": "s3"
                }}
            },
            DISTRIBUTION_LOGICAL_ID: {
                "Type": "AWS::CloudFront::Distribution",
                "Properties": {"DistributionConfig": {
                    "Enabled": true,
                    "Comment": comment,
                    "ConnectionMode": "tenant-only",
                    "Origins": [{
                        "Id": "the-bucket",
                        "DomainName": format!("{bucket}.s3.amazonaws.com"),
                        "S3OriginConfig": {"OriginAccessIdentity": ""},
                        "OriginAccessControlId": {"Ref": OAC_LOGICAL_ID}
                    }],
                    "DefaultCacheBehavior": {
                        "TargetOriginId": "the-bucket",
                        "ViewerProtocolPolicy": "redirect-to-https"
                    },
                    "TenantConfig": {"ParameterDefinitions": [{
                        "Name": "tenantName",
                        "Definition": {"StringSchema": {
                            "Required": false,
                            "Comment": "the tenant's own name",
                            "DefaultValue": "demo"
                        }}
                    }]}
                }}
            }
        },
        "Outputs": {
            "DistributionId": {"Value": {"Ref": DISTRIBUTION_LOGICAL_ID}},
            "DistributionIdAtt": {
                "Value": {"Fn::GetAtt": [DISTRIBUTION_LOGICAL_ID, "Id"]}
            },
            "OacId": {"Value": {"Ref": OAC_LOGICAL_ID}},
            "OacIdAtt": {"Value": {"Fn::GetAtt": [OAC_LOGICAL_ID, "Id"]}}
        }
    })
    .to_string()
}

/// The template of the standard row, in YAML: a bucket and a distribution that serves traffic
/// of its own, carrying every list-valued member CloudFront's model wraps in a container.
fn standard_template(bucket: &str, comment: &str) -> String {
    [
        "AWSTemplateFormatVersion: '2010-09-09'",
        "Resources:",
        &format!("  {BUCKET_LOGICAL_ID}:"),
        "    Type: AWS::S3::Bucket",
        "    Properties:",
        &format!("      BucketName: {bucket}"),
        &format!("  {OAC_LOGICAL_ID}:"),
        "    Type: AWS::CloudFront::OriginAccessControl",
        "    Properties:",
        "      OriginAccessControlConfig:",
        "        Name: standard-origin-access",
        &format!("        Description: {comment}"),
        "        SigningProtocol: sigv4",
        "        SigningBehavior: no-override",
        "        OriginAccessControlOriginType: s3",
        &format!("  {DISTRIBUTION_LOGICAL_ID}:"),
        "    Type: AWS::CloudFront::Distribution",
        "    Properties:",
        "      Tags:",
        "        - Key: Estate",
        "          Value: storefront",
        "      DistributionConfig:",
        "        Enabled: true",
        &format!("        Comment: {comment}"),
        "        PriceClass: PriceClass_100",
        "        IPV6Enabled: true",
        "        HttpVersion: http2",
        "        DefaultRootObject: index.html",
        "        Origins:",
        "          - Id: the-bucket",
        &format!("            DomainName: {bucket}.s3.us-east-1.amazonaws.com"),
        "            OriginCustomHeaders:",
        "              - HeaderName: X-Estate",
        "                HeaderValue: storefront",
        "            CustomOriginConfig:",
        "              HTTPPort: 80",
        "              HTTPSPort: 443",
        "              OriginProtocolPolicy: https-only",
        "              OriginSSLProtocols:",
        "                - TLSv1.2",
        "        DefaultCacheBehavior:",
        "          TargetOriginId: the-bucket",
        "          ViewerProtocolPolicy: allow-all",
        "          AllowedMethods:",
        "            - GET",
        "            - HEAD",
        "            - OPTIONS",
        "          CachedMethods:",
        "            - GET",
        "            - HEAD",
        "        CacheBehaviors:",
        "          - PathPattern: /images/*",
        "            TargetOriginId: the-bucket",
        "            ViewerProtocolPolicy: https-only",
        "          - PathPattern: /api/*",
        "            TargetOriginId: the-bucket",
        "            ViewerProtocolPolicy: redirect-to-https",
        "        CustomErrorResponses:",
        "          - ErrorCode: 404",
        "            ResponseCode: '200'",
        "            ResponsePagePath: /index.html",
        "            ErrorCachingMinTTL: 10",
        "        Restrictions:",
        "          GeoRestriction:",
        "            RestrictionType: whitelist",
        "            Locations:",
        "              - DE",
        "              - FR",
        "Outputs:",
        "  DistributionId:",
        &format!("    Value: !Ref {DISTRIBUTION_LOGICAL_ID}"),
        "  DistributionDomain:",
        &format!("    Value: !GetAtt {DISTRIBUTION_LOGICAL_ID}.DomainName"),
        "  OacId:",
        &format!("    Value: !GetAtt {OAC_LOGICAL_ID}.Id"),
        "",
    ]
    .join("\n")
}

/// The rows.
const ROWS: &[Row] = &[
    Row {
        case: "a multi-tenant distribution with an origin access control, in JSON",
        template: multi_tenant_template,
        expected_connection_mode: Some("tenant-only"),
        expected_comment: "the multi-tenant estate",
        expected_origin_domain: |bucket| format!("{bucket}.s3.amazonaws.com"),
        expected_aliases: &[],
        expected_cache_behaviors: &[],
        origin_names_the_control: true,
        publishes_a_domain_name: false,
        distribution_id_outputs: &["DistributionId", "DistributionIdAtt"],
        oac_id_outputs: &["OacId", "OacIdAtt"],
    },
    Row {
        case: "a standard distribution carrying every wrapped list, in YAML",
        template: standard_template,
        expected_connection_mode: None,
        expected_comment: "the storefront",
        expected_origin_domain: |bucket| format!("{bucket}.s3.us-east-1.amazonaws.com"),
        expected_aliases: &[],
        expected_cache_behaviors: &["/images/*", "/api/*"],
        origin_names_the_control: false,
        publishes_a_domain_name: true,
        distribution_id_outputs: &["DistributionId"],
        oac_id_outputs: &["OacId"],
    },
];

/// Every row: the ids the stack reports for its CloudFront resources are the ones CloudFront
/// minted, that service's own reads resolve them to the resources the template configured,
/// and deleting the stack takes them away again.
#[tokio::test]
async fn a_stack_provisions_the_cloudfront_resources_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stack_name = format!("cloudfront-estate-{index}");
        let bucket = format!("cloudfront-estate-origin-{index}");
        let template = (row.template)(&bucket, row.expected_comment);

        let (status, body) = cloudformation(
            &router,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        let described = settled_stack(&router, &stack_name, case).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        // The physical resource ids of the declared resources: what a client reads out of
        // DescribeStackResources, and what every identity output must carry.
        let (status, resources) = cloudformation(
            &router,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let reported = physical_ids_of(&resources);
        let distribution_id = reported
            .get(DISTRIBUTION_LOGICAL_ID)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no {DISTRIBUTION_LOGICAL_ID} in {resources}"));
        let oac_id = reported
            .get(OAC_LOGICAL_ID)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no {OAC_LOGICAL_ID} in {resources}"));
        for (what, id) in [("distribution", &distribution_id), ("control", &oac_id)] {
            assert!(
                is_cloudfront_id(id),
                "case {case:?}: expected a CloudFront-assigned {what} id, got {id:?}"
            );
        }
        assert_ne!(
            distribution_id, oac_id,
            "case {case:?}: expected the two resources to carry ids of their own"
        );

        let outputs = outputs_of(&described);
        for key in row.distribution_id_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&distribution_id),
                "case {case:?}: expected output {key:?} to be the distribution id \
                 {distribution_id:?}, got {:?}",
                outputs.get(*key)
            );
        }
        for key in row.oac_id_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&oac_id),
                "case {case:?}: expected output {key:?} to be the control's id {oac_id:?}, got \
                 {:?}",
                outputs.get(*key)
            );
        }

        // CloudFront itself resolves the id the stack published, to the configuration the
        // template declared.
        let (status, distribution) =
            cloudfront(&router, "GET", &format!("/2020-05-31/distribution/{distribution_id}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetDistribution on the id the stack \
             reported, got {status} with body {distribution}"
        );
        assert_eq!(
            opt_element(&distribution, "Id").as_deref(),
            Some(distribution_id.as_str()),
            "case {case:?}: expected the distribution to report the stack's id, got \
             {distribution}"
        );
        let config = element(&distribution, "DistributionConfig");
        assert_eq!(
            opt_element(&config, "Comment").as_deref(),
            Some(row.expected_comment),
            "case {case:?}: expected Comment {:?}, got {config}",
            row.expected_comment
        );
        assert_eq!(
            opt_element(&config, "ConnectionMode").as_deref(),
            row.expected_connection_mode,
            "case {case:?}: expected ConnectionMode {:?}, got {config}",
            row.expected_connection_mode
        );
        // The configuration's OWN `Enabled` is the one that follows its `Logging` element:
        // the service writes a configuration's members in the order its model declares them,
        // and several of the members before it carry an `Enabled` of their own (an origin's
        // Origin Shield, a behaviour's trusted key groups, the logging configuration).
        let after_logging = config.split("</Logging>").nth(1).unwrap_or_default();
        assert_eq!(
            opt_element(after_logging, "Enabled").as_deref(),
            Some("true"),
            "case {case:?}: expected the distribution to be enabled, got {config}"
        );
        let origins = element(&config, "Origins");
        assert_eq!(
            opt_element(&origins, "Quantity").as_deref(),
            Some("1"),
            "case {case:?}: expected one origin, got {origins}"
        );
        assert_eq!(
            opt_element(&origins, "DomainName"),
            Some((row.expected_origin_domain)(&bucket)),
            "case {case:?}: expected the origin to name the stack's bucket, got {origins}"
        );
        if row.origin_names_the_control {
            assert_eq!(
                opt_element(&origins, "OriginAccessControlId").as_deref(),
                Some(oac_id.as_str()),
                "case {case:?}: expected the origin to name the control CloudFront minted, got \
                 {origins}"
            );
        }
        assert_eq!(
            elements(&config, "CNAME"),
            row.expected_aliases
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?}: expected the declared aliases, got {config}"
        );
        assert_eq!(
            elements(&config, "PathPattern"),
            row.expected_cache_behaviors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?}: expected the declared cache behaviours, got {config}"
        );
        let domain_name = opt_element(&distribution, "DomainName").unwrap_or_default();
        if row.publishes_a_domain_name {
            assert_eq!(
                outputs.get("DistributionDomain"),
                Some(&domain_name),
                "case {case:?}: expected the output to be the domain name CloudFront \
                 published, got {:?}",
                outputs.get("DistributionDomain")
            );
            assert!(
                !domain_name.is_empty(),
                "case {case:?}: expected a published domain name, got {distribution}"
            );
        }

        // … and resolves the origin access control the same way, to the configuration the
        // template declared.
        let (status, control) = cloudfront(
            &router,
            "GET",
            &format!("/2020-05-31/origin-access-control/{oac_id}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetOriginAccessControl on the id the stack \
             reported, got {status} with body {control}"
        );
        assert_eq!(
            opt_element(&control, "SigningProtocol").as_deref(),
            Some("sigv4"),
            "case {case:?}: expected the declared signing protocol, got {control}"
        );
        assert_eq!(
            opt_element(&control, "OriginAccessControlOriginType").as_deref(),
            Some("s3"),
            "case {case:?}: expected the declared origin type, got {control}"
        );
        assert_eq!(
            opt_element(&control, "Description").as_deref(),
            Some(row.expected_comment),
            "case {case:?}: expected the declared description, got {control}"
        );

        // A CHANGED declaration reaches both resources through CloudFront's own update,
        // which takes the whole configuration and the version the caller read — so each
        // keeps the id its create minted and the change is readable through that service.
        let (status, updated) = cloudformation(
            &router,
            "UpdateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &(row.template)(&bucket, UPDATED_COMMENT)),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {updated}"
        );
        let described = settled_stack(&router, &stack_name, case).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("UPDATE_COMPLETE"),
            "case {case:?}: expected the stack to reach UPDATE_COMPLETE, got {described}"
        );
        let (status, distribution) =
            cloudfront(&router, "GET", &format!("/2020-05-31/distribution/{distribution_id}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the updated distribution to keep its id, got {status} \
             with body {distribution}"
        );
        assert_eq!(
            opt_element(&element(&distribution, "DistributionConfig"), "Comment").as_deref(),
            Some(UPDATED_COMMENT),
            "case {case:?}: expected the changed comment to reach CloudFront, got \
             {distribution}"
        );
        let (status, control) = cloudfront(
            &router,
            "GET",
            &format!("/2020-05-31/origin-access-control/{oac_id}"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the updated control to keep its id, got {status} with \
             body {control}"
        );
        assert_eq!(
            opt_element(&control, "Description").as_deref(),
            Some(UPDATED_COMMENT),
            "case {case:?}: expected the changed description to reach CloudFront, got {control}"
        );

        // Deleting the stack takes both resources away in CloudFront — a distribution only
        // after the stack has disabled it and CloudFront has propagated that.
        let (status, deleted) =
            cloudformation(&router, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {deleted}"
        );
        deleted_stack(&router, &stack_name, case).await;
        let (status, gone) =
            cloudfront(&router, "GET", &format!("/2020-05-31/distribution/{distribution_id}")).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted stack to leave no distribution, got {status} \
             with body {gone}"
        );
        let (status, gone) = cloudfront(
            &router,
            "GET",
            &format!("/2020-05-31/origin-access-control/{oac_id}"),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted stack to leave no origin access control, got \
             {status} with body {gone}"
        );
    }
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

/// A stack whose distribution declares a property no provider here applies is reported
/// CREATE_FAILED, and nothing is left behind in CloudFront: a resource reported created with
/// less than its declaration asked for is the shallow success the provider refuses.
#[tokio::test]
async fn a_declaration_this_emulator_cannot_apply_fails_the_stack() {
    let router = build_router();
    let stack_name = "cloudfront-unapplied-property";
    let template = serde_json::json!({
        "Resources": {
            DISTRIBUTION_LOGICAL_ID: {                "Type": "AWS::CloudFront::Distribution",
                "Properties": {"DistributionConfig": {
                    "Enabled": true,
                    "Comment": "a legacy member",
                    "CNAMEs": ["legacy.example.com"],
                    "Origins": [{
                        "Id": "the-origin",
                        "DomainName": "origin.example.com",
                        "CustomOriginConfig": {
                            "HTTPPort": 80,
                            "HTTPSPort": 443,
                            "OriginProtocolPolicy": "https-only"
                        }
                    }],
                    "DefaultCacheBehavior": {
                        "TargetOriginId": "the-origin",
                        "ViewerProtocolPolicy": "allow-all"
                    }
                }}
            }
        }
    })
    .to_string();

    let (status, body) = cloudformation(
        &router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a legacy member\": expected HTTP 200 from CreateStack, got {status} with body \
         {body}"
    );
    let described = settled_stack(&router, stack_name, "a legacy member").await;
    let reported = opt_element(&described, "StackStatus").unwrap_or_default();
    assert!(
        reported.starts_with("CREATE_FAILED") || reported.starts_with("ROLLBACK"),
        "case \"a legacy member\": expected the stack to fail, got {described}"
    );
    // WHY it failed is the provider's own account of the declaration, which the stack records
    // as the reason of the event that failed the resource.
    let (status, events) =
        cloudformation(&router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case \"a legacy member\": expected HTTP 200 from DescribeStackEvents, got {status} \
         with body {events}"
    );
    assert!(
        elements(&events, "ResourceStatusReason")
            .iter()
            .any(|reason| reason.contains("CNAMEs")),
        "case \"a legacy member\": expected the reason to name the property, got {events}"
    );
    let (status, listed) = cloudfront(&router, "GET", "/2020-05-31/distribution").await;
    assert_eq!(
        status, 200,
        "case \"a legacy member\": expected HTTP 200 from ListDistributions, got {status} with \
         body {listed}"
    );
    assert!(
        elements(&listed, "ARN").is_empty(),
        "case \"a legacy member\": expected no distribution to be left behind, got {listed}"
    );
}
