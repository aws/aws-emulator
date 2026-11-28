// SPDX-License-Identifier: CC0-1.0

//! WHICH credentials CodeCommit's git smart-HTTP transport authenticates, over the WHOLE
//! mounted service set: IAM mints the credential through its own API and the git
//! transport is then driven with it, so what the transport accepts is checked against
//! the pair the account really issued rather than against a value this suite made up.
//!
//! The RULE under test: **the git transport serves only a credential the ACCOUNT
//! ISSUED.** A CodeCommit HTTPS git credential is the (`ServiceUserName`,
//! `ServicePassword`) pair `iam:CreateServiceSpecificCredential` mints for
//! `codecommit.amazonaws.com`, and the user name is not the secret —
//! `ListServiceSpecificCredentials` reports it — so the password is the whole proof: the
//! account keeps a verifier of the password it minted, and the transport asks the
//! account whether the pair a request presented is one it issued. Every other
//! presentation identifies nobody and is answered `403` with an
//! `UnrecognizedClientException` error document — a wrong password, a credential since
//! deleted, one made `Inactive`,
//! a password a reset superseded, a user name or access key id the account holds no
//! record of — while a request that presents no credential at all is answered `401`
//! with the `WWW-Authenticate: Basic` challenge that asks for one; and
//! nothing such a request asked for is served: a refused push leaves
//! every reference where it stood.
//!
//! Two crates have to agree for the rule to hold — IAM mints the credential and keeps
//! the verifier of its secret, CodeCommit's transport decides a request by asking the
//! account about the pair presented — so the suite lives here rather than in either of
//! them.
//!
//! The rows differ along exactly what the rule generalises over: which half of the pair
//! is wrong and HOW a wrong password differs from the right one (longer, shorter, empty,
//! another credential's, superseded by a reset), how a credential stopped being one the
//! account holds (deleted, deactivated, reset), which form a credential is presented in
//! (a generated git user name, an access key id as the credential helper writes one, a
//! SigV4 signature, the access key id this emulator published for ITSELF — the one
//! presented id that is served without the account holding a record of it, because that
//! is the position AWS account root credentials are in), and which endpoint of the
//! transport the request addresses (the ref advertisement and the pack request).

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{authorization, router};
use crate::query_wire::{element, form_encode};

/// The Region the repository and the git credential live in.
const REGION: &str = "us-west-2";

/// CodeCommit's `X-Amz-Target` prefix and its signing name.
const CODECOMMIT_TARGET_PREFIX: &str = "CodeCommit_20150413";
const CODECOMMIT_SIGNING_NAME: &str = "codecommit";

/// IAM's signing name and awsQuery API version.
const IAM_SIGNING_NAME: &str = "iam";
const IAM_API_VERSION: &str = "2010-05-08";

/// The service principal CodeCommit HTTPS git credentials are created for:
/// `CreateServiceSpecificCredential`'s own `ServiceName` for them.
const GIT_CREDENTIAL_SERVICE: &str = "codecommit.amazonaws.com";

/// The challenge a request that presented NO credential must carry, which is what makes
/// a git client ask its credential helper for credentials and send the request again.
const BASIC_CHALLENGE: &str = "Basic realm=\"\"";

/// The error document the transport refuses an authentication step with: the root
/// element is the code and `Message` is that code's own sentence. A request that
/// presented NO credential is told that one is required; a request whose credential this
/// account never issued is told the credential is not recognized.
const NOT_AUTHORIZED_DOCUMENT: &str =
    "<NotAuthorizedException><Message>SPNEGO token required</Message></NotAuthorizedException>";
const UNRECOGNIZED_CLIENT_DOCUMENT: &str = "<UnrecognizedClientException><Message>The security \
                                            token included in the request is invalid.</Message>\
                                            </UnrecognizedClientException>";

/// The branch the rows read and try to move, and the one a refused push would create.
const MAIN: &str = "main";
const WORK: &str = "work";

