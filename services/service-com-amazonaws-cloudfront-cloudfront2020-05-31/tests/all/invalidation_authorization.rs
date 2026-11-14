// SPDX-License-Identifier: CC0-1.0

//! The INVALIDATION operations of a distribution are authorized by IAM before this service
//! looks at the distribution, end to end over HTTP.
//!
//! The RULE asserted here: a call to one of these operations is the IAM action
//! `cloudfront:<OperationName>` on the ARN of the resource the request addressed, and the
//! identity-based policies in force on the calling principal decide it — an explicit `Deny`
//! about it refuses it whatever grants it, otherwise it needs an `Allow` about it, and with
//! neither it falls to IAM's default deny. A call the policies do not permit is refused with
//! `AccessDenied` (HTTP 403) in IAM's own sentence, and the decision is taken BEFORE the
//! distribution is resolved — so a caller holding no grant that names a distribution the
//! account does not hold reads that refusal rather than `NoSuchDistribution`, and nothing is
//! created.
//!
//! Every fact the rule needs is staged through the SERVICE that owns it, on one router over
//! one engine: the principal and its policies through IAM's own `CreateUser` /
//! `CreateAccessKey` / `PutUserPolicy`, so the documents this service reads are the ones a
//! client really wrote, and the requests under test are signed with the access key id IAM
//! minted — the credential a client would use.
//!
//! What the rows span: whether the principal carries a policy at all; whether the policy
//! names this action, this service's whole namespace, another action or another service;
//! whether its `Resource` is every resource, this distribution's ARN or ANOTHER
//! distribution's; an explicit `Deny` beside a grant; and which of the three operations the
//! grant covers — so a policy that grants the create does not grant the reads and the other
//! way round. Two further cases are the ORDER: an unauthorized call naming a distribution id
//! the account does not hold, and the account's own credential, which the IAM store holds no
//! policies for.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{batch_xml, config, ACCOUNT_ID, API_VERSION};

/// The host every CloudFront request below is addressed to, and the one IAM's own calls are.
const HOST: &str = "cloudfront.amazonaws.com";
const IAM_HOST: &str = "iam.amazonaws.com";

/// IAM's API version, which every awsQuery call below carries.
const IAM_VERSION: &str = "2010-05-08";

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The access key id the account's OWN published credential presents, which the IAM store
/// holds no user for.
const ACCOUNT_KEY_ID: &str = "AKIDEXAMPLE";

/// The IAM user the requests under test are made as.
const USER_NAME: &str = "invalidator";

/// A distribution id of the right shape that the account holds NO distribution under, used
/// to pin that authorization is decided before the resource is resolved.
const ABSENT_DISTRIBUTION: &str = "E2ABSENTABSENT";

/// The wire error codes the two refusals carry.
const ACCESS_DENIED: &str = "AccessDenied";
const NO_SUCH_DISTRIBUTION: &str = "NoSuchDistribution";

/// One emulator: CloudFront and IAM over ONE shared engine, the shape
/// `core/router/src/lib.rs` mounts them in.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` presenting `access_key_id` and naming `service` in its credential
/// scope, which is what routes the request and what names the principal it speaks for.
fn authorization(service: &str, access_key_id: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// What the emulator answered: the status and the raw body.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The wire error code (`<Code>` of the restXml error envelope), or `""`.
    fn code(&self) -> String {
        element(&self.body, "Code").unwrap_or_default()
    }

    /// The refusal's message, or `""`.
    fn message(&self) -> String {
        element(&self.body, "Message").unwrap_or_default()
    }
}

