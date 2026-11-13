// SPDX-License-Identifier: CC0-1.0

//! IAM authorization of the git transport: who may clone, and who may push WHICH
//! reference — the only branch protection CodeCommit has.
//!
//! The rules under test:
//!
//! 1. The clone URL the API announces is an address a stock git client DIALS: this
//!    suite opens a real socket to it and speaks HTTP/1.1 in the clear on it, with no
//!    trust material of any kind, and the git transport answers there.
//! 2. Every request to that transport is made with git credentials, and one that
//!    presents none is answered `401` with a `WWW-Authenticate: Basic` challenge —
//!    which is what makes a git client ask its credential helper for them.
//! 3. The principal those credentials speak for decides what the request may do:
//!    `codecommit:GitPull` for a fetch, `codecommit:GitPush` for a push, each
//!    evaluated against the identity-based policies in force on that principal.
//! 4. A push is authorized ONCE PER AUTHORIZATION STEP, and the steps do not all
//!    carry a reference: the advertisement a push begins with names none, so it is
//!    decided with no value for `codecommit:References`, while every ref update
//!    command is decided with its own. That is what makes the documented
//!    `Null: {"codecommit:References": "false"}` guard necessary — without it a
//!    `Deny` scoped to one reference by `StringEqualsIfExists` holds on the keyless
//!    step too and widens to the whole repository — and what makes a refused
//!    reference refused on AUTHORIZATION, leaving the ref where it stands while the
//!    other references of the same push are decided on their own.
//!
//! The tables span what the rules generalise over: which git service the step belongs
//! to, whether the step carries a reference, which reference a `Deny` names (a
//! branch, a tag, every reference), whether that `Deny` carries the guard, whether a
//! grant exists at all, and which of the two documented kinds of git credentials the
//! request presents. No policy here is the one from any one recording: each row's
//! document is built for the repository the row created.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;

use crate::repository_lifecycle::{base64, ok, string_at};

/// The role every row assumes to reach the git transport, and the session name its
/// credentials are minted under.
const PUSH_ROLE: &str = "CodeCommitPusher";
const PUSH_SESSION: &str = "branch-protection";

/// The caller that sets each row up and assumes the role: a key no store holds, which
/// is the account's own principal — the position the credentials this emulator
/// publishes at start are in.
const SETUP_ACCESS_KEY_ID: &str = "AKIDSETUPCALLER";

/// STS's awsQuery API version and signing name.
const STS_API_VERSION: &str = "2011-06-15";

/// The reference a row's protected branch is named by, and the two others the tables
/// contrast it with.
const MAIN_REF: &str = "refs/heads/main";
const FEATURE_REF: &str = "refs/heads/feature";
const TAG_REF: &str = "refs/tags/release-1";

/// The sentence CodeCommit's receive-pack refuses one reference with, which a git
/// client prints beside `! [remote rejected] <ref>`.
const NO_PERMISSION_TO_PUSH_REFERENCE: &str =
    "You don't have permission to push changes to this branch.";

/// This service and STS mounted on the SAME shared engine, so a session this suite
/// mints is resolved by the transport from the very record `sts:AssumeRole` wrote.
fn emulator_with_sts() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codecommit_codecommit_20150413::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(sts::EmulatedService::new(Arc::clone(&engine)));
    crate::wire::hold_long_term_key(&engine, crate::wire::DEFAULT_ACCESS_KEY_ID);
    (router, engine)
}