/// One CodeCommit call (`awsJson1_1`), with whatever status it answered.
async fn codecommit_answered(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "host",
            format!("{CODECOMMIT_SIGNING_NAME}.{REGION}.amazonaws.com"),
        )
        .header("authorization", authorization(CODECOMMIT_SIGNING_NAME, REGION))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("{CODECOMMIT_TARGET_PREFIX}.{operation}"),
        )
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One CodeCommit call that must answer 200.
async fn codecommit(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = codecommit_answered(router, operation, body).await;
    assert_eq!(
        status, 200,
        "CodeCommit {operation} must answer 200, got {status} with {parsed}"
    );
    parsed
}

/// One IAM call (`awsQuery`) that must answer 200, as its XML text.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> String {
    let mut body = format!("Action={action}&Version={IAM_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{IAM_SIGNING_NAME}.amazonaws.com"))
        .header("authorization", authorization(IAM_SIGNING_NAME, REGION))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "IAM {action} must answer 200, got {status} with {text}"
    );
    text
}

/// The string at `pointer` of a JSON answer.
fn string_at(payload: &Value, pointer: &str) -> String {
    payload
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string at {pointer} in {payload}"))
        .to_string()
}

/// A CodeCommit git credential as the account issued it: the generated user name a
/// client puts in a clone URL, the password that create was the only time of, and the id
/// every later operation on it takes.
struct GitCredential {
    service_user_name: String,
    password: String,
    credential_id: String,
    user_name: String,
}

/// Mint one through IAM's own API, for a fresh user the account grants both git actions:
/// `CreateUser`, `PutUserPolicy`, then `CreateServiceSpecificCredential` — the
/// documented setup flow for HTTPS git credentials.
async fn mint_git_credential(router: &MultiServiceRouter, user_name: &str) -> GitCredential {
    iam(router, "CreateUser", &[("UserName", user_name)]).await;
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["codecommit:GitPull", "codecommit:GitPush"],
            "Resource": "*",
        }],
    })
    .to_string();
    iam(
        router,
        "PutUserPolicy",
        &[
            ("UserName", user_name),
            ("PolicyName", "git-access"),
            ("PolicyDocument", &document),
        ],
    )
    .await;
    let created = iam(
        router,
        "CreateServiceSpecificCredential",
        &[
            ("UserName", user_name),
            ("ServiceName", GIT_CREDENTIAL_SERVICE),
        ],
    )
    .await;
    GitCredential {
        service_user_name: element(&created, "ServiceUserName"),
        password: element(&created, "ServicePassword"),
        credential_id: element(&created, "ServiceSpecificCredentialId"),
        user_name: user_name.to_string(),
    }
}

/// The `Authorization` header a git client sends once it has been challenged: the git
/// credentials as HTTP Basic ones, which is how a git client presents HTTPS git
/// credentials.
fn basic(user: &str, password: &str) -> String {
    format!(
        "Basic {}",
        aws_smithy_types::base64::encode(format!("{user}:{password}").as_bytes())
    )
}

/// One answer from the git transport.
struct Reply {
    status: u16,
    challenge: Option<String>,
    body: String,
}