/// Call one CloudFront operation signed with `access_key_id`.
async fn cloudfront(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    uri: &str,
    body: Option<&str>,
) -> Answer {
    let mut request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", HOST)
        .header("authorization", authorization("cloudfront", access_key_id));
    if body.is_some() {
        request = request.header("content-type", "application/xml");
    }
    let request = request
        .body(::bytes::Bytes::from(body.unwrap_or_default().to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    Answer { status, body }
}

/// Call one IAM operation over awsQuery as the account's own credential, and answer the XML
/// body. Every call here is setup, so a non-200 fails the case loudly.
async fn iam(router: &MultiServiceRouter, action: &str, parameters: &[(&str, String)]) -> String {
    let mut body = format!("Action={action}&Version={IAM_VERSION}");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(
            &::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC)
                .to_string(),
        );
    }
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", IAM_HOST)
        .header("authorization", authorization("iam", ACCOUNT_KEY_ID))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(::bytes::Bytes::from(body))
        .expect("the IAM request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(&response.into_body()).to_string();
    assert_eq!(
        status, 200,
        "setup {action}: expected HTTP 200 from IAM, got {status} with {text}"
    );
    text
}

/// The text of the first `<name>…</name>` element of an XML body.
fn element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// Create the user the requests under test are made as, give it `document` as its one inline
/// policy when the row states one, and answer the access key id IAM minted for it.
async fn given_principal(router: &MultiServiceRouter, case: &str, document: Option<String>) -> String {
    iam(router, "CreateUser", &[("UserName", USER_NAME.to_string())]).await;
    if let Some(document) = document {
        iam(
            router,
            "PutUserPolicy",
            &[
                ("UserName", USER_NAME.to_string()),
                ("PolicyName", "invalidations".to_string()),
                ("PolicyDocument", document),
            ],
        )
        .await;
    }
    let created = iam(
        router,
        "CreateAccessKey",
        &[("UserName", USER_NAME.to_string())],
    )
    .await;
    element(&created, "AccessKeyId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateAccessKey answered no id: {created}"))
}

/// An identity policy with one statement per `(effect, action, resource)` triple.
fn policy(statements: &[(&str, &str, &str)]) -> String {
    let rendered: Vec<String> = statements
        .iter()
        .map(|(effect, action, resource)| {
            ::serde_json::json!({
                "Effect": effect,
                "Action": action,
                "Resource": resource,
            })
            .to_string()
        })
        .collect();
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}",
        rendered.join(",")
    )
}

/// The ARN of a distribution of this account, as this service mints one: CloudFront is
/// GLOBAL, so the Region segment is empty.
fn distribution_arn(id: &str) -> String {
    format!("arn:aws:cloudfront::{ACCOUNT_ID}:distribution/{id}")
}

/// Which of the invalidation operations a row calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    Create,
    Get,
    List,
}

impl Operation {
    /// The operation's own name, which is the action IAM publishes it as.
    fn name(self) -> &'static str {
        match self {
            Self::Create => "CreateInvalidation",
            Self::Get => "GetInvalidation",
            Self::List => "ListInvalidations",
        }
    }

    /// The status this operation answers a call IAM permits with.
    fn success(self) -> u16 {
        match self {
            Self::Create => 201,
            Self::Get | Self::List => 200,
        }
    }
}

/// What a row expects of the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// IAM permits it, so the operation answers its own success status.
    Allowed,
    /// IAM refuses it: HTTP 403 `AccessDenied`, whose message closes with this clause.
    Refused(Clause),
}

/// Which of IAM's rules refused the call, as the clause its sentence closes with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clause {
    /// No readable document allowed the action — IAM's default deny.
    NoPolicyAllows,
    /// An explicit `Deny` in an identity-based document.
    ExplicitDeny,
}

