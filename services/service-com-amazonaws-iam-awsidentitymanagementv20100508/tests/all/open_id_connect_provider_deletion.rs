// SPDX-License-Identifier: CC0-1.0

//! `DeleteOpenIDConnectProvider`, end to end over the awsQuery wire, cross-read through
//! `GetOpenIDConnectProvider`, `ListOpenIDConnectProviderTags`,
//! `ListOpenIDConnectProviders` and `GetRole`.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of providers that were
//! PERSISTED by `CreateOpenIDConnectProvider`, never a handler called directly. The
//! awsQuery harness is the one `tests/all/user_lifecycle.rs` owns, the parameter and tag
//! readers are `tests/all/role_lifecycle.rs`'s, the provider fixtures and the ARN
//! derivation are `tests/all/open_id_connect_providers.rs`'s and the list-entry reader is
//! `tests/all/saml_provider_listing.rs`'s, so a request is built and read exactly one way
//! across this crate.
//!
//! The rule under test is that the delete FORGETS the provider the ARN names, with
//! everything held on that record — its URL, client IDs, thumbprints, creation instant and
//! tags — and touches nothing else: afterwards no call on that ARN resolves it, the listing
//! no longer carries it, the URL it was registered under is free for a create that mints a
//! NEW record holding only what that create asked for, every other provider the account
//! holds reads back exactly as its create wrote it, and a role whose trust policy names the
//! deleted provider keeps that document — "deleting an IAM OIDC provider resource does not
//! update any roles that reference the provider as a principal in their trust policies".
//! An ARN the account holds no provider under is refused as absent and a value that is not
//! an ARN as invalid, with nothing forgotten either way.
//!
//! Each suite is a TABLE whose rows differ along what that rule generalises over: the
//! URL's path depth, case and length (including the longest the model admits), how many
//! client IDs and thumbprints the record carries (from none to the published thumbprint
//! bound) and its tag count — so no single request is the specification.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::open_id_connect_providers::{
    client_ids, expected_arn, issuer_of, list_members, tagging_absence_message, thumbprint,
};
use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::saml_provider_listing::split_element;
use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// "IAM lets you have up to five thumbprints for an OIDC provider"
/// (`CreateOpenIDConnectProvider$ThumbprintList`) — the bound the library states, so the
/// table holds a row carrying that many.
const MAX_THUMBPRINTS: usize = state::MAX_THUMBPRINTS_PER_OPEN_ID_CONNECT_PROVIDER;

/// How many characters a provider URL may carry: `OpenIDConnectProviderUrlType` is len
/// 1..255 (`smithy_query(query="shape", shape="OpenIDConnectProviderUrlType")`), so the
/// table holds one at the published upper bound.
const MAX_PROVIDER_URL_CHARACTERS: usize = 255;

/// One provider the account is made to hold, with the members the row exercises.
struct DeletionCase {
    /// `Url` — "the URL must begin with https://" (`CreateOpenIDConnectProvider$Url`),
    /// len 1..255. The provider is NAMED by it: its ARN is derived from the URL.
    url: String,
    /// `ClientIDList`, in the order the create carries it.
    client_ids: Vec<String>,
    /// `ThumbprintList`. Each entry is exactly 40 characters, the length `thumbprintType`
    /// declares (len 40..40).
    thumbprints: Vec<String>,
    /// The tags the create carries, in request order.
    tags: Vec<(&'static str, &'static str)>,
}

impl DeletionCase {
    fn params(&self) -> Vec<(String, String)> {
        let mut params = vec![param("Url", &self.url)];
        for (index, client_id) in self.client_ids.iter().enumerate() {
            params.push(param(
                &format!("ClientIDList.member.{}", index + 1),
                client_id,
            ));
        }
        for (index, value) in self.thumbprints.iter().enumerate() {
            params.push(param(
                &format!("ThumbprintList.member.{}", index + 1),
                value,
            ));
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("Tags.member.{position}.Key"), key));
            params.push(param(&format!("Tags.member.{position}.Value"), value));
        }
        params
    }

    fn expected_tags(&self) -> BTreeMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }
}