/// Send one git transport request to `url` exactly as a git client does: no SigV4
/// credential, no `X-Amz-Target`, the `Host` of the clone URL it was given, and the
/// credentials `authorization` names — `None` for a request that presents none at all.
async fn git(
    router: &MultiServiceRouter,
    method: &str,
    url: &str,
    content_type: Option<&str>,
    credentials: Option<String>,
    body: Vec<u8>,
) -> Reply {
    let host = url
        .split_once("://")
        .map_or(url, |(_scheme, authority)| authority)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let mut request = http::Request::builder()
        .method(method)
        .uri(url)
        .header("host", host)
        .header("user-agent", "git/2.43.0");
    if let Some(credentials) = credentials {
        request = request.header("authorization", credentials);
    }
    if let Some(content_type) = content_type {
        request = request.header("content-type", content_type);
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body))
                .expect("the test request is well formed"),
        )
        .await;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    Reply {
        status: response.status().as_u16(),
        challenge: header("www-authenticate"),
        body: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// The ref advertisement a fetch starts with, presenting `credentials`.
async fn fetch_advertisement(
    router: &MultiServiceRouter,
    clone_url: &str,
    credentials: Option<String>,
) -> Reply {
    git(
        router,
        "GET",
        &format!("{clone_url}/info/refs?service=git-upload-pack"),
        None,
        credentials,
        Vec::new(),
    )
    .await
}

/// One pkt-line carrying `payload`: four hex length digits, the length counting
/// themselves.
fn pkt(payload: &str) -> Vec<u8> {
    let mut out = format!("{:04x}", payload.len() + 4).into_bytes();
    out.extend_from_slice(payload.as_bytes());
    out
}

/// A push that would move `reference` from `old` to `new`, presenting `credentials`.
/// Every object it names is already in the repository, so it carries no packfile.
async fn push(
    router: &MultiServiceRouter,
    clone_url: &str,
    credentials: Option<String>,
    old: &str,
    new: &str,
    reference: &str,
) -> Reply {
    let mut body = pkt(&format!("{old} {new} {reference}\0report-status"));
    body.extend_from_slice(b"0000");
    git(
        router,
        "POST",
        &format!("{clone_url}/git-receive-pack"),
        Some("application/x-git-receive-pack-request"),
        credentials,
        body,
    )
    .await
}

/// A repository with `main` at one commit and a second commit on a `work` branch:
/// everything a row needs to ask for a fast-forward of `main`.
struct Repo {
    name: String,
    clone_url: String,
    /// The commit `main` stands at.
    base: String,
    /// The commit a row asks to move `main` to.
    next: String,
}

async fn repository(router: &MultiServiceRouter, name: &str) -> Repo {
    let created = codecommit(router, "CreateRepository", &json!({ "repositoryName": name })).await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");
    let base = string_at(
        &codecommit(
            router,
            "PutFile",
            &json!({
                "repositoryName": name,
                "branchName": MAIN,
                "filePath": "README.md",
                "fileContent": aws_smithy_types::base64::encode(b"seed\n"),
                "commitMessage": "seed",
            }),
        )
        .await,
        "/commitId",
    );
    codecommit(
        router,
        "CreateBranch",
        &json!({ "repositoryName": name, "branchName": WORK, "commitId": base }),
    )
    .await;
    let next = string_at(
        &codecommit(
            router,
            "PutFile",
            &json!({
                "repositoryName": name,
                "branchName": WORK,
                "filePath": "work.txt",
                "fileContent": aws_smithy_types::base64::encode(b"work\n"),
                "commitMessage": "work",
                "parentCommitId": base,
            }),
        )
        .await,
        "/commitId",
    );
    Repo {
        name: name.to_string(),
        clone_url,
        base,
        next,
    }
}

/// The commit a branch of `repository` now points at, read back through the API.
async fn branch_tip(router: &MultiServiceRouter, repository: &str, branch: &str) -> String {
    let read = codecommit(
        router,
        "GetBranch",
        &json!({ "repositoryName": repository, "branchName": branch }),
    )
    .await;
    string_at(&read, "/branch/commitId")
}

/// Assert that `reply` is the refusal the transport answers a request it authenticates
/// nobody for with: the status, the AWS error document that status carries, the `Basic`
/// challenge on the `401` that asks a client for credentials and none on the `403` that
/// has nothing more to ask for, and no part of what was asked for.
fn assert_refused_authentication(case: &str, reply: &Reply, expected: u16) {
    assert_eq!(
        reply.status, expected,
        "case {case:?}: a request this account authenticates nobody for must be refused \
         {expected}, got {} with {:?}",
        reply.status, reply.body
    );
    let (document, challenge) = if expected == 401 {
        (NOT_AUTHORIZED_DOCUMENT, Some(BASIC_CHALLENGE))
    } else {
        (UNRECOGNIZED_CLIENT_DOCUMENT, None)
    };
    assert_eq!(
        reply.body, document,
        "case {case:?}: expected the error document {document:?}, got {:?}",
        reply.body
    );
    assert_eq!(
        reply.challenge.as_deref(),
        challenge,
        "case {case:?}: expected the challenge {challenge:?}, got {:?}",
        reply.challenge
    );
    assert!(
        !reply.body.contains("# service="),
        "case {case:?}: nothing of the advertisement may be served, got {:?}",
        reply.body
    );
}

/// The refusal of a request that presented a credential this account never issued: it
/// identifies nobody, and no challenge would change that.
fn assert_unrecognized(case: &str, reply: &Reply) {
    assert_refused_authentication(case, reply, 403);
}

/// Assert that `reply` is a served ref advertisement.
fn assert_served(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the pair the account issued must be served, got {} with {:?}",
        reply.status, reply.body
    );
    assert!(
        reply.body.contains("# service=git-upload-pack"),
        "case {case:?}: the answer must be the fetch advertisement, got {:?}",
        reply.body
    );
}