impl Clause {
    /// The clause's own text, written as the literal IAM states rather than through the
    /// implementation's constant.
    fn text(self, action: &str) -> String {
        match self {
            Self::NoPolicyAllows => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// One row: the policy the principal carries, which operation it calls, and the verdict.
struct Row {
    case: &'static str,
    /// The statements of the principal's one inline policy; empty for a principal carrying
    /// no policy at all. `{arn}` stands for the distribution the call addresses.
    statements: &'static [(&'static str, &'static str, &'static str)],
    operation: Operation,
    expected: Expect,
}

/// `{arn}` in a row's `Resource`, replaced with the distribution the call addresses.
const THIS_DISTRIBUTION: &str = "{arn}";

/// Another distribution's ARN, which no call below addresses.
const OTHER_DISTRIBUTION: &str = "arn:aws:cloudfront::000000000000:distribution/E9OTHEROTHER";

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a principal carrying no policy at all",
            statements: &[],
            operation: Operation::Create,
            expected: Expect::Refused(Clause::NoPolicyAllows),
        },
        Row {
            case: "the action on every resource",
            statements: &[("Allow", "cloudfront:CreateInvalidation", "*")],
            operation: Operation::Create,
            expected: Expect::Allowed,
        },
        Row {
            case: "the action on this distribution's ARN",
            statements: &[("Allow", "cloudfront:CreateInvalidation", THIS_DISTRIBUTION)],
            operation: Operation::Create,
            expected: Expect::Allowed,
        },
        Row {
            case: "the action on ANOTHER distribution's ARN",
            statements: &[("Allow", "cloudfront:CreateInvalidation", OTHER_DISTRIBUTION)],
            operation: Operation::Create,
            expected: Expect::Refused(Clause::NoPolicyAllows),
        },
        Row {
            case: "this service's whole namespace",
            statements: &[("Allow", "cloudfront:*", "*")],
            operation: Operation::Create,
            expected: Expect::Allowed,
        },
        Row {
            case: "another action of this service",
            statements: &[("Allow", "cloudfront:CreateDistribution", "*")],
            operation: Operation::Create,
            expected: Expect::Refused(Clause::NoPolicyAllows),
        },
        Row {
            case: "another service's actions",
            statements: &[("Allow", "logs:*", "*")],
            operation: Operation::Create,
            expected: Expect::Refused(Clause::NoPolicyAllows),
        },
        Row {
            case: "an explicit deny beside the grant",
            statements: &[
                ("Allow", "cloudfront:*", "*"),
                ("Deny", "cloudfront:CreateInvalidation", THIS_DISTRIBUTION),
            ],
            operation: Operation::Create,
            expected: Expect::Refused(Clause::ExplicitDeny),
        },
        Row {
            case: "the read of one invalidation, granted",
            statements: &[("Allow", "cloudfront:GetInvalidation", THIS_DISTRIBUTION)],
            operation: Operation::Get,
            expected: Expect::Allowed,
        },
        Row {
            case: "the read of one invalidation, under the create's grant alone",
            statements: &[("Allow", "cloudfront:CreateInvalidation", "*")],
            operation: Operation::Get,
            expected: Expect::Refused(Clause::NoPolicyAllows),
        },
        Row {
            case: "the listing, granted",
            statements: &[("Allow", "cloudfront:ListInvalidations", THIS_DISTRIBUTION)],
            operation: Operation::List,
            expected: Expect::Allowed,
        },
        Row {
            case: "the listing, under the read's grant alone",
            statements: &[("Allow", "cloudfront:GetInvalidation", "*")],
            operation: Operation::List,
            expected: Expect::Refused(Clause::NoPolicyAllows),
        },
    ]
}

/// Create the distribution every row addresses, and one invalidation of it to read back, as
/// the ACCOUNT (whose own credential the IAM store holds no policies for). Answers the
/// distribution's id and the invalidation's id.
async fn given_distribution(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let created = cloudfront(
        router,
        ACCOUNT_KEY_ID,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        Some(&config("authorized", "ref-authorization").xml()),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: CreateDistribution must succeed, got {} with {}",
        created.status, created.body
    );
    let id = element(&created.body, "Id")
        .unwrap_or_else(|| panic!("case {case:?}: no distribution Id in {}", created.body));
    let invalidated = cloudfront(
        router,
        ACCOUNT_KEY_ID,
        "POST",
        &format!("/{API_VERSION}/distribution/{id}/invalidation"),
        Some(&batch_xml("ref-staged", &["/staged.html"])),
    )
    .await;
    assert_eq!(
        invalidated.status, 201,
        "case {case:?}: the staged CreateInvalidation must succeed, got {} with {}",
        invalidated.status, invalidated.body
    );
    let invalidation = element(&invalidated.body, "Id").unwrap_or_else(|| {
        panic!(
            "case {case:?}: no invalidation Id in {}",
            invalidated.body
        )
    });
    (id, invalidation)
}

/// Call `operation` on the distribution `id` as `access_key_id`.
async fn call_operation(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: Operation,
    id: &str,
    invalidation: &str,
) -> Answer {
    match operation {
        Operation::Create => {
            cloudfront(
                router,
                access_key_id,
                "POST",
                &format!("/{API_VERSION}/distribution/{id}/invalidation"),
                Some(&batch_xml("ref-under-test", &["/index.html"])),
            )
            .await
        }
        Operation::Get => {
            cloudfront(
                router,
                access_key_id,
                "GET",
                &format!("/{API_VERSION}/distribution/{id}/invalidation/{invalidation}"),
                None,
            )
            .await
        }
        Operation::List => {
            cloudfront(
                router,
                access_key_id,
                "GET",
                &format!("/{API_VERSION}/distribution/{id}/invalidation"),
                None,
            )
            .await
        }
    }
}