/// The longest URL the model admits: `https://` plus a host filling the remaining
/// characters of `OpenIDConnectProviderUrlType`.
fn longest_url() -> String {
    let suffix = ".example.com";
    let host = "d".repeat(MAX_PROVIDER_URL_CHARACTERS - "https://".len() - suffix.len());
    format!("https://{host}{suffix}")
}

/// The providers this suite's rows create, in creation order: a bare host carrying nothing
/// at all, one path segment with one of everything, a nested path, a mixed-case host whose
/// client IDs carry a dot and a space, a hyphen-and-digit host at the published thumbprint
/// bound, and the longest URL the model admits.
fn deletion_cases() -> Vec<DeletionCase> {
    vec![
        DeletionCase {
            url: "https://server.example.org".to_string(),
            client_ids: Vec::new(),
            thumbprints: Vec::new(),
            tags: Vec::new(),
        },
        DeletionCase {
            url: "https://example.com/oidc".to_string(),
            client_ids: client_ids(1),
            thumbprints: vec![thumbprint(1)],
            tags: vec![("env", "dev")],
        },
        DeletionCase {
            url: "https://oidc.eks.us-east-1.amazonaws.com/id/EXAMPLED539D4633E53DE1B716D3041E"
                .to_string(),
            client_ids: client_ids(3),
            thumbprints: vec![thumbprint(2), thumbprint(3)],
            tags: vec![("team", "platform"), ("env", "prod")],
        },
        DeletionCase {
            url: "https://Accounts.Example.COM/Realms/Main".to_string(),
            client_ids: vec!["one.client".to_string(), "another client".to_string()],
            thumbprints: vec![thumbprint(4)],
            tags: vec![("zeta", "last"), ("alpha", "first"), ("mu", "middle")],
        },
        DeletionCase {
            url: "https://idp-7.example-corp.net/auth/realms/dev-2".to_string(),
            client_ids: client_ids(2),
            thumbprints: (5..5 + MAX_THUMBPRINTS as u32).map(thumbprint).collect(),
            tags: Vec::new(),
        },
        DeletionCase {
            url: longest_url(),
            client_ids: Vec::new(),
            thumbprints: vec![thumbprint(11)],
            tags: vec![("length", "published-maximum")],
        },
    ]
}

/// Create one fixture provider, answering the ARN the create reports.
async fn create_provider(router: &MultiServiceRouter, case: &DeletionCase) -> String {
    let params = case.params();
    let (status, body) = call(router, "CreateOpenIDConnectProvider", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {:?}: the fixture CreateOpenIDConnectProvider must succeed, got {status}: {body}",
        case.url
    );
    let arn = element(&body, "OpenIDConnectProviderArn");
    assert_eq!(
        arn,
        expected_arn(&case.url),
        "case {:?}: the create must hold the provider under the ARN derived from its URL, got \
         {arn:?}",
        case.url
    );
    arn
}

/// One `DeleteOpenIDConnectProvider` call.
async fn delete_provider(router: &MultiServiceRouter, arn: &str) -> (u16, String) {
    let params = vec![param("OpenIDConnectProviderArn", arn)];
    call(router, "DeleteOpenIDConnectProvider", &borrowed(&params)).await
}

/// One `GetOpenIDConnectProvider` call.
async fn read_provider(router: &MultiServiceRouter, arn: &str) -> (u16, String) {
    let params = vec![param("OpenIDConnectProviderArn", arn)];
    call(router, "GetOpenIDConnectProvider", &borrowed(&params)).await
}

/// One `ListOpenIDConnectProviderTags` call.
async fn read_tags(router: &MultiServiceRouter, arn: &str) -> (u16, String) {
    let params = vec![param("OpenIDConnectProviderArn", arn)];
    call(
        router,
        "ListOpenIDConnectProviderTags",
        &borrowed(&params),
    )
    .await
}

/// The ARNs `ListOpenIDConnectProviders` reports, sorted, so the set is independent of the
/// order the creates and deletes arrived in.
///
/// A list entry is a STRUCTURE (`OpenIDConnectProviderListEntry` declares the ARN alone),
/// so the shared leaf scan cannot read it: this narrows to the list and then to each entry.
async fn held_arns(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, "ListOpenIDConnectProviders", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from ListOpenIDConnectProviders, got {status} with \
         body {body}"
    );
    let mut arns = Vec::new();
    if let Some((content, _)) = split_element(&body, "OpenIDConnectProviderList") {
        let mut rest = content;
        while let Some((entry, remainder)) = split_element(rest, "member") {
            arns.push(element(entry, "Arn"));
            rest = remainder;
        }
    }
    arns.sort();
    arns
}

