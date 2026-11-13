// SPDX-License-Identifier: CC0-1.0

//! The ONE form every IAM response carries a policy document in: percent-encoded, over the
//! awsQuery wire, on single-resource reads and on every page of a paginated listing alike.
//!
//! IAM holds the document it was given and reports it URL-encoded: "the policy document
//! returned in this structure is URL-encoded compliant with RFC 3986. You can use a URL
//! decoding method to convert the policy back to plain JSON text"
//! (`smithy_query(query="doc",shape="PolicyVersion",member="Document")`), stated again by
//! every operation that serves one. The rule is about the MEMBER — every response member
//! whose target is `policyDocumentType` — so this suite asserts it wherever one is written
//! out: a role read on its own, a page of roles, the roles embedded in an instance profile
//! on EVERY page of the two instance-profile listings, the inline policies of a role, a
//! user and a group, and a managed policy's version.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour of documents that were PERSISTED by the write operations, never a handler
//! called directly. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! characters the document carries (the punctuation JSON is made of, the spaces and tabs a
//! caller indents with, the `+` and `%` a decode of UNencoded text corrupts, the slash of an
//! ARN, the unreserved characters that are reported as they stand), the page size the
//! listing is walked with, and which operation serves the document. The DECODE every row
//! checks is the runtime's URI decoder, not this crate's encoder, so a document encoded
//! wrongly cannot still match.

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{call, element, elements, router};

/// One document the fixture stores, and what makes it worth storing.
struct DocumentCase {
    case: &'static str,
    /// The text the request carries.
    submitted: &'static str,
    /// The text the account HOLDS for it, which is what every read reports (encoded).
    ///
    /// For a role's trust policy that is the document RENDERED — the whitespace between its
    /// JSON tokens gone, a one-element string-or-array element written as its string, an
    /// account principal written as that account's root ARN (`crate::state::trust_policy`) —
    /// and for an inline or managed policy document it is the submitted text, which those
    /// stores keep.
    stored: &'static str,
}

/// The trust policy this suite pins the exact encoding of: the one a caller writes for an
/// instance profile's role, compact.
const CANONICAL_TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// [`CANONICAL_TRUST_POLICY`] percent-encoded, written out here character by character
/// rather than computed, so this suite pins the TRANSFORM and not whatever the service
/// happens to do: every character outside RFC 3986's unreserved set (the ASCII letters and
/// digits, `-`, `.`, `_` and `~`) is the `%XX` of its byte, and `2012-10-17` and
/// `ec2.amazonaws.com` carry only unreserved characters and so stand as they are.
const CANONICAL_TRUST_POLICY_ENCODED: &str = "%7B%22Version%22%3A%222012-10-17%22%2C%22Statement%22%3A%5B%7B%22Effect%22%3A%22Allow%22%2C%22Principal%22%3A%7B%22Service%22%3A%22ec2.amazonaws.com%22%7D%2C%22Action%22%3A%22sts%3AAssumeRole%22%7D%5D%7D";

/// The trust policies the fixture stores, one per role.
///
/// `policyDocumentType` is `@pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$`
/// (`smithy_query(query="shape",shape="policyDocumentType")`), so the tab, the line feed and
/// the carriage return are part of what a caller may submit and therefore part of what a
/// report has to carry back.
fn trust_policies() -> Vec<DocumentCase> {
    vec![
        DocumentCase {
            case: "the compact trust policy of an instance profile's role",
            submitted: CANONICAL_TRUST_POLICY,
            stored: CANONICAL_TRUST_POLICY,
        },
        DocumentCase {
            case: "the spaces an SDK's JSON serializer writes between the tokens",
            submitted: r#"{"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"Service": "ec2.amazonaws.com"}, "Action": "sts:AssumeRole"}]}"#,
            stored: CANONICAL_TRUST_POLICY,
        },
        DocumentCase {
            case: "a condition value carrying a literal plus, which a decode of raw text \
                   would turn into a space",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole","Condition":{"StringEquals":{"sts:ExternalId":"a+b/c=="}}}]}"#,
            stored: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole","Condition":{"StringEquals":{"sts:ExternalId":"a+b/c=="}}}]}"#,
        },
        DocumentCase {
            case: "a condition value carrying a literal percent, which a decode of raw \
                   text would read as an escape",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole","Condition":{"StringLike":{"sts:ExternalId":"100%7Bdone"}}}]}"#,
            stored: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole","Condition":{"StringLike":{"sts:ExternalId":"100%7Bdone"}}}]}"#,
        },
        DocumentCase {
            case: "a document indented with the tab and line feed the member admits",
            submitted: "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Effect\": \"Allow\",\n\t\t\t\"Principal\": {\"Service\": \"ec2.amazonaws.com\"},\n\t\t\t\"Action\": \"sts:AssumeRole\"\n\t\t}\n\t]\n}",
            stored: CANONICAL_TRUST_POLICY,
        },
        DocumentCase {
            case: "a principal ARN, whose slashes and colons are not unreserved",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::210987654321:role/deploy~1_x.y-z"},"Action":"sts:AssumeRole"}]}"#,
            stored: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::210987654321:role/deploy~1_x.y-z"},"Action":"sts:AssumeRole"}]}"#,
        },
    ]
}

