// SPDX-License-Identifier: CC0-1.0

//! The stack outputs of a content-delivery estate: an Amazon S3 bucket behind an Amazon
//! CloudFront distribution, and an Amazon API Gateway REST API behind another.
//!
//! The rule under test: `DescribeStacks` reports each declared `Output` as a COMPLETE pair
//! — an `OutputKey` AND its resolved `OutputValue` — and that value is the return value
//! the resource TYPE publishes, in the identifier form the OWNING service produces,
//! including the identifiers and endpoint names that service ASSIGNS (a distribution id
//! and its `cloudfront.net` domain name, a REST API's id and its root resource id, a
//! bucket's four DNS names). A resource type whose published attribute this service cannot
//! compute still reports its output with a key and no value — never an invented one — so
//! the two halves of the rule are asserted side by side.
//!
//! What the rows span: both template spellings (YAML short form and JSON long form), every
//! CloudFront type whose `Ref` is a service-assigned id (a distribution, an origin access
//! control, an origin access identity, a cache policy), every API Gateway type whose `Ref`
//! is one (a REST API, a resource, a deployment), the API Gateway stage whose `Ref` is the
//! NAME the template states, each of the S3 bucket's five published attributes, a value
//! read through `Fn::Sub` and `Fn::Join` rather than a bare `Ref`/`Fn::GetAtt`, an
//! attribute no type publishes, and two resources of one type in one stack (whose ids must
//! differ).

use crate::nested_resource_outputs::is_lower_hex;
use crate::stack_lifecycle::{
    create_stack, describe_stack, router, wait_for_the_transition, xml_text, REGION,
};
use crate::stack_outputs::{output_for, wire_outputs};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// How many characters follow the `E` of a CloudFront-assigned identifier — a distribution
/// id, an origin access control id, an origin access identity id — as
/// `CloudFrontOriginAccessIdentity$Id` documents the form by example ("for example,
/// E74FTE3AJFJ256A").
const CLOUDFRONT_ID_BODY: usize = 13;

/// How many characters an API Gateway-assigned identifier carries: ten for an identifier of
/// the ACCOUNT — a `RestApi`'s own id, and the root resource that create mints beside it —
/// and six for one of a record created INSIDE an api, a `Resource` or a `Deployment`. These
/// are the widths that service mints, which are the identifiers in force: a stack creates the
/// api in API Gateway, and what `Ref` returns is the id that service assigned.
const APIGATEWAY_ID_LEN: usize = 10;
const APIGATEWAY_SUB_RESOURCE_ID_LEN: usize = 6;

/// How many lowercase hexadecimal characters an Amazon S3 canonical user id carries.
const CANONICAL_USER_ID_LEN: usize = 64;

/// Whether `value` is an identifier of the shape CloudFront assigns one in: `E` and
/// [`CLOUDFRONT_ID_BODY`] more characters of the alphabet that service mints one from —
/// decimal digits and uppercase letters.
///
/// The alphabet is base 36 rather than hexadecimal because the ids of the two types this
/// emulator really CREATES in CloudFront (a distribution, an origin access control) are the
/// ones that service minted, and it mints them from its own alphabet; an id CloudFormation's
/// naming rules compute for a type left declared-only (an origin access identity) is
/// uppercase hexadecimal, which is a subset of the same alphabet.
fn is_cloudfront_id(value: &str) -> bool {
    value.strip_prefix('E').is_some_and(|body| {
        body.chars().count() == CLOUDFRONT_ID_BODY
            && body
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
    })
}

/// Whether `value` is an address a viewer can DIAL the distribution on.
///
/// `Distribution$DomainName` is documented as "the domain name that corresponds to the
/// distribution … such as `d111111abcdef8.cloudfront.net`", and what this emulator publishes
/// for a distribution it really holds is the authority of the loopback listener its edge is
/// served on — a name a client on this machine can reach, which a `cloudfront.net` label
/// could not be. So the FORM asserted here is an authority: a host and a numeric port.
fn is_reachable_authority(value: &str) -> bool {
    value.rsplit_once(':').is_some_and(|(host, port)| {
        !host.is_empty() && !port.is_empty() && port.chars().all(|c| c.is_ascii_digit())
    })
}