/// Record the role every row assumes, with `document` as its ONE inline policy — the
/// records `iam:CreateRole` and `iam:PutRolePolicy` write — and answer its ARN.
fn hold_role_with_policy(engine: &engine::InMemoryEngine, document: &str) -> String {
    let account = engine::emulator_account_id();
    let arn = format!("arn:aws:iam::{account}:role/{PUSH_ROLE}");
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{account}:root") },
            "Action": ["sts:AssumeRole"],
        }],
    })
    .to_string();
    let state = iam::service_state(engine).expect("the in-memory engine carries the IAM store");
    let mut store = state.lock();
    store.roles.insert(
        iam::name_key(PUSH_ROLE),
        Role {
            path: "/".to_string(),
            role_name: PUSH_ROLE.to_string(),
            role_id: sts::state::role_id(&arn),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: Some(trust),
            description: None,
            max_session_duration: Some(3_600),
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    let mut embedded = std::collections::BTreeMap::new();
    embedded.insert(
        iam::name_key("git-access"),
        iam::InlinePolicy {
            policy_name: "git-access".to_string(),
            policy_document: document.to_string(),
        },
    );
    store
        .role_inline_policies
        .insert(iam::name_key(PUSH_ROLE), embedded);
    arn
}

/// Mint the role's temporary credentials through `sts:AssumeRole` over the wire and
/// answer the access key id of the session — the user name the CodeCommit credential
/// helper writes for a caller holding them.
async fn assume_push_role(router: &MultiServiceRouter, case: &str, role_arn: &str) -> String {
    let body = format!(
        "Action=AssumeRole&Version={STS_API_VERSION}&RoleArn={}&RoleSessionName={PUSH_SESSION}",
        role_arn.replace(':', "%3A").replace('/', "%2F")
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("https://sts.amazonaws.com/")
        .header("host", "sts.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={SETUP_ACCESS_KEY_ID}/20260101/us-east-1/sts/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: sts:AssumeRole must mint the role's credentials, got {status} with {text}"
    );
    let start = text
        .find("<AccessKeyId>")
        .unwrap_or_else(|| panic!("case {case:?}: no <AccessKeyId> in {text}"))
        + "<AccessKeyId>".len();
    let end = text[start..]
        .find("</AccessKeyId>")
        .unwrap_or_else(|| panic!("case {case:?}: no </AccessKeyId> in {text}"))
        + start;
    text[start..end].to_string()
}

/// The `Authorization` header a git client sends once it has been challenged: the git
/// credentials as HTTP Basic ones. `session` is the session token the CodeCommit
/// credential helper appends to the access key id after a `%`, which the transport
/// must read past to find the credential it names.
fn basic(user: &str, session: Option<&str>) -> String {
    use base64::Engine as _;
    let user = match session {
        Some(token) => format!("{user}%{token}"),
        None => user.to_string(),
    };
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:a-signature"))
    )
}

/// One answer from the git transport.
struct Reply {
    status: u16,
    content_type: String,
    challenge: Option<String>,
    body: String,
}

/// Send one git transport request to `url`, with the credentials `authorization`
/// names — `None` for a request that presents none at all.
async fn git(
    router: &MultiServiceRouter,
    method: &str,
    url: &str,
    content_type: Option<&str>,
    authorization: Option<String>,
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
    if let Some(authorization) = authorization {
        request = request.header("authorization", authorization);
    }
    if let Some(content_type) = content_type {
        request = request.header("content-type", content_type);
    }
    let response = router
        .dispatch(
            request
                .body(bytes::Bytes::from(body))
                .expect("a well-formed request"),
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
        content_type: header("content-type").unwrap_or_default(),
        challenge: header("www-authenticate"),
        body: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// One pkt-line carrying `payload`: four hex length digits, the length counting
/// themselves.
fn pkt(payload: &str) -> Vec<u8> {
    let mut out = format!("{:04x}", payload.len() + 4).into_bytes();
    out.extend_from_slice(payload.as_bytes());
    out
}

/// The body of a `git-receive-pack` request that moves `reference` from `old` to
/// `new` and carries no packfile — every object it names is already in the
/// repository, which is what a push of objects the remote already holds sends.
fn push_body(old: &str, new: &str, reference: &str) -> Vec<u8> {
    let mut body = pkt(&format!("{old} {new} {reference}\0report-status"));
    body.extend_from_slice(b"0000");
    body
}

/// A repository with `main` at one commit, a `feature` branch at the same commit and
/// a second commit on a `work` branch: everything a row needs to ask for a
/// fast-forward of any reference.
struct Repo {
    name: String,
    arn: String,
    clone_url: String,
    /// The commit `main` and `feature` start at.
    base: String,
    /// The commit every row asks to move a reference to.
    next: String,
}

async fn repository(router: &MultiServiceRouter, name: &str) -> Repo {
    let created = ok(router, "CreateRepository", &json!({ "repositoryName": name })).await;
    let metadata = created
        .pointer("/repositoryMetadata")
        .cloned()
        .unwrap_or(Value::Null);
    let base = string_at(
        &ok(
            router,
            "PutFile",
            &json!({
                "repositoryName": name,
                "branchName": "main",
                "filePath": "README.md",
                "fileContent": base64(b"seed\n"),
                "commitMessage": "seed",
            }),
        )
        .await,
        "/commitId",
    );
    for branch in ["feature", "work"] {
        ok(
            router,
            "CreateBranch",
            &json!({ "repositoryName": name, "branchName": branch, "commitId": base }),
        )
        .await;
    }
    let next = string_at(
        &ok(
            router,
            "PutFile",
            &json!({
                "repositoryName": name,
                "branchName": "work",
                "filePath": "feature.txt",
                "fileContent": base64(b"work\n"),
                "commitMessage": "work",
                "parentCommitId": base,
            }),
        )
        .await,
        "/commitId",
    );
    Repo {
        name: name.to_string(),
        arn: string_at(&metadata, "/Arn"),
        clone_url: string_at(&metadata, "/cloneUrlHttp"),
        base,
        next,
    }
}

/// The commit a branch of `repository` now points at, read back through the API.
async fn branch_tip(router: &MultiServiceRouter, repository: &str, branch: &str) -> String {
    let read = ok(
        router,
        "GetBranch",
        &json!({ "repositoryName": repository, "branchName": branch }),
    )
    .await;
    string_at(&read, "/branch/commitId")
}

/// The identity policy of a principal that may pull and push everywhere in
/// `repository`: the grant every row starts from.
fn allow_all_git(repository: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["codecommit:GitPull", "codecommit:GitPush"],
            "Resource": repository,
        }],
    })
}