/// Assert the provider the ARN names still reports the record `case` created, member for
/// member.
async fn assert_reads_back(router: &MultiServiceRouter, case: &DeletionCase, arn: &str) {
    let (status, read) = read_provider(router, arn).await;
    assert_eq!(
        status, 200,
        "case {:?}: the held provider must still read back, got {status}: {read}",
        case.url
    );
    let reported_url = element(&read, "Url");
    assert_eq!(
        reported_url,
        issuer_of(&case.url),
        "case {:?}: deleting another provider must leave the held URL alone, got \
         {reported_url:?}",
        case.url
    );
    let reported_client_ids = list_members(&read, "ClientIDList");
    assert_eq!(
        reported_client_ids, case.client_ids,
        "case {:?}: deleting another provider must leave the held client IDs alone, got \
         {reported_client_ids:?}",
        case.url
    );
    let reported_thumbprints = list_members(&read, "ThumbprintList");
    assert_eq!(
        reported_thumbprints, case.thumbprints,
        "case {:?}: deleting another provider must leave the held thumbprints alone, got \
         {reported_thumbprints:?}",
        case.url
    );
    let tags = reported_tags(&read);
    assert_eq!(
        tags,
        case.expected_tags(),
        "case {:?}: deleting another provider must leave the held tags alone, got {tags:?}",
        case.url
    );
}

// ---------------------------------------------------------------------------
// The named provider, and nothing else
// ---------------------------------------------------------------------------

/// A delete forgets the provider the ARN names: afterwards the read reports it absent
/// (`NoSuchEntity`, HTTP 404), the tags held on that record are gone with it, the listing no
/// longer carries it — and every provider not yet deleted reads back exactly as its create
/// wrote it. The providers are deleted in an order that is neither the creation order nor
/// its reverse, so the rule cannot depend on forgetting the newest or the oldest record.
#[tokio::test]
async fn deleting_a_provider_forgets_that_provider_and_leaves_every_other_one() {
    let router = router();
    let cases = deletion_cases();
    let mut arns: Vec<String> = Vec::new();
    for case in &cases {
        arns.push(create_provider(&router, case).await);
    }

    let mut expected = arns.clone();
    expected.sort();
    assert_eq!(
        held_arns(&router, "every provider created").await,
        expected,
        "the account must hold every created provider before any delete"
    );

    for index in [2usize, 0, 5, 1, 4, 3] {
        let case = &cases[index];
        let arn = &arns[index];

        let (status, body) = delete_provider(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from DeleteOpenIDConnectProvider, got {status} with \
             body {body}",
            case.url
        );

        let (status, read) = read_provider(&router, arn).await;
        assert_eq!(
            status, 404,
            "case {:?}: a deleted provider must not read back, got {status}: {read}",
            case.url
        );
        assert_eq!(
            error_code(&read),
            "NoSuchEntity",
            "case {:?}: expected NoSuchEntity from GetOpenIDConnectProvider, got {read}",
            case.url
        );

        // The tags went with the record: the provider they hung off is gone, so the tag
        // read is the same "entity absent" answer, whatever the record held.
        let (status, tags) = read_tags(&router, arn).await;
        assert_eq!(
            status, 404,
            "case {:?}: a deleted provider's tags must not read back, got {status}: {tags}",
            case.url
        );
        assert_eq!(
            error_code(&tags),
            "NoSuchEntity",
            "case {:?}: expected NoSuchEntity from ListOpenIDConnectProviderTags, got {tags}",
            case.url
        );

        expected.retain(|held| held != arn);
        let listed = held_arns(&router, &case.url).await;
        assert_eq!(
            listed, expected,
            "case {:?}: the listing must report exactly the providers still held, got \
             {listed:?}",
            case.url
        );

        for (other_index, other) in cases.iter().enumerate() {
            if expected.contains(&arns[other_index]) {
                assert_reads_back(&router, other, &arns[other_index]).await;
            }
        }
    }

    let listed = held_arns(&router, "every provider deleted").await;
    assert!(
        listed.is_empty(),
        "an account whose every provider was deleted must hold none, got {listed:?}"
    );
}