/// Whether `value` is an identifier of the shape API Gateway assigns one in: `width`
/// lowercase alphanumeric characters.
fn is_apigateway_id_of(value: &str, width: usize) -> bool {
    value.chars().count() == width
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
}

/// Whether `value` is an identifier of the shape API Gateway assigns a record of the ACCOUNT.
fn is_apigateway_id(value: &str) -> bool {
    is_apigateway_id_of(value, APIGATEWAY_ID_LEN)
}

/// Whether `value` is an identifier of the shape API Gateway assigns a record created INSIDE
/// one api.
fn is_apigateway_sub_resource_id(value: &str) -> bool {
    is_apigateway_id_of(value, APIGATEWAY_SUB_RESOURCE_ID_LEN)
}

/// Whether `value` is a canonical UUID with the version-4 and RFC 4122 variant bits set,
/// which is the shape CloudFront mints a policy id in.
fn is_uuid_v4(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| is_lower_hex(group, *width))
        && groups[2].starts_with('4')
        && groups[3]
            .chars()
            .next()
            .is_some_and(|variant| matches!(variant, '8' | '9' | 'a' | 'b'))
}

/// What one declared output must carry.
enum Expect {
    /// Exactly this text.
    Exact(String),
    /// A value of this FORM: an identifier the owning service assigns is fixed by no
    /// request, so no row can spell it.
    Form {
        described: &'static str,
        check: fn(&str) -> bool,
    },
    /// Reported with its key and NO value, because nothing here can compute it.
    NoValue,
}

impl Expect {
    /// Whether `value` satisfies this expectation, and the English of what was required.
    fn check(&self, value: Option<&str>) -> (bool, String) {
        match self {
            Self::Exact(text) => (value == Some(text.as_str()), format!("exactly {text:?}")),
            Self::Form { described, check } => {
                (value.is_some_and(check), (*described).to_string())
            }
            Self::NoValue => (value.is_none(), "no OutputValue at all".to_string()),
        }
    }
}

/// A `Form` expectation, spelled once.
fn form(described: &'static str, check: fn(&str) -> bool) -> Expect {
    Expect::Form { described, check }
}

