// SPDX-License-Identifier: CC0-1.0

//! The bound the git smart-HTTP transport holds a request BODY to, over the wire.
//!
//! The rule under test: the transport reads a body only up to
//! `crate::service::git_http::MAX_REQUEST_BODY_BYTES` — the largest pack a
//! `receive-pack` may carry plus the pkt-line prelude that names the ref updates — and
//! refuses a larger one with HTTP 413 BEFORE it is decoded, before this service's store
//! is locked and before a byte of it is expanded. A body within that bound is read, so
//! the bound is what the transport refuses on and not the request's shape: a short body
//! that is no git request at all draws the transport's own protocol refusal instead.
//!
//! This is about the EMULATOR as a program — a caller-supplied body drives inflate and
//! delta resolution under this service's store lock, so the size of what it may drive
//! is bounded at the seam the body arrives at rather than deep inside the pack reader.
//!
//! The rows span what the rule generalises over: both POST endpoints a body reaches
//! (`git-receive-pack` and `git-upload-pack`), a body at the bound and bodies past it
//! (one byte past, and far past), and both `Content-Encoding` forms a client may send
//! (none, and `gzip` — where the bound is about the bytes that ARRIVE, since a
//! compressed body is refused on its own size before it is inflated).

use serde_json::json;
use service_com_amazonaws_codecommit_codecommit_20150413::service::git_http::MAX_REQUEST_BODY_BYTES;
use smithy_server_runtime::MultiServiceRouter;

use super::repository_lifecycle::{emulator, ok, string_at};

/// The user name of the git credentials every request here presents. The account
/// holds no record of it, so it speaks for no IAM identity and no identity-based
/// policy is in force on it.
const GIT_USER: &str = "body-bound-user-at-000000000000";

/// The `Authorization` header a git client sends once it has been challenged: the git
/// credentials as HTTP Basic ones.
fn git_credentials() -> String {
    use base64::Engine as _;
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{GIT_USER}:password"))
    )
}

/// Send one git transport request the way a git client does: no SigV4 credential, no
/// `X-Amz-Target`, the `Host` of the clone URL it was given, and git credentials as
/// HTTP Basic ones — which is what the transport challenges a request carrying none
/// for.
async fn post(
    router: &MultiServiceRouter,
    url: &str,
    service: &str,
    content_encoding: Option<&str>,
    body: Vec<u8>,
) -> (u16, String) {
    let host = url
        .split_once("://")
        .map_or(url, |(_scheme, authority)| authority)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_string();
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("{url}/{service}"))
        .header("host", host)
        .header("user-agent", "git/2.43.0")
        .header("authorization", git_credentials())
        .header("content-type", format!("application/x-{service}-request"));
    if let Some(encoding) = content_encoding {
        request = request.header("content-encoding", encoding);
    }
    let response = router
        .dispatch(
            request
                .body(bytes::Bytes::from(body))
                .expect("a well-formed request"),
        )
        .await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

#[tokio::test]
async fn the_transport_refuses_a_request_body_past_the_bound_it_reads() {
    let router = emulator();
    let created = ok(
        &router,
        "CreateRepository",
        &json!({ "repositoryName": "body-bound" }),
    )
    .await;
    let clone_url = string_at(&created, "/repositoryMetadata/cloneUrlHttp");

    // (case, endpoint, body length, content-encoding, refused for its size?)
    let rows: [(&str, &str, usize, Option<&str>, bool); 8] = [
        ("a push at the bound", "git-receive-pack", MAX_REQUEST_BODY_BYTES, None, false),
        (
            "a push one byte past the bound",
            "git-receive-pack",
            MAX_REQUEST_BODY_BYTES + 1,
            None,
            true,
        ),
        (
            "a push far past the bound",
            "git-receive-pack",
            MAX_REQUEST_BODY_BYTES * 2,
            None,
            true,
        ),
        ("a fetch at the bound", "git-upload-pack", MAX_REQUEST_BODY_BYTES, None, false),
        (
            "a fetch one byte past the bound",
            "git-upload-pack",
            MAX_REQUEST_BODY_BYTES + 1,
            None,
            true,
        ),
        (
            // The bound is about the bytes that ARRIVE: a compressed body past it is
            // refused on its own size, before anything is inflated.
            "a gzip-declared push past the bound",
            "git-receive-pack",
            MAX_REQUEST_BODY_BYTES + 1,
            Some("gzip"),
            true,
        ),
        (
            "a gzip-declared fetch past the bound",
            "git-upload-pack",
            MAX_REQUEST_BODY_BYTES + 1,
            Some("gzip"),
            true,
        ),
        (
            // Well within the bound, and no git request at all: the transport answers
            // its own protocol refusal, so the 413 above is about the SIZE.
            "a short body that is no git request",
            "git-receive-pack",
            64,
            None,
            false,
        ),
    ];

    for (case, service, length, content_encoding, refused_for_size) in rows {
        let (status, body) = post(
            &router,
            &clone_url,
            service,
            content_encoding,
            vec![0u8; length],
        )
        .await;
        if refused_for_size {
            assert_eq!(
                status, 413,
                "case {case:?}: expected HTTP 413 for a {length}-byte body, got {status} \
                 with {body:?}"
            );
            assert!(
                body.contains(&MAX_REQUEST_BODY_BYTES.to_string()),
                "case {case:?}: the refusal must name the bound it holds the body to, got \
                 {body:?}"
            );
        } else {
            assert_ne!(
                status, 413,
                "case {case:?}: a {length}-byte body is within the bound, got {status} with \
                 {body:?}"
            );
            assert!(
                (400..500).contains(&status),
                "case {case:?}: expected the transport's own refusal of a body that is no \
                 git request, got {status} with {body:?}"
            );
        }
    }

    // Every refusal stored nothing: the repository still holds no branch.
    let branches = ok(
        &router,
        "ListBranches",
        &json!({ "repositoryName": "body-bound" }),
    )
    .await;
    assert_eq!(
        branches["branches"].as_array().map(Vec::len),
        Some(0),
        "a refused transport request must store nothing, got {branches:?}"
    );
}