/// That grant plus a `Deny` on `codecommit:GitPush` scoped to `references` — with the
/// documented `Null` guard when `guarded`, which is what keeps the deny on the
/// references it names instead of on every one.
fn deny_push_to(repository: &str, references: &[&str], guarded: bool) -> Value {
    let mut condition = json!({
        "StringEqualsIfExists": { "codecommit:References": references },
    });
    if guarded {
        condition["Null"] = json!({ "codecommit:References": "false" });
    }
    let mut document = allow_all_git(repository);
    document["Statement"]
        .as_array_mut()
        .expect("the document carries a statement list")
        .push(json!({
            "Effect": "Deny",
            "Action": "codecommit:GitPush",
            "Resource": repository,
            "Condition": condition,
        }));
    document
}

#[tokio::test]
async fn the_announced_clone_url_is_an_address_a_git_client_dials() {
    let (router, engine) = emulator_with_sts();
    let repo = repository(&router, "dialable").await;
    let role = hold_role_with_policy(&engine, &allow_all_git(&repo.arn).to_string());
    let session = assume_push_role(&router, "dialable", &role).await;

    // The announced URL is dialled AS IT STANDS: the host and port come from the value
    // the API reported, and the exchange is plain HTTP/1.1 on a socket with no trust
    // material configured — which is the whole requirement on a clone URL. The address
    // an emulator announces is its own, so the host it names must be a loopback one and
    // the socket below dials that loopback address on the announced port.
    let authority = repo
        .clone_url
        .split_once("://")
        .map(|(_scheme, rest)| rest.split('/').next().unwrap_or_default().to_string())
        .unwrap_or_else(|| panic!("the announced clone URL names no origin: {}", repo.clone_url));
    assert!(
        repo.clone_url.starts_with("http://"),
        "the announced clone URL must name a scheme a stock git client dials without \
         extra trust material, got {:?}",
        repo.clone_url
    );
    let (announced_host, announced_port) = authority
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("the announced authority names no port: {authority}"));
    assert_eq!(
        announced_host, "127.0.0.1",
        "the announced git address must be this machine's own, got {authority:?}"
    );
    let announced_port: u16 = announced_port
        .parse()
        .unwrap_or_else(|error| panic!("the announced port {announced_port:?}: {error}"));

    // The rows span the two git services a client asks for, and whether it has been
    // challenged for credentials yet.
    let cases: [(&str, &str, Option<String>, u16); 3] = [
        (
            "a fetch with credentials",
            "git-upload-pack",
            Some(basic(&session, None)),
            200,
        ),
        (
            "a push with credentials",
            "git-receive-pack",
            Some(basic(&session, None)),
            200,
        ),
        ("a fetch with none", "git-upload-pack", None, 401),
    ];
    for (case, service, authorization, expected) in cases {
        let mut request = format!(
            "GET /v1/repos/{}/info/refs?service={service} HTTP/1.1\r\nHost: {authority}\r\n\
             User-Agent: git/2.43.0\r\nConnection: close\r\n",
            repo.name
        );
        if let Some(authorization) = &authorization {
            request.push_str(&format!("Authorization: {authorization}\r\n"));
        }
        request.push_str("\r\n");

        let mut socket = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, announced_port))
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the announced address {authority} must accept a connection: {error}")
            });
        socket
            .write_all(request.as_bytes())
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the request must be sent: {error}"));
        let mut answer = Vec::new();
        socket
            .read_to_end(&mut answer)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the answer must be read: {error}"));
        let answer = String::from_utf8_lossy(&answer).to_string();
        assert!(
            answer.starts_with(&format!("HTTP/1.1 {expected} ")),
            "case {case:?}: expected HTTP {expected} from the announced address, got {answer:?}"
        );
        if expected == 200 {
            assert!(
                answer.contains(&format!("# service={service}")),
                "case {case:?}: the answer must be the ref advertisement of {service}, got \
                 {answer:?}"
            );
        } else {
            assert!(
                answer.to_ascii_lowercase().contains("www-authenticate: basic"),
                "case {case:?}: a request presenting no credential must be challenged, got \
                 {answer:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_request_presenting_no_git_credential_is_challenged() {
    let (router, _engine) = emulator_with_sts();
    let repo = repository(&router, "challenged").await;

    // The rows span every endpoint of the transport: both advertisements and both
    // pack requests.
    let cases: [(&str, &str, &str, Option<&str>); 4] = [
        (
            "the fetch advertisement",
            "GET",
            "/info/refs?service=git-upload-pack",
            None,
        ),
        (
            "the push advertisement",
            "GET",
            "/info/refs?service=git-receive-pack",
            None,
        ),
        (
            "a fetch",
            "POST",
            "/git-upload-pack",
            Some("application/x-git-upload-pack-request"),
        ),
        (
            "a push",
            "POST",
            "/git-receive-pack",
            Some("application/x-git-receive-pack-request"),
        ),
    ];
    for (case, method, suffix, content_type) in cases {
        let reply = git(
            &router,
            method,
            &format!("{}{suffix}", repo.clone_url),
            content_type,
            None,
            Vec::new(),
        )
        .await;
        assert_eq!(
            reply.status, 401,
            "case {case:?}: a request presenting no git credential must be refused 401, got \
             {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.challenge.as_deref(),
            Some("Basic realm=\"CodeCommit\""),
            "case {case:?}: a 401 must carry the challenge that makes a git client supply \
             credentials, got {:?}",
            reply.challenge
        );
    }

    // Nothing was served: the repository still stands where the setup left it.
    assert_eq!(
        branch_tip(&router, &repo.name, "main").await,
        repo.base,
        "a challenged push must leave the branch where it stood"
    );
}

#[tokio::test]
async fn a_push_is_authorized_once_per_reference() {
    // Each row: the policy in force, the reference the push names, and whether the
    // ref update is admitted. The rows span which reference a `Deny` names (the one
    // pushed, another branch, a tag, every reference), whether it carries the
    // documented `Null` guard, and whether a grant exists at all.
    struct Case {
        label: &'static str,
        repository: &'static str,
        policy: fn(&str) -> Value,
        reference: &'static str,
        branch: &'static str,
        admitted: bool,
    }
    let cases: [Case; 7] = [
        Case {
            label: "a grant with no deny admits the branch",
            repository: "per-ref-granted",
            policy: |arn| allow_all_git(arn),
            reference: MAIN_REF,
            branch: "main",
            admitted: true,
        },
        Case {
            label: "a guarded deny refuses the reference it names",
            repository: "per-ref-denied",
            policy: |arn| deny_push_to(arn, &[MAIN_REF], true),
            reference: MAIN_REF,
            branch: "main",
            admitted: false,
        },
        Case {
            label: "a guarded deny admits another reference",
            repository: "per-ref-other",
            policy: |arn| deny_push_to(arn, &[MAIN_REF], true),
            reference: FEATURE_REF,
            branch: "feature",
            admitted: true,
        },
        Case {
            label: "a guarded deny naming a tag admits a branch",
            repository: "per-ref-tag-denied",
            policy: |arn| deny_push_to(arn, &[TAG_REF], true),
            reference: MAIN_REF,
            branch: "main",
            admitted: true,
        },
        Case {
            label: "a guarded deny naming several references refuses each of them",
            repository: "per-ref-list",
            policy: |arn| deny_push_to(arn, &[TAG_REF, FEATURE_REF], true),
            reference: FEATURE_REF,
            branch: "feature",
            admitted: false,
        },
        Case {
            label: "a deny on every reference refuses the branch",
            repository: "per-ref-all",
            policy: |arn| {
                let mut document = allow_all_git(arn);
                document["Statement"]
                    .as_array_mut()
                    .expect("the document carries a statement list")
                    .push(json!({
                        "Effect": "Deny",
                        "Action": "codecommit:GitPush",
                        "Resource": arn,
                    }));
                document
            },
            reference: MAIN_REF,
            branch: "main",
            admitted: false,
        },
        Case {
            label: "a principal granted only the pull refuses the branch",
            repository: "per-ref-pull-only",
            policy: |arn| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": "codecommit:GitPull",
                        "Resource": arn,
                    }],
                })
            },
            reference: MAIN_REF,
            branch: "main",
            admitted: false,
        },
    ];

    for case in cases {
        let (router, engine) = emulator_with_sts();
        let repo = repository(&router, case.repository).await;
        let role = hold_role_with_policy(&engine, &(case.policy)(&repo.arn).to_string());
        let session = assume_push_role(&router, case.label, &role).await;

        let reply = git(
            &router,
            "POST",
            &format!("{}/git-receive-pack", repo.clone_url),
            Some("application/x-git-receive-pack-request"),
            // The credential helper writes the session token after the access key id;
            // the transport reads the key id in front of it.
            Some(basic(&session, Some("an-opaque-session-token"))),
            push_body(&repo.base, &repo.next, case.reference),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: receive-pack reports its verdict per ref in its own status \
             report, got {} with {:?}",
            case.label, reply.status, reply.body
        );
        assert_eq!(
            reply.content_type, "application/x-git-receive-pack-result",
            "case {:?}: the status report's content type",
            case.label
        );
        let expected_tip = if case.admitted {
            repo.next.clone()
        } else {
            repo.base.clone()
        };
        if case.admitted {
            assert!(
                reply.body.contains(&format!("ok {}", case.reference)),
                "case {:?}: the reference must be admitted, got {:?}",
                case.label,
                reply.body
            );
        } else {
            assert!(
                reply
                    .body
                    .contains(&format!("ng {} {NO_PERMISSION_TO_PUSH_REFERENCE}", case.reference)),
                "case {:?}: the reference must be refused with the authorization \
                 sentence, got {:?}",
                case.label,
                reply.body
            );
            assert!(
                !reply.body.contains("fetch first") && !reply.body.contains("non-fast-forward"),
                "case {:?}: an authorization refusal is not git's own stale-ref refusal, \
                 got {:?}",
                case.label,
                reply.body
            );
        }
        assert_eq!(
            branch_tip(&router, &repo.name, case.branch).await,
            expected_tip,
            "case {:?}: the branch must stand at the commit the verdict implies",
            case.label
        );
    }
}