#[tokio::test]
async fn an_invalidation_call_is_decided_by_the_policies_in_force_on_the_caller() {
    for row in rows() {
        let case = row.case;
        let router = emulator();
        let (id, invalidation) = given_distribution(&router, case).await;
        let document = (!row.statements.is_empty()).then(|| {
            let arn = distribution_arn(&id);
            let statements: Vec<(&str, &str, String)> = row
                .statements
                .iter()
                .map(|(effect, action, resource)| {
                    let resource = if *resource == THIS_DISTRIBUTION {
                        arn.clone()
                    } else {
                        (*resource).to_string()
                    };
                    (*effect, *action, resource)
                })
                .collect();
            let borrowed: Vec<(&str, &str, &str)> = statements
                .iter()
                .map(|(effect, action, resource)| (*effect, *action, resource.as_str()))
                .collect();
            policy(&borrowed)
        });
        let access_key_id = given_principal(&router, case, document).await;

        let answered =
            call_operation(&router, &access_key_id, row.operation, &id, &invalidation).await;

        match row.expected {
            Expect::Allowed => assert_eq!(
                answered.status,
                row.operation.success(),
                "case {case:?}: expected {} from {}, got {} with {}",
                row.operation.success(),
                row.operation.name(),
                answered.status,
                answered.body
            ),
            Expect::Refused(clause) => {
                assert_eq!(
                    answered.status, 403,
                    "case {case:?}: expected HTTP 403 from {}, got {} with {}",
                    row.operation.name(),
                    answered.status,
                    answered.body
                );
                assert_eq!(
                    answered.code(),
                    ACCESS_DENIED,
                    "case {case:?}: expected the code {ACCESS_DENIED}, got {}",
                    answered.body
                );
                // IAM's own sentence: the principal that signed, the action it asked for,
                // the resource it addressed, and which of IAM's rules refused it.
                let action = format!("cloudfront:{}", row.operation.name());
                let expected = format!(
                    "User: arn:aws:iam::{ACCOUNT_ID}:user/{USER_NAME} is not authorized to \
                     perform: {action} on resource: {} {}",
                    distribution_arn(&id),
                    clause.text(&action)
                );
                assert_eq!(
                    answered.message(),
                    expected,
                    "case {case:?}: expected the sentence {expected:?}, got {}",
                    answered.body
                );
            }
        }
    }
}

/// One row of the ORDER table: the policy the principal carries, and the code a call naming
/// a distribution the account does not hold earns under it.
struct Order {
    case: &'static str,
    statements: &'static [(&'static str, &'static str, &'static str)],
    expected_code: &'static str,
}

/// RULE (the ORDER): authorization is decided before the distribution is resolved, so a
/// caller holding no grant that names a distribution the account does not hold reads the
/// authorization refusal — while the SAME call under a grant reads that the distribution is
/// not there.
#[tokio::test]
async fn an_unauthorized_call_is_refused_before_the_distribution_is_resolved() {
    let rows = [
        Order {
            case: "no grant at all",
            statements: &[],
            expected_code: ACCESS_DENIED,
        },
        Order {
            case: "a grant covering the action",
            statements: &[("Allow", "cloudfront:*", "*")],
            expected_code: NO_SUCH_DISTRIBUTION,
        },
    ];
    for row in rows {
        let case = row.case;
        let router = emulator();
        let document = (!row.statements.is_empty()).then(|| policy(row.statements));
        let access_key_id = given_principal(&router, case, document).await;
        let answered = cloudfront(
            &router,
            &access_key_id,
            "POST",
            &format!("/{API_VERSION}/distribution/{ABSENT_DISTRIBUTION}/invalidation"),
            Some(&batch_xml("ref-absent", &["/index.html"])),
        )
        .await;
        let expected_code = row.expected_code;
        let expected_status = if expected_code == ACCESS_DENIED {
            403
        } else {
            404
        };
        assert_eq!(
            answered.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with {}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.code(),
            expected_code,
            "case {case:?}: expected the code {expected_code}, got {}",
            answered.body
        );
    }
}

/// RULE (the caller this gate does not bound): the account's own published credential holds
/// no identity-based policies, so every one of these calls made with it is served — which is
/// what every other suite in this crate relies on.
#[tokio::test]
async fn the_accounts_own_credential_is_served() {
    let router = emulator();
    let (id, invalidation) = given_distribution(&router, "the account's own credential").await;
    for operation in [Operation::Create, Operation::Get, Operation::List] {
        let answered =
            call_operation(&router, ACCOUNT_KEY_ID, operation, &id, &invalidation).await;
        assert_eq!(
            answered.status,
            operation.success(),
            "case {:?}: expected {} for the account's own credential, got {} with {}",
            operation.name(),
            operation.success(),
            answered.status,
            answered.body
        );
    }
}