/// The identity-policy documents the fixture embeds and publishes: an identity statement
/// "must include … the Action element" and "the Resource element" (IAM's policy grammar), so
/// each row carries both, and the rows differ in the characters the values carry.
fn identity_policies() -> Vec<DocumentCase> {
    vec![
        DocumentCase {
            case: "a wildcard action and resource",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#,
            stored: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#,
        },
        DocumentCase {
            case: "an object key and a bucket ARN carrying slashes and a plus",
            submitted: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::reports/2026+Q1/*"}]}"#,
            stored: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::reports/2026+Q1/*"}]}"#,
        },
        DocumentCase {
            case: "the spaces and newlines a caller indents an inline policy with",
            submitted: "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [{\"Effect\": \"Deny\", \"Action\": \"iam:*\", \"Resource\": \"*\"}]\n}",
            stored: "{\n  \"Version\": \"2012-10-17\",\n  \"Statement\": [{\"Effect\": \"Deny\", \"Action\": \"iam:*\", \"Resource\": \"*\"}]\n}",
        },
    ]
}

/// The characters RFC 3986 leaves unencoded: "unreserved characters" are the ASCII letters
/// and digits plus `-`, `.`, `_` and `~`.
fn is_unreserved(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | '_' | '~')
}

/// Assert that `reported` is a percent-ENCODED document: every character of it is either
/// unreserved or part of a `%XX` escape, so nothing a policy document is punctuated with
/// (`{`, `"`, `:`, `,`, `[`, `]`, `*`, `/`, a space, a tab) reaches the caller as itself.
fn assert_percent_encoded(case: &str, reported: &str) {
    let mut characters = reported.chars();
    while let Some(character) = characters.next() {
        if is_unreserved(character) {
            continue;
        }
        assert_eq!(
            character, '%',
            "case {case:?}: expected every character outside the unreserved set to be \
             percent-encoded, got {character:?} in {reported:?}"
        );
        let escape: String = characters.by_ref().take(2).collect();
        assert!(
            escape.len() == 2 && escape.chars().all(|digit| digit.is_ascii_hexdigit()),
            "case {case:?}: expected two hexadecimal digits after a '%', got {escape:?} in \
             {reported:?}"
        );
    }
}

/// Assert that the `tag` members of `body` are each the stored document, REPORTED: the
/// percent-encoded form, which decodes back to `stored` byte for byte.
///
/// The decode is what IAM's own contract tells a caller to do, and it is the half that makes
/// this more than a shape check: a document carrying a literal `+` or `%` only survives it
/// when the member really was encoded.
fn assert_reported_documents(case: &str, body: &str, tag: &str, stored: &str, expected_count: usize) {
    let reported = elements(body, tag);
    assert_eq!(
        reported.len(),
        expected_count,
        "case {case:?}: expected {expected_count} <{tag}> member(s), got {reported:?} in {body}"
    );
    for value in &reported {
        assert_percent_encoded(case, value);
        let decoded = ::smithy_server_runtime::percent_decode(value);
        assert_eq!(
            decoded, stored,
            "case {case:?}: decoding <{tag}> must return the stored document, got {decoded:?} \
             from {value:?}"
        );
    }
}

/// Create a role whose trust policy is `submitted`, asserting the fixture create succeeded
/// and that the create's own answer already reports the STORED document, encoded.
async fn create_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    submitted: &str,
    stored: &str,
) {
    let (status, body) = call(
        router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", submitted)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateRole must succeed, got {status}: {body}"
    );
    assert_reported_documents(case, &body, "AssumeRolePolicyDocument", stored, 1);
}

/// The fixture of the paginated suite: one role whose trust policy is
/// [`CANONICAL_TRUST_POLICY`], and `profiles` instance profiles all holding it.
async fn with_bound_profiles(role: &str, profiles: &[&str]) -> MultiServiceRouter {
    let router = router();
    create_role(
        &router,
        "fixture",
        role,
        CANONICAL_TRUST_POLICY,
        CANONICAL_TRUST_POLICY,
    )
    .await;
    for profile in profiles {
        let (status, body) = call(
            &router,
            "CreateInstanceProfile",
            &[("InstanceProfileName", profile), ("Path", "/probe/")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {profile:?}: the fixture CreateInstanceProfile must succeed, got {status}: \
             {body}"
        );
        let (status, body) = call(
            &router,
            "AddRoleToInstanceProfile",
            &[("InstanceProfileName", profile), ("RoleName", role)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {profile:?}: the fixture AddRoleToInstanceProfile must succeed, got \
             {status}: {body}"
        );
    }
    router
}

/// RULE: every page of an instance-profile listing reports the trust policy of every role it
/// embeds percent-encoded — the same form the single-resource reads report — so a caller
/// that walks the collection with `Marker`/`MaxItems` reads the documents it would read one
/// profile at a time.
#[tokio::test]
async fn every_page_of_the_instance_profile_listings_reports_the_encoded_trust_policy() {
    const ROLE: &str = "probe-role";
    let profiles = ["probe-0", "probe-1", "probe-2", "probe-3"];
    let router = with_bound_profiles(ROLE, &profiles).await;

    // Rows: the page size the walk uses — one profile per page (so every page is a
    // follow-up page reached through a `Marker`), two, exactly the collection, past it, and
    // no `MaxItems` at all.
    let page_sizes: &[Option<usize>] = &[Some(1), Some(2), Some(profiles.len()), Some(1000), None];
    for listing in ["ListInstanceProfiles", "ListInstanceProfilesForRole"] {
        for page_size in page_sizes {
            let case = format!("{listing} with MaxItems={page_size:?}");
            let mut marker: Option<String> = None;
            let mut seen = 0;
            let mut pages = 0;
            loop {
                let mut params: Vec<(String, String)> = Vec::new();
                if listing == "ListInstanceProfilesForRole" {
                    params.push(param("RoleName", ROLE));
                }
                if let Some(size) = page_size {
                    params.push(param("MaxItems", &size.to_string()));
                }
                if let Some(token) = &marker {
                    params.push(param("Marker", token));
                }
                let (status, body) = call(&router, listing, &borrowed(&params)).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status}: {body}"
                );

                let on_this_page = elements(&body, "InstanceProfileName").len();
                assert_reported_documents(
                    &case,
                    &body,
                    "AssumeRolePolicyDocument",
                    CANONICAL_TRUST_POLICY,
                    on_this_page,
                );
                // And the exact text, pinned: the transform is the requirement, so one page
                // asserts the whole encoded string rather than only its invariants.
                for value in elements(&body, "AssumeRolePolicyDocument") {
                    assert_eq!(
                        value, CANONICAL_TRUST_POLICY_ENCODED,
                        "case {case:?}: expected the percent-encoded trust policy, got {value:?}"
                    );
                }
                seen += on_this_page;
                pages += 1;
                assert!(
                    pages <= profiles.len() + 1,
                    "case {case:?}: the pagination never ended, saw {seen} profiles"
                );
                if element(&body, "IsTruncated") == "true" {
                    marker = Some(element(&body, "Marker"));
                } else {
                    break;
                }
            }
            assert_eq!(
                seen,
                profiles.len(),
                "case {case:?}: expected the walk to report every bound profile"
            );
        }
    }

    // The single-resource reads of the same records: the profile, the role, and a page of
    // roles all report that one encoded document.
    for (case, operation, params) in [
        (
            "GetInstanceProfile",
            "GetInstanceProfile",
            vec![param("InstanceProfileName", profiles[0])],
        ),
        ("GetRole", "GetRole", vec![param("RoleName", ROLE)]),
        ("ListRoles", "ListRoles", vec![param("MaxItems", "1")]),
    ] {
        let (status, body) = call(&router, operation, &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert_reported_documents(case, &body, "AssumeRolePolicyDocument", CANONICAL_TRUST_POLICY, 1);
        assert_eq!(
            element(&body, "AssumeRolePolicyDocument"),
            CANONICAL_TRUST_POLICY_ENCODED,
            "case {case:?}: expected the same encoded text the listing pages report"
        );
    }
}

/// RULE: the encoding is a transform of the stored bytes, so a document is reported encoded
/// whatever characters it carries, and percent-decoding the report returns the text that was
/// submitted — including the `+` and `%` that a decode of UNencoded text would corrupt.
#[tokio::test]
async fn a_trust_policy_is_reported_encoded_whatever_characters_it_carries() {
    let router = router();
    let documents = trust_policies();
    for (index, row) in documents.iter().enumerate() {
        let role = format!("trust-{index}");
        create_role(&router, row.case, &role, row.submitted, row.stored).await;
        let profile = format!("trust-profile-{index}");
        let (status, body) = call(
            &router,
            "CreateInstanceProfile",
            &[("InstanceProfileName", &profile)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture CreateInstanceProfile must succeed, got {status}: {body}",
            row.case
        );
        let (status, body) = call(
            &router,
            "AddRoleToInstanceProfile",
            &[("InstanceProfileName", &profile), ("RoleName", &role)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture AddRoleToInstanceProfile must succeed, got {status}: {body}",
            row.case
        );

        // Every reader of that role's trust policy: the role itself, a page of the role's
        // profiles, the profile, and the account-wide report.
        for (reader, params) in [
            ("GetRole", vec![param("RoleName", &role)]),
            (
                "GetInstanceProfile",
                vec![param("InstanceProfileName", &profile)],
            ),
            (
                "ListInstanceProfilesForRole",
                vec![param("RoleName", &role), param("MaxItems", "1")],
            ),
        ] {
            let case = format!("{} through {reader}", row.case);
            let (status, body) = call(&router, reader, &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {body}"
            );
            assert_reported_documents(&case, &body, "AssumeRolePolicyDocument", row.stored, 1);
        }
    }

    // One `ListRoles` page carries every fixture role, each reporting the document its own
    // role holds. Several rows render to the SAME stored text (the compact, the spaced and the
    // indented trust policy are one document once rendered), so what the page must carry is
    // that stored text, encoded.
    let (status, body) = call(&router, "ListRoles", &[("MaxItems", "1000")]).await;
    assert_eq!(status, 200, "ListRoles must succeed, got {status}: {body}");
    let listed = elements(&body, "AssumeRolePolicyDocument");
    for row in &documents {
        let reported = listed
            .iter()
            .any(|value| ::smithy_server_runtime::percent_decode(value) == row.stored);
        assert!(
            reported,
            "case {:?}: expected the listing to report {:?} encoded, got {listed:?}",
            row.case, row.stored
        );
    }
    for value in &listed {
        assert_percent_encoded("a ListRoles page", value);
    }
}

/// RULE: the same transform applies to every other member that carries a policy document —
/// an inline policy embedded in a role, a user or a group, and a managed policy's version —
/// so one decode reads any document IAM reports.
#[tokio::test]
async fn every_reader_of_an_inline_or_managed_policy_document_reports_it_encoded() {
    let router = router();
    create_role(
        &router,
        "fixture",
        "doc-role",
        CANONICAL_TRUST_POLICY,
        CANONICAL_TRUST_POLICY,
    )
    .await;
    for (operation, params) in [
        ("CreateUser", vec![param("UserName", "doc-user")]),
        ("CreateGroup", vec![param("GroupName", "doc-group")]),
    ] {
        let (status, body) = call(&router, operation, &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "the fixture {operation} must succeed, got {status}: {body}"
        );
    }

    for (index, row) in identity_policies().iter().enumerate() {
        let name = format!("Embedded{index}");
        // The three entities an inline policy is embedded in, each written and read back
        // through its own pair of operations.
        for (entity, put, get, key, value) in [
            (
                "role",
                "PutRolePolicy",
                "GetRolePolicy",
                "RoleName",
                "doc-role",
            ),
            (
                "user",
                "PutUserPolicy",
                "GetUserPolicy",
                "UserName",
                "doc-user",
            ),
            (
                "group",
                "PutGroupPolicy",
                "GetGroupPolicy",
                "GroupName",
                "doc-group",
            ),
        ] {
            let case = format!("{} embedded in a {entity}", row.case);
            let (status, body) = call(
                &router,
                put,
                &[
                    (key, value),
                    ("PolicyName", &name),
                    ("PolicyDocument", row.submitted),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture {put} must succeed, got {status}: {body}"
            );
            let (status, body) =
                call(&router, get, &[(key, value), ("PolicyName", &name)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {get} 200, got {status}: {body}"
            );
            assert_reported_documents(&case, &body, "PolicyDocument", row.stored, 1);
        }

        // And the managed policy the same document is published as: its version carries the
        // document, encoded, where a reader serves one.
        let case = format!("{} published as a managed policy", row.case);
        let policy_name = format!("Managed{index}");
        let (status, body) = call(
            &router,
            "CreatePolicy",
            &[
                ("PolicyName", &policy_name),
                ("PolicyDocument", row.submitted),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture CreatePolicy must succeed, got {status}: {body}"
        );
        let arn = element(&body, "Arn");
        let (status, body) = call(
            &router,
            "GetPolicyVersion",
            &[("PolicyArn", &arn), ("VersionId", "v1")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetPolicyVersion 200, got {status}: {body}"
        );
        assert_reported_documents(&case, &body, "Document", row.stored, 1);
    }

    // The account-wide report carries the documents of every entity it reports, under the
    // members that carry them.
    let (status, body) = call(&router, "GetAccountAuthorizationDetails", &[]).await;
    assert_eq!(
        status, 200,
        "GetAccountAuthorizationDetails must succeed, got {status}: {body}"
    );
    for tag in ["AssumeRolePolicyDocument", "PolicyDocument", "Document"] {
        let reported = elements(&body, tag);
        assert!(
            !reported.is_empty(),
            "expected the report to carry <{tag}> members, got {body}"
        );
        for value in &reported {
            assert_percent_encoded(tag, value);
        }
    }
}