#[tokio::test]
async fn one_push_decides_each_of_its_references_on_its_own() {
    let (router, engine) = emulator_with_sts();
    let repo = repository(&router, "two-refs").await;
    let role = hold_role_with_policy(
        &engine,
        &deny_push_to(&repo.arn, &[MAIN_REF], true).to_string(),
    );
    let session = assume_push_role(&router, "two refs in one push", &role).await;

    // ONE request carrying both commands: the denied reference is refused and the
    // admitted one lands, which is what a per-reference decision means.
    let mut body = pkt(&format!(
        "{} {} {MAIN_REF}\0report-status",
        repo.base, repo.next
    ));
    body.extend_from_slice(&pkt(&format!(
        "{} {} {FEATURE_REF}",
        repo.base, repo.next
    )));
    body.extend_from_slice(b"0000");
    let reply = git(
        &router,
        "POST",
        &format!("{}/git-receive-pack", repo.clone_url),
        Some("application/x-git-receive-pack-request"),
        Some(basic(&session, None)),
        body,
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "a push carrying two commands is answered with a status report, got {} with {:?}",
        reply.status, reply.body
    );
    assert!(
        reply
            .body
            .contains(&format!("ng {MAIN_REF} {NO_PERMISSION_TO_PUSH_REFERENCE}")),
        "the protected reference must be refused, got {:?}",
        reply.body
    );
    assert!(
        reply.body.contains(&format!("ok {FEATURE_REF}")),
        "the reference the deny does not name must be admitted, got {:?}",
        reply.body
    );
    assert_eq!(
        branch_tip(&router, &repo.name, "main").await,
        repo.base,
        "the refused reference must stand where it did"
    );
    assert_eq!(
        branch_tip(&router, &repo.name, "feature").await,
        repo.next,
        "the admitted reference must carry the pushed commit"
    );
}