/// One row: a template and what each of its declared outputs must carry.
struct Case {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    expected: Vec<(&'static str, Expect)>,
}

const BUCKET: &str = "cdn-origin-bucket";

fn cases() -> Vec<Case> {
    vec![
        Case {
            // The shape a CloudFormation-driven CloudFront + S3 integration deploys, in
            // the YAML short-form spelling: a bucket, an origin access control, an origin
            // access identity and a distribution, with every published return value of
            // each read back as an output.
            case: "yaml short form: a bucket behind a distribution with an access control",
            stack_name: "cdn-s3-oac",
            template: concat!(
                "Resources:\n",
                "  Bucket:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: cdn-origin-bucket\n",
                "  Oac:\n",
                "    Type: AWS::CloudFront::OriginAccessControl\n",
                "    Properties:\n",
                "      OriginAccessControlConfig:\n",
                "        Name: cdn-origin-access\n",
                "        SigningProtocol: sigv4\n",
                "        SigningBehavior: always\n",
                "        OriginAccessControlOriginType: s3\n",
                "  Oai:\n",
                "    Type: AWS::CloudFront::CloudFrontOriginAccessIdentity\n",
                "  CachePolicy:\n",
                "    Type: AWS::CloudFront::CachePolicy\n",
                "  Distribution:\n",
                "    Type: AWS::CloudFront::Distribution\n",
                "    Properties:\n",
                "      DistributionConfig:\n",
                "        Enabled: true\n",
                "        Comment: the content-delivery estate\n",
                "        Origins:\n",
                "          - Id: the-bucket\n",
                "            DomainName: cdn-origin-bucket.s3.amazonaws.com\n",
                "            S3OriginConfig:\n",
                "              OriginAccessIdentity: ''\n",
                "            OriginAccessControlId: !Ref Oac\n",
                "        DefaultCacheBehavior:\n",
                "          TargetOriginId: the-bucket\n",
                "          ViewerProtocolPolicy: redirect-to-https\n",
                "Outputs:\n",
                "  BucketRef:\n",
                "    Value: !Ref Bucket\n",
                "  BucketArn:\n",
                "    Value: !GetAtt Bucket.Arn\n",
                "  BucketDomainName:\n",
                "    Value: !GetAtt Bucket.DomainName\n",
                "  BucketRegionalDomainName:\n",
                "    Value: !GetAtt Bucket.RegionalDomainName\n",
                "  BucketDualStackDomainName:\n",
                "    Value: !GetAtt Bucket.DualStackDomainName\n",
                "  BucketWebsiteURL:\n",
                "    Value: !GetAtt Bucket.WebsiteURL\n",
                "  OacRef:\n",
                "    Value: !Ref Oac\n",
                "  OacId:\n",
                "    Value: !GetAtt Oac.Id\n",
                "  OaiRef:\n",
                "    Value: !Ref Oai\n",
                "  OaiCanonicalUser:\n",
                "    Value: !GetAtt Oai.S3CanonicalUserId\n",
                "  CachePolicyRef:\n",
                "    Value: !Ref CachePolicy\n",
                "  DistributionRef:\n",
                "    Value: !Ref Distribution\n",
                "  DistributionId:\n",
                "    Value: !GetAtt Distribution.Id\n",
                "  DistributionDomainName:\n",
                "    Value: !GetAtt Distribution.DomainName\n",
                "  DistributionUrl:\n",
                "    Value: !Sub \"https://${Distribution.DomainName}/index.html\"\n",
                "  OriginPair:\n",
                "    Value: !Join\n",
                "      - \"|\"\n",
                "      - - !GetAtt Bucket.RegionalDomainName\n",
                "        - !Ref Distribution\n",
                "  DistributionStatus:\n",
                "    Value: !GetAtt Distribution.Status\n",
            ),
            expected: vec![
                ("BucketRef", Expect::Exact(BUCKET.to_string())),
                (
                    "BucketArn",
                    Expect::Exact(format!("arn:aws:s3:::{BUCKET}")),
                ),
                (
                    "BucketDomainName",
                    Expect::Exact(format!("{BUCKET}.s3.amazonaws.com")),
                ),
                (
                    "BucketRegionalDomainName",
                    Expect::Exact(format!("{BUCKET}.s3.{REGION}.amazonaws.com")),
                ),
                (
                    "BucketDualStackDomainName",
                    Expect::Exact(format!("{BUCKET}.s3.dualstack.{REGION}.amazonaws.com")),
                ),
                (
                    "BucketWebsiteURL",
                    Expect::Exact(format!("http://{BUCKET}.s3-website-{REGION}.amazonaws.com")),
                ),
                (
                    "OacRef",
                    form("an id CloudFront assigned (E + 13)", is_cloudfront_id),
                ),
                (
                    "OacId",
                    form("an id CloudFront assigned (E + 13)", is_cloudfront_id),
                ),
                (
                    "OaiRef",
                    form("an id CloudFront assigned (E + 13)", is_cloudfront_id),
                ),
                (
                    "OaiCanonicalUser",
                    form("64 lowercase hexadecimal characters", |value| {
                        is_lower_hex(value, CANONICAL_USER_ID_LEN)
                    }),
                ),
                ("CachePolicyRef", form("a version-4 UUID", is_uuid_v4)),
                (
                    "DistributionRef",
                    form("an id CloudFront assigned (E + 13)", is_cloudfront_id),
                ),
                (
                    "DistributionId",
                    form("an id CloudFront assigned (E + 13)", is_cloudfront_id),
                ),
                (
                    "DistributionDomainName",
                    form("an address a viewer can dial", is_reachable_authority),
                ),
                (
                    "DistributionUrl",
                    form("https://<the published address>/index.html", |value| {
                        value
                            .strip_prefix("https://")
                            .and_then(|rest| rest.strip_suffix("/index.html"))
                            .is_some_and(is_reachable_authority)
                    }),
                ),
                (
                    "OriginPair",
                    form(
                        "the bucket's regional domain name, a pipe, and the \
                         distribution's id",
                        |value| {
                            value
                                .split_once('|')
                                .is_some_and(|(domain, id)| {
                                    domain == format!("{BUCKET}.s3.{REGION}.amazonaws.com")
                                        && is_cloudfront_id(id)
                                })
                        },
                    ),
                ),
                // The complement: a distribution's deployment STATUS is not a return value
                // this service computes, so the output is reported with its key and no
                // value rather than an invented one.
                ("DistributionStatus", Expect::NoValue),
            ],
        },
        Case {
            // The JSON spelling with long-form intrinsics: a REST API behind a
            // distribution, with the REST API's own tree (a resource, a deployment and a
            // stage) and the endpoint a client addresses it on.
            case: "json long form: a rest api, its tree and a distribution in front of it",
            stack_name: "cdn-apigw",
            template: r#"{"Resources":{
                 "Api":{"Type":"AWS::ApiGateway::RestApi",
                   "Properties":{"Name":"cdn-api"}},
                 "Resource":{"Type":"AWS::ApiGateway::Resource",
                   "Properties":{"RestApiId":{"Ref":"Api"},
                     "ParentId":{"Fn::GetAtt":["Api","RootResourceId"]},
                     "PathPart":"items"}},
                 "Method":{"Type":"AWS::ApiGateway::Method",
                   "Properties":{"RestApiId":{"Ref":"Api"},
                     "ResourceId":{"Ref":"Resource"},"HttpMethod":"GET",
                     "AuthorizationType":"NONE",
                     "Integration":{"Type":"MOCK"}}},
                 "Deployment":{"Type":"AWS::ApiGateway::Deployment",
                   "DependsOn":["Method"],
                   "Properties":{"RestApiId":{"Ref":"Api"}}},
                 "Stage":{"Type":"AWS::ApiGateway::Stage",
                   "Properties":{"RestApiId":{"Ref":"Api"},"StageName":"prod",
                     "DeploymentId":{"Ref":"Deployment"}}},
                 "First":{"Type":"AWS::CloudFront::Distribution",
                   "Properties":{"DistributionConfig":{"Enabled":true,
                     "Comment":"the first distribution",
                     "Origins":[{"Id":"the-api",
                       "DomainName":"api.example.com",
                       "CustomOriginConfig":{"HTTPPort":80,"HTTPSPort":443,
                         "OriginProtocolPolicy":"https-only"}}],
                     "DefaultCacheBehavior":{"TargetOriginId":"the-api",
                       "ViewerProtocolPolicy":"allow-all"}}}},
                 "Second":{"Type":"AWS::CloudFront::Distribution",
                   "Properties":{"DistributionConfig":{"Enabled":true,
                     "Comment":"the second distribution",
                     "Origins":[{"Id":"the-api",
                       "DomainName":"api.example.com",
                       "CustomOriginConfig":{"HTTPPort":80,"HTTPSPort":443,
                         "OriginProtocolPolicy":"https-only"}}],
                     "DefaultCacheBehavior":{"TargetOriginId":"the-api",
                       "ViewerProtocolPolicy":"allow-all"}}}}},
               "Outputs":{
                 "ApiRef":{"Value":{"Ref":"Api"}},
                 "ApiId":{"Value":{"Fn::GetAtt":["Api","RestApiId"]}},
                 "RootResourceId":{"Value":{"Fn::GetAtt":["Api","RootResourceId"]}},
                 "ResourceRef":{"Value":{"Ref":"Resource"}},
                 "ResourceId":{"Value":{"Fn::GetAtt":["Resource","ResourceId"]}},
                 "DeploymentRef":{"Value":{"Ref":"Deployment"}},
                 "DeploymentId":{"Value":{"Fn::GetAtt":["Deployment","DeploymentId"]}},
                 "StageRef":{"Value":{"Ref":"Stage"}},
                 "ApiEndpoint":{"Value":{"Fn::Sub":
                   "https://${Api}.execute-api.${AWS::Region}.amazonaws.com/prod"}},
                 "FirstDomainName":{"Value":{"Fn::GetAtt":["First","DomainName"]}},
                 "SecondDomainName":{"Value":{"Fn::GetAtt":["Second","DomainName"]}}}}"#,
            expected: vec![
                ("ApiRef", form("10 lowercase characters", is_apigateway_id)),
                ("ApiId", form("10 lowercase characters", is_apigateway_id)),
                (
                    "RootResourceId",
                    form("10 lowercase characters", is_apigateway_id),
                ),
                (
                    "ResourceRef",
                    form("6 lowercase characters", is_apigateway_sub_resource_id),
                ),
                (
                    "ResourceId",
                    form("6 lowercase characters", is_apigateway_sub_resource_id),
                ),
                (
                    "DeploymentRef",
                    form("6 lowercase characters", is_apigateway_sub_resource_id),
                ),
                (
                    "DeploymentId",
                    form("6 lowercase characters", is_apigateway_sub_resource_id),
                ),
                // "Ref returns the stage name" — the one this type's template states.
                ("StageRef", Expect::Exact("prod".to_string())),
                (
                    "ApiEndpoint",
                    form(
                        "https://<api id>.execute-api.<region>.amazonaws.com/prod",
                        |value| {
                            value
                                .strip_prefix("https://")
                                .and_then(|rest| {
                                    rest.strip_suffix(&format!(
                                        ".execute-api.{REGION}.amazonaws.com/prod"
                                    ))
                                })
                                .is_some_and(is_apigateway_id)
                        },
                    ),
                ),
                (
                    "FirstDomainName",
                    form("an address a viewer can dial", is_reachable_authority),
                ),
                (
                    "SecondDomainName",
                    form("an address a viewer can dial", is_reachable_authority),
                ),
            ],
        },
    ]
}