#[tokio::test]
async fn only_the_password_iam_minted_authenticates_a_git_credential() {
    let router = router();
    let repo = repository(&router, "git-password-verified").await;
    let credential = mint_git_credential(&router, "Mary_Major").await;
    // A second credential, for another user of the account: its password is one the
    // account DID mint, which is what makes it a test of the PAIR being checked together
    // rather than of a password nothing ever issued.
    let other = mint_git_credential(&router, "Li_Juan").await;

    // POSITIVE CONTROL FIRST: the pair the account issued is served, so every refusal
    // below is attributable to the half the row changed.
    assert_served(
        "the pair the account minted",
        &fetch_advertisement(
            &router,
            &repo.clone_url,
            Some(basic(&credential.service_user_name, &credential.password)),
        )
        .await,
    );

    // Each row: the user name presented, the password presented, and why that is not the
    // pair the account issued.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "the password with one more character",
            credential.service_user_name.clone(),
            format!("{}x", credential.password),
        ),
        (
            "the password with a character in front",
            credential.service_user_name.clone(),
            format!("x{}", credential.password),
        ),
        (
            "the password one character short",
            credential.service_user_name.clone(),
            credential.password[..credential.password.len() - 1].to_string(),
        ),
        (
            "no password at all",
            credential.service_user_name.clone(),
            String::new(),
        ),
        (
            "the user name as the password",
            credential.service_user_name.clone(),
            credential.service_user_name.clone(),
        ),
        (
            "another credential's password",
            credential.service_user_name.clone(),
            other.password.clone(),
        ),
        (
            "another credential's user name with this password",
            other.service_user_name.clone(),
            credential.password.clone(),
        ),
        (
            "a user name the account holds no credential for",
            format!("No_Such_User-at-{}", engine::emulator_account_id()),
            credential.password.clone(),
        ),
    ];

    for (case, user, password) in rows {
        assert_ne!(
            (user.as_str(), password.as_str()),
            (
                credential.service_user_name.as_str(),
                credential.password.as_str()
            ),
            "case {case:?}: the row must differ from the pair the account issued"
        );
        assert_unrecognized(
            case,
            &fetch_advertisement(&router, &repo.clone_url, Some(basic(&user, &password))).await,
        );
        // The same presentation proves nothing on the write endpoint either, and the
        // push it carried changes nothing.
        assert_unrecognized(
            case,
            &push(
                &router,
                &repo.clone_url,
                Some(basic(&user, &password)),
                &repo.base,
                &repo.next,
                "refs/heads/main",
            )
            .await,
        );
        assert_eq!(
            branch_tip(&router, &repo.name, MAIN).await,
            repo.base,
            "case {case:?}: a challenged push must leave the branch where it stood"
        );
    }

    // The credential that WAS issued still works after every refusal: nothing above
    // consumed it.
    assert_served(
        "the issued pair after the refusals",
        &fetch_advertisement(
            &router,
            &repo.clone_url,
            Some(basic(&credential.service_user_name, &credential.password)),
        )
        .await,
    );
}