#[tokio::test]
async fn the_keyless_advertisement_step_is_what_the_null_guard_narrows() {
    // Each row: the policy in force, and the status each advertisement answers. The
    // advertisement carries NO reference, so a deny scoped by `StringEqualsIfExists`
    // holds on it unless the `Null` guard says the step must carry one.
    struct Case {
        label: &'static str,
        repository: &'static str,
        policy: fn(&str) -> Value,
        fetch: u16,
        push: u16,
    }
    let cases: [Case; 5] = [
        Case {
            label: "a grant with no deny",
            repository: "keyless-granted",
            policy: |arn| allow_all_git(arn),
            fetch: 200,
            push: 200,
        },
        Case {
            label: "a guarded deny on one branch",
            repository: "keyless-guarded",
            policy: |arn| deny_push_to(arn, &[MAIN_REF], true),
            fetch: 200,
            push: 200,
        },
        Case {
            label: "the same deny with the guard dropped",
            repository: "keyless-unguarded",
            policy: |arn| deny_push_to(arn, &[MAIN_REF], false),
            fetch: 200,
            push: 403,
        },
        Case {
            label: "a principal granted only the pull",
            repository: "keyless-pull-only",
            policy: |arn| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": "codecommit:GitPull",
                        "Resource": arn,
                    }],
                })
            },
            fetch: 200,
            push: 403,
        },
        Case {
            label: "a principal granted nothing",
            repository: "keyless-no-grant",
            policy: |_arn| json!({ "Version": "2012-10-17", "Statement": [] }),
            fetch: 403,
            push: 403,
        },
    ];

    for case in cases {
        let (router, engine) = emulator_with_sts();
        let repo = repository(&router, case.repository).await;
        let role = hold_role_with_policy(&engine, &(case.policy)(&repo.arn).to_string());
        let session = assume_push_role(&router, case.label, &role).await;

        for (service, expected) in [
            ("git-upload-pack", case.fetch),
            ("git-receive-pack", case.push),
        ] {
            let reply = git(
                &router,
                "GET",
                &format!("{}/info/refs?service={service}", repo.clone_url),
                None,
                Some(basic(&session, None)),
                Vec::new(),
            )
            .await;
            assert_eq!(
                reply.status, expected,
                "case {:?}: the {service} advertisement, got {} with {:?}",
                case.label, reply.status, reply.body
            );
            if expected == 200 {
                assert_eq!(
                    reply.content_type,
                    format!("application/x-{service}-advertisement"),
                    "case {:?}: the {service} advertisement's content type",
                    case.label
                );
            }
        }
    }
}