/// Every call that resolves a provider by its ARN reports a deleted one absent — the record
/// and everything held on it are forgotten, so no read reports it and no mutation finds it
/// to change.
#[tokio::test]
async fn every_call_on_a_deleted_provider_reports_it_absent() {
    let router = router();

    for case in deletion_cases() {
        let arn = create_provider(&router, &case).await;
        let (status, body) = delete_provider(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the delete must be served, got {status} with body {body}",
            case.url
        );

        // One row per operation that addresses a provider by its ARN, with the members its
        // request requires beside it.
        let requests: Vec<(&str, Vec<(String, String)>)> = vec![
            ("GetOpenIDConnectProvider", Vec::new()),
            ("ListOpenIDConnectProviderTags", Vec::new()),
            (
                "AddClientIDToOpenIDConnectProvider",
                vec![param("ClientID", "audience-z")],
            ),
            (
                "RemoveClientIDFromOpenIDConnectProvider",
                vec![param("ClientID", "audience-z")],
            ),
            (
                "UpdateOpenIDConnectProviderThumbprint",
                vec![param("ThumbprintList.member.1", &thumbprint(42))],
            ),
            (
                "TagOpenIDConnectProvider",
                vec![
                    param("Tags.member.1.Key", "env"),
                    param("Tags.member.1.Value", "dev"),
                ],
            ),
            (
                "UntagOpenIDConnectProvider",
                vec![param("TagKeys.member.1", "env")],
            ),
            ("DeleteOpenIDConnectProvider", Vec::new()),
        ];

        for (action, extra) in requests {
            let mut params = vec![param("OpenIDConnectProviderArn", &arn)];
            params.extend(extra);
            let (status, body) = call(&router, action, &borrowed(&params)).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (404, "NoSuchEntity"),
                "case {:?}: {action} on a deleted provider expected (404, NoSuchEntity), got \
                 ({status}, {body})",
                case.url
            );
        }
    }
}