/// How a credential stopped being one the account holds.
enum Revocation {
    /// `DeleteServiceSpecificCredential`, which revokes git access.
    Deleted,
    /// `UpdateServiceSpecificCredential` to `Inactive`, which `Status` documents as not
    /// valid for calls.
    Deactivated,
    /// `ResetServiceSpecificCredential`, which replaces the password: the superseded one
    /// is no longer the secret of the pair the account holds.
    Reset,
}

#[tokio::test]
async fn a_git_credential_the_account_no_longer_holds_is_refused() {
    let router = router();
    // Each row runs its own POSITIVE CONTROL first — the identical request with the same
    // credential — so the refusal is attributable to the IAM call the row made and to
    // nothing else.
    let rows = [
        (
            "a deleted credential",
            "git-revoked-deleted",
            "Mary_Major",
            Revocation::Deleted,
        ),
        (
            "a credential made inactive",
            "git-revoked-inactive",
            "Li_Juan",
            Revocation::Deactivated,
        ),
        (
            "a reset credential",
            "git-revoked-reset",
            "John_Stiles",
            Revocation::Reset,
        ),
    ];

    for (case, repository_name, user, revocation) in rows {
        let repo = repository(&router, repository_name).await;
        let credential = mint_git_credential(&router, user).await;
        let presented = basic(&credential.service_user_name, &credential.password);

        assert_served(
            case,
            &fetch_advertisement(&router, &repo.clone_url, Some(presented.clone())).await,
        );

        let mut replacement = None;
        match revocation {
            Revocation::Deleted => {
                iam(
                    &router,
                    "DeleteServiceSpecificCredential",
                    &[
                        ("UserName", &credential.user_name),
                        ("ServiceSpecificCredentialId", &credential.credential_id),
                    ],
                )
                .await;
            }
            Revocation::Deactivated => {
                iam(
                    &router,
                    "UpdateServiceSpecificCredential",
                    &[
                        ("UserName", &credential.user_name),
                        ("ServiceSpecificCredentialId", &credential.credential_id),
                        ("Status", "Inactive"),
                    ],
                )
                .await;
            }
            Revocation::Reset => {
                let reset = iam(
                    &router,
                    "ResetServiceSpecificCredential",
                    &[
                        ("UserName", &credential.user_name),
                        ("ServiceSpecificCredentialId", &credential.credential_id),
                    ],
                )
                .await;
                replacement = Some(element(&reset, "ServicePassword"));
            }
        }

        assert_unrecognized(
            case,
            &fetch_advertisement(&router, &repo.clone_url, Some(presented.clone())).await,
        );
        assert_unrecognized(
            case,
            &push(
                &router,
                &repo.clone_url,
                Some(presented),
                &repo.base,
                &repo.next,
                "refs/heads/main",
            )
            .await,
        );
        assert_eq!(
            branch_tip(&router, &repo.name, MAIN).await,
            repo.base,
            "case {case:?}: a challenged push must leave the branch where it stood"
        );

        // A reset does not revoke the credential: it replaces its secret, so the pair
        // the account now holds is served.
        if let Some(replacement) = replacement {
            assert_ne!(
                replacement, credential.password,
                "case {case:?}: a reset must mint a password of its own"
            );
            assert_served(
                "the password the reset minted",
                &fetch_advertisement(
                    &router,
                    &repo.clone_url,
                    Some(basic(&credential.service_user_name, &replacement)),
                )
                .await,
            );
        }
    }
}