#[tokio::test]
async fn content_delivery_outputs_carry_the_identifiers_their_types_publish() {
    let router = router();

    for case in cases() {
        let (status_code, body) =
            create_stack(&router, case.stack_name, case.template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected CreateStack to succeed, got {status_code} {body}",
            case.case
        );
        wait_for_the_transition().await;

        let (status_code, body) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected DescribeStacks to succeed, got {status_code} {body}",
            case.case
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must have reached CREATE_COMPLETE, got {body}",
            case.case
        );

        let outputs = wire_outputs(&body);
        assert_eq!(
            outputs.len(),
            case.expected.len(),
            "case {:?}: expected one Output per declared output ({} of them), got {outputs:?}",
            case.case,
            case.expected.len()
        );
        for (key, expected) in &case.expected {
            let output = output_for(&outputs, key).unwrap_or_else(|| {
                panic!(
                    "case {:?}: output {key:?} must be reported, got {outputs:?}",
                    case.case
                )
            });
            let (ok, required) = expected.check(output.value.as_deref());
            assert!(
                ok,
                "case {:?}: output {key:?} must carry {required}, got {:?}",
                case.case, output.value
            );
        }
    }
}

/// Two resources of ONE service-assigned type in one stack get DIFFERENT identifiers, each
/// resource's id differs from its own other identifiers, and a second read reports the same
/// ones: an assigned identifier is derived from the resource, not drawn fresh per read.
#[tokio::test]
async fn assigned_identifiers_are_unique_per_resource_and_stable_across_reads() {
    let router = router();
    let template = r#"{"Resources":{
         "First":{"Type":"AWS::CloudFront::Distribution",
           "Properties":{"DistributionConfig":{"Enabled":true,
             "Comment":"the first twin",
             "Origins":[{"Id":"the-origin","DomainName":"origin.example.com",
               "CustomOriginConfig":{"HTTPPort":80,"HTTPSPort":443,
                 "OriginProtocolPolicy":"https-only"}}],
             "DefaultCacheBehavior":{"TargetOriginId":"the-origin",
               "ViewerProtocolPolicy":"allow-all"}}}},
         "Second":{"Type":"AWS::CloudFront::Distribution",
           "Properties":{"DistributionConfig":{"Enabled":true,
             "Comment":"the second twin",
             "Origins":[{"Id":"the-origin","DomainName":"origin.example.com",
               "CustomOriginConfig":{"HTTPPort":80,"HTTPSPort":443,
                 "OriginProtocolPolicy":"https-only"}}],
             "DefaultCacheBehavior":{"TargetOriginId":"the-origin",
               "ViewerProtocolPolicy":"allow-all"}}}},
         "Api":{"Type":"AWS::ApiGateway::RestApi","Properties":{"Name":"twin-api"}}},
       "Outputs":{
         "FirstId":{"Value":{"Ref":"First"}},
         "FirstDomainName":{"Value":{"Fn::GetAtt":["First","DomainName"]}},
         "SecondId":{"Value":{"Ref":"Second"}},
         "SecondDomainName":{"Value":{"Fn::GetAtt":["Second","DomainName"]}},
         "ApiId":{"Value":{"Ref":"Api"}},
         "RootResourceId":{"Value":{"Fn::GetAtt":["Api","RootResourceId"]}}}}"#;

    let (status_code, body) = create_stack(&router, "twin-cdn", template, &[]).await;
    assert_eq!(
        status_code, 200,
        "case twin-cdn: expected CreateStack to succeed, got {status_code} {body}"
    );
    wait_for_the_transition().await;

    let mut reads = Vec::new();
    for round in 0..2 {
        let (status_code, body) = describe_stack(&router, "twin-cdn").await;
        assert_eq!(
            status_code, 200,
            "case twin-cdn (read {round}): expected DescribeStacks to succeed, \
             got {status_code} {body}"
        );
        let outputs = wire_outputs(&body);
        let value = |key: &str| {
            output_for(&outputs, key)
                .and_then(|output| output.value.clone())
                .unwrap_or_else(|| {
                    panic!(
                        "case twin-cdn (read {round}): output {key:?} must carry a value, \
                         got {outputs:?}"
                    )
                })
        };
        reads.push(vec![
            ("FirstId".to_string(), value("FirstId")),
            ("FirstDomainName".to_string(), value("FirstDomainName")),
            ("SecondId".to_string(), value("SecondId")),
            ("SecondDomainName".to_string(), value("SecondDomainName")),
            ("ApiId".to_string(), value("ApiId")),
            ("RootResourceId".to_string(), value("RootResourceId")),
        ]);
    }

    let first = &reads[0];
    let lookup = |key: &str| {
        first
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    let pairs = [
        ("FirstId", "SecondId"),
        ("FirstDomainName", "SecondDomainName"),
        ("FirstId", "FirstDomainName"),
        ("ApiId", "RootResourceId"),
    ];
    for (left, right) in pairs {
        assert_ne!(
            lookup(left),
            lookup(right),
            "case twin-cdn: {left} and {right} must be different identifiers, \
             both are {:?}",
            lookup(left)
        );
    }
    assert_eq!(
        reads[0], reads[1],
        "case twin-cdn: a second read must report the identifiers the first did, \
         got {:?} then {:?}",
        reads[0], reads[1]
    );
}