/// A deleted URL is FREE: a later create for it is served and mints a NEW record, which
/// reports the client IDs, thumbprints and tags THAT create carried — never anything the
/// deleted provider held.
#[tokio::test]
async fn a_deleted_url_can_be_registered_again_as_a_new_record() {
    let router = router();

    for case in deletion_cases() {
        let arn = create_provider(&router, &case).await;
        let (status, body) = delete_provider(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the delete must be served, got {status} with body {body}",
            case.url
        );

        // The second create carries DIFFERENT client IDs, thumbprints and tags from the
        // deleted provider's, so anything that survived the delete would be visible in the
        // read below.
        let recreated = DeletionCase {
            url: case.url.clone(),
            client_ids: vec!["registered-again".to_string()],
            thumbprints: vec![thumbprint(31), thumbprint(32)],
            tags: vec![("recreated", "yes")],
        };
        let new_arn = create_provider(&router, &recreated).await;
        assert_eq!(
            new_arn, arn,
            "case {:?}: the ARN is derived from the URL, so re-registering it answers the same \
             ARN, got {new_arn:?}",
            case.url
        );
        assert_reads_back(&router, &recreated, &new_arn).await;

        // Leave the account empty for the next row.
        let (status, body) = delete_provider(&router, &new_arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: the re-registered provider must delete too, got {status}: {body}",
            case.url
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// RULE: an ARN the account holds no provider under is refused as absent (`NoSuchEntity`,
/// HTTP 404) — which a second delete of the same ARN is a case of — while a value that is
/// not an ARN is refused as invalid (`InvalidInput`, HTTP 400). A refused delete forgets
/// nothing.
#[tokio::test]
async fn an_absent_provider_is_not_found_and_a_non_arn_is_invalid() {
    let router = router();
    let cases = deletion_cases();
    let held_case = &cases[2];
    let held = create_provider(&router, held_case).await;
    let doomed = create_provider(&router, &cases[1]).await;

    let (status, body) = delete_provider(&router, &doomed).await;
    assert_eq!(
        status, 200,
        "the first delete must be served, got {status} with body {body}"
    );

    // Rows: the same provider twice, a URL nothing was ever registered under, a near-miss
    // of the held one, an ARN of another IAM resource type, the held URL in another
    // account, and an ARN-shaped value carrying the characters XML gives meaning to — so
    // the refusal's body must still parse.
    for (what, arn) in [
        ("the same provider twice", doomed.clone()),
        (
            "a URL no create used",
            expected_arn("https://never.example.org"),
        ),
        (
            "a near-miss of a held URL",
            format!("{held}/extra-path-segment"),
        ),
        (
            "another resource type",
            format!("arn:aws:iam::{}:role/{}", account_id(), "server.example.org"),
        ),
        (
            "another account",
            format!(
                "arn:aws:iam::111122223333:oidc-provider/{}",
                issuer_of(&held_case.url)
            ),
        ),
        (
            "a value carrying XML metacharacters",
            format!(
                "arn:aws:iam::{}:oidc-provider/a&b<c>d\"e'f.example.org",
                account_id()
            ),
        ),
    ] {
        let (status, body) = delete_provider(&router, &arn).await;
        assert_eq!(
            status, 404,
            "case {what:?}: expected HTTP 404, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {what:?}: expected NoSuchEntity, got {body}"
        );
        // "The error message describes the resource" (`NoSuchEntityException`): the
        // resource a delete names is the provider ARN the request carried, worded the way
        // this family words a provider it cannot find.
        let message = element(&body, "Message");
        assert_eq!(
            message,
            tagging_absence_message(&arn),
            "case {what:?}: the refusal must describe the ARN the request named, got \
             {message:?}"
        );
    }

    // Values past `arnType`'s published 20-character minimum, so the HANDLER is what
    // refuses them rather than the boundary validator: a URL, a bare name, and a resource
    // portion with the `arn:` prefix missing.
    for value in [
        "https://example.com/oidc",
        "example.com-oidc-provider",
        "aws:iam::000000000000:oidc-provider/example.com/oidc",
    ] {
        let (status, body) = delete_provider(&router, value).await;
        assert_eq!(
            status, 400,
            "case {value:?}: a value that is not an ARN must be refused with HTTP 400, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {value:?}: expected InvalidInput, got {body}"
        );
    }

    // A value SHORTER than `arnType`'s published 20-character minimum is refused by the
    // modelled constraint, before the operation runs at all.
    let (status, body) = delete_provider(&router, "example.com").await;
    assert_eq!(
        status, 400,
        "a value shorter than arnType's minimum must be refused with HTTP 400, got {status} \
         with body {body}"
    );

    // Nothing was forgotten by any refused call: the provider the account still holds reads
    // back as its create wrote it, and it is the only one listed.
    assert_reads_back(&router, held_case, &held).await;
    assert_eq!(
        held_arns(&router, "after every refusal").await,
        vec![held],
        "a refused delete must forget nothing"
    );
}

// ---------------------------------------------------------------------------
// Roles that trust the provider
// ---------------------------------------------------------------------------

/// "Deleting an IAM OIDC provider resource does not update any roles that reference the
/// provider as a principal in their trust policies": the role keeps the document it was
/// created with, naming the ARN of a provider the account no longer holds.
#[tokio::test]
async fn deleting_a_provider_leaves_a_role_that_trusts_it_alone() {
    let router = router();

    for (index, case) in deletion_cases().into_iter().enumerate() {
        let arn = create_provider(&router, &case).await;
        let trust_policy = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\"Principal\":\
             {{\"Federated\":\"{arn}\"}},\"Action\":\"sts:AssumeRoleWithWebIdentity\",\
             \"Condition\":{{\"StringEquals\":{{\"{}:aud\":\"audience-0\"}}}}}}]}}",
            issuer_of(&case.url)
        );
        let role_name = format!("WebIdentityRole{index}");
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role_name.as_str()),
                ("AssumeRolePolicyDocument", trust_policy.as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: the fixture CreateRole must succeed, got {status}: {body}"
        );

        let (status, body) = delete_provider(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: the delete must be served, got {status} with body {body}"
        );

        let (status, role) = call(&router, "GetRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {role_name:?}: the role must still read back, got {status}: {role}"
        );
        assert_eq!(
            crate::user_lifecycle::document_element(&role, "AssumeRolePolicyDocument"),
            trust_policy,
            "case {role_name:?}: the role must keep the trust policy naming the deleted \
             provider, got {role}"
        );
    }
}