#[tokio::test]
async fn a_credential_the_account_never_issued_identifies_nobody() {
    let router = router();
    let repo = repository(&router, "git-never-issued").await;
    // A user of the account that holds NO git credential: the account knows the user,
    // which is not the same as having issued it credentials for this service.
    iam(&router, "CreateUser", &[("UserName", "Nikki_Wolf")]).await;
    let account = engine::emulator_account_id();

    // Each row: what the request presents, why it is nothing this account issued, and
    // the status that case is refused with — a request that carries no credential at
    // all is asked for one, and a request that presented something the account never
    // issued identifies nobody, which no challenge would change.
    // The rows span the forms a client can present and a request that presents none.
    let rows: Vec<(&str, Option<String>, u16)> = vec![
        ("no credentials at all", None, 401),
        (
            "a git user name nothing minted",
            Some(basic(&format!("Invented_User-at-{account}"), "any-password")),
            403,
        ),
        (
            "a git user name of a user holding no credential",
            Some(basic(&format!("Nikki_Wolf-at-{account}"), "any-password")),
            403,
        ),
        (
            "an access key id no store holds",
            Some(basic("AKIAIOSFODNN7INVENTD", "a-signature")),
            403,
        ),
        (
            "an access key id with a session token, as the credential helper writes one",
            Some(basic(
                "ASIAIOSFODNN7INVENTD%an-opaque-session-token",
                "a-signature",
            )),
            403,
        ),
        (
            "a SigV4 signature naming a credential no store holds",
            Some(format!(
                "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7INVENTD/20260101/{REGION}/\
                 codecommit/aws4_request, SignedHeaders=host, Signature=0000"
            )),
            403,
        ),
        // A header that carries nothing a credential can be read out of still
        // PRESENTED something, and this refusal does not tell a malformed credential
        // apart from one the account never issued.
        ("an empty Basic value", Some("Basic ".to_string()), 403),
    ];

    for (case, credentials, expected) in rows {
        assert_refused_authentication(
            case,
            &fetch_advertisement(&router, &repo.clone_url, credentials.clone()).await,
            expected,
        );
        assert_refused_authentication(
            case,
            &push(
                &router,
                &repo.clone_url,
                credentials,
                "0000000000000000000000000000000000000000",
                &repo.next,
                "refs/heads/invented",
            )
            .await,
            expected,
        );
    }

    // Nothing any of them asked for was served: the branch nobody was authorized to
    // create does not exist, and `main` still stands where the setup left it.
    assert_eq!(
        branch_tip(&router, &repo.name, MAIN).await,
        repo.base,
        "a challenged push must leave the repository as it stood"
    );
    let (status, payload) = codecommit_answered(
        &router,
        "GetBranch",
        &json!({ "repositoryName": repo.name, "branchName": "invented" }),
    )
    .await;
    assert_ne!(
        status, 200,
        "no refused push may have created the branch it named, got {payload}"
    );

    // The one presented id that is NOT a credential of the account and is still served:
    // the access key id this emulator process PUBLISHED for itself, which is the position
    // AWS account root credentials are in — the account holds no identity-based policy for
    // it, so it is bounded by nothing. It is read from the authentication layer that owns
    // that pair rather than written here.
    let published = ::emulator_server::SigV4Authenticator::for_this_process();
    assert_served(
        "the access key id this emulator published for itself",
        &fetch_advertisement(
            &router,
            &repo.clone_url,
            Some(basic(&published.access_key_id, "a-signature")),
        )
        .await,
    );
    // And it is that id alone: the same id with one character changed is a credential
    // nothing published and nothing minted.
    let mut altered = published.access_key_id.clone();
    altered.push('X');
    assert_unrecognized(
        "an access key id one character from the published one",
        &fetch_advertisement(&router, &repo.clone_url, Some(basic(&altered, "a-signature"))).await,
    );
}