#[tokio::test]
async fn a_service_specific_git_credential_speaks_for_the_user_that_holds_it() {
    // The second documented way to get CodeCommit git credentials: an IAM
    // service-specific credential, whose generated `ServiceUserName` is what a client
    // puts in the URL. It must reach the same decision the session credentials of a
    // role reach — the policies of the identity that HOLDS it.
    let (router, engine) = emulator_with_sts();
    let repo = repository(&router, "service-credential").await;
    let user = "Mary_Major";
    let user_arn = crate::wire::hold_long_term_key_for(&engine, user, "AKIDGITUSERKEY");
    let service_user_name = format!("{user}-at-{}", engine::emulator_account_id());
    {
        let state = iam::service_state(&*engine).expect("the engine carries the IAM store");
        let mut store = state.lock();
        let mut embedded = std::collections::BTreeMap::new();
        embedded.insert(
            iam::name_key("git-access"),
            iam::InlinePolicy {
                policy_name: "git-access".to_string(),
                policy_document: deny_push_to(&repo.arn, &[MAIN_REF], true).to_string(),
            },
        );
        store
            .user_inline_policies
            .insert(iam::name_key(user), embedded);
        store.service_specific_credentials.insert(
            "ACCAEXAMPLEGITCREDENTIAL".to_string(),
            service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::ServiceSpecificCredentialMetadata {
                user_name: user.to_string(),
                status: iam::CREDENTIAL_STATUS_ACTIVE.to_string(),
                service_user_name: Some(service_user_name.clone()),
                service_credential_alias: None,
                create_date: ::aws_smithy_types::DateTime::from_secs(0),
                expiration_date: None,
                service_specific_credential_id: "ACCAEXAMPLEGITCREDENTIAL".to_string(),
                service_name: "codecommit.amazonaws.com".to_string(),
            },
        );
    }
    assert!(
        user_arn.ends_with(&format!("user/{user}")),
        "the suite holds the credential for an IAM user of the account, got {user_arn:?}"
    );

    // The rows span the two references the user's own policy decides differently.
    for (case, reference, branch, admitted) in [
        ("the protected branch", MAIN_REF, "main", false),
        ("another branch", FEATURE_REF, "feature", true),
    ] {
        let reply = git(
            &router,
            "POST",
            &format!("{}/git-receive-pack", repo.clone_url),
            Some("application/x-git-receive-pack-request"),
            Some(basic(&service_user_name, None)),
            push_body(&repo.base, &repo.next, reference),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: receive-pack answers a status report, got {} with {:?}",
            reply.status, reply.body
        );
        let expected = if admitted {
            format!("ok {reference}")
        } else {
            format!("ng {reference} {NO_PERMISSION_TO_PUSH_REFERENCE}")
        };
        assert!(
            reply.body.contains(&expected),
            "case {case:?}: expected {expected:?} in the status report, got {:?}",
            reply.body
        );
        assert_eq!(
            branch_tip(&router, &repo.name, branch).await,
            if admitted {
                repo.next.clone()
            } else {
                repo.base.clone()
            },
            "case {case:?}: the branch must stand at the commit the verdict implies"
        );
    }
}
