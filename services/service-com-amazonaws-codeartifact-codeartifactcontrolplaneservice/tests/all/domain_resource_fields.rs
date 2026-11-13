// SPDX-License-Identifier: CC0-1.0

//! The DERIVED fields of a domain, of a resource policy and of an idempotent delete, over
//! the wire.
//!
//! Every case drives real `restJson1` requests through the `MultiServiceRouter` the
//! production emulator serves with, so what is pinned here is what a CodeArtifact client
//! reads — and, for the KMS key a domain is encrypted with, the KMS service is mounted
//! beside CodeArtifact on the SAME engine, so the key the domain reports is asked about
//! through the service that owns it.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one example:
//!
//! 1. `DomainDescription$s3BucketArn` is the ARN of the asset bucket of the ACCOUNT and
//!    REGION the domain lives in — `arn:aws:s3:::assets-<domain-owner>-<region>`, two
//!    suffix components and nothing per-domain or per-call — so every domain of one
//!    account and Region reports the same bucket and every read of one domain reports the
//!    same string.
//! 2. `DomainDescription$encryptionKey` is the ARN of the key the domain IS encrypted
//!    with: a create that names a key reports the key KMS resolves that reference to,
//!    whichever of the four spellings it used (key id, key ARN, alias name, alias ARN),
//!    and a create that names none reports the Amazon Web Services managed key of the
//!    account and Region, which KMS itself then answers for. Every later read of the
//!    domain reports that same key.
//! 3. `ResourcePolicy$revision` is the digest of the document STORED under it — base64 of
//!    its SHA-256 — so the same document always reports the same token, other bytes report
//!    another, and the token a caller read is the precondition that matches exactly while
//!    that document is still attached. One rule for both policy-bearing resources.
//! 4. `DeletePackageGroup` is `@idempotent` and its `packageGroup` output member is
//!    optional, so deleting a group the domain holds no record of has the delete's goal
//!    already met: HTTP 200 with an empty body, and nothing else in the domain changes.
//!    The ROOT group, which the domain is created with, is still refused.
//! 5. An `externalConnection` that names no registry this service offers is
//!    `ResourceNotFoundException` (404) worded the way this service words every other
//!    resource it looked for and did not find.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::external_connection::associate;
use crate::harness::{call, create_domain, external_connection_not_found, Answer, ACCOUNT, REGION};
use crate::package_group_hierarchy::{create_package_group, describe_package_group};
use crate::resource_tags::encode_query;

/// The root package group every domain is created with.
const ROOT_GROUP: &str = "/*";

/// The alias of the Amazon Web Services managed key CodeArtifact encrypts a domain with
/// when the create names no key. `aws/<service-name>` is the form of such an alias
/// (`service_com_amazonaws_kms_trentservice::state::is_predefined_aws_alias`), under this
/// service's own name.
const MANAGED_KEY_ALIAS: &str = "alias/aws/codeartifact";

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

/// A router with CodeArtifact AND KMS mounted over one shared engine — the assembly a case
/// about the key a domain is encrypted with needs, since that key is KMS's own resource.
fn emulator_with_kms() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        engine,
    ));
    router
}

/// Call one KMS operation: `awsJson1_1`, `POST /` with
/// `X-Amz-Target: TrentService.<Operation>`, signed for [`REGION`].
async fn kms(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("kms.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/kms/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("serialize the KMS request"),
        ))
        .expect("build the KMS request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// `GET /v1/domain?domain=`.
async fn describe_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(router, "GET", &format!("/v1/domain?domain={domain}"), None).await
}

/// `DELETE /v1/domain?domain=`.
async fn delete_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/domain?domain={domain}"),
        None,
    )
    .await
}

/// `POST /v1/domains` — the `DomainSummary` list, which carries the key too.
async fn list_domains(router: &MultiServiceRouter) -> Answer {
    call(router, "POST", "/v1/domains", Some(json!({}))).await
}

/// The `encryptionKey` the listing reports for one domain.
fn listed_key(listed: &Answer, domain: &str) -> Option<String> {
    listed.body["domains"]
        .as_array()?
        .iter()
        .find(|summary| summary["name"].as_str() == Some(domain))?
        .get("encryptionKey")?
        .as_str()
        .map(str::to_string)
}

/// `DELETE /v1/package-group?domain=&package-group=`, with the pattern encoded for the
/// query string it travels in.
async fn delete_package_group(
    router: &MultiServiceRouter,
    domain: &str,
    pattern: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!(
            "/v1/package-group?domain={domain}&package-group={}",
            encode_query(pattern)
        ),
        None,
    )
    .await
}

/// `PUT /v1/domain/permissions/policy`.
async fn put_domain_policy(
    router: &MultiServiceRouter,
    domain: &str,
    document: &str,
    revision: Option<&str>,
) -> Answer {
    let mut body = json!({ "domain": domain, "policyDocument": document });
    if let Some(revision) = revision {
        body["policyRevision"] = json!(revision);
    }
    call(router, "PUT", "/v1/domain/permissions/policy", Some(body)).await
}

/// `GET /v1/domain/permissions/policy?domain=`.
async fn get_domain_policy(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/domain/permissions/policy?domain={domain}"),
        None,
    )
    .await
}

/// `PUT /v1/repository/permissions/policy?domain=&repository=`.
async fn put_repository_policy(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
    document: &str,
    revision: Option<&str>,
) -> Answer {
    let mut body = json!({ "policyDocument": document });
    if let Some(revision) = revision {
        body["policyRevision"] = json!(revision);
    }
    call(
        router,
        "PUT",
        &format!("/v1/repository/permissions/policy?domain={domain}&repository={repository}"),
        Some(body),
    )
    .await
}

/// `GET /v1/repository/permissions/policy?domain=&repository=`.
async fn get_repository_policy(
    router: &MultiServiceRouter,
    domain: &str,
    repository: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/repository/permissions/policy?domain={domain}&repository={repository}"),
        None,
    )
    .await
}

/// The revision a policy document must carry: base64 (standard alphabet) of its SHA-256,
/// computed here from the document the ANSWER reports as stored — so the assertion is the
/// rule, not a copy of what the service did.
fn digest_revision(stored_document: &str) -> String {
    use base64::Engine as _;
    use sha2::Digest as _;

    base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(stored_document))
}

/// The `policy` member of an answer.
fn policy(answer: &Answer) -> &Value {
    &answer.body["policy"]
}

// ---------------------------------------------------------------------------
// RULE 1: the asset bucket is the account's and Region's, not the domain's
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_asset_bucket_arn_is_the_account_and_region_bucket() {
    let router = emulator_with_kms();

    // The bucket ARN of the account the domains are created in, in the Region they are
    // signed for: `assets-<domain-owner>-<region>`, with an S3 ARN's empty region and
    // account segments. Composed here from the owner and the Region the responses
    // themselves report, so the row below asserts the RULE and not a literal.
    let expected = format!("arn:aws:s3:::assets-{ACCOUNT}-{REGION}");

    // Rows differ along what a per-domain derivation would have varied over: the name's
    // length (the modelled extremes of `DomainName`, len 2..50), its character classes,
    // and whether the create named an encryption key at all.
    let longest = format!("d{}e", "x".repeat(48));
    let rows: Vec<(&str, String, Option<&str>)> = vec![
        ("the shortest legal name", "ab".to_string(), None),
        ("a hyphenated name", "my-domain-7".to_string(), None),
        ("the longest legal name", longest.clone(), None),
        (
            "a create that names its own key",
            "keyed-domain".to_string(),
            Some("alias/some-caller-key"),
        ),
    ];

    for (label, domain, key) in &rows {
        let created = create_domain(&router, domain, *key).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected CreateDomain 200, got {created:?}"
        );
        let reported = created.body["domain"]["s3BucketArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            reported, expected,
            "case {label:?}: expected the account's asset bucket, got {created:?}"
        );
        // The suffix is exactly two components: the owner the domain reports, and the
        // Region. Nothing per-domain and nothing random follows them.
        let owner = created.body["domain"]["owner"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            reported
                .strip_prefix("arn:aws:s3:::assets-")
                .map(|suffix| suffix.split('-').count()),
            Some(format!("{owner}-{REGION}").split('-').count()),
            "case {label:?}: the bucket name must carry no segment beyond \
             <owner>-<region>, got {reported:?}"
        );

        // Every read of the domain reports the identical string — the create, the
        // describe, and the delete that answers for the removed domain.
        let described = describe_domain(&router, domain).await;
        assert_eq!(
            described.body["domain"]["s3BucketArn"].as_str(),
            Some(expected.as_str()),
            "case {label:?}: DescribeDomain must report the same bucket, got {described:?}"
        );
        let deleted = delete_domain(&router, domain).await;
        assert_eq!(
            (deleted.status, deleted.body["domain"]["s3BucketArn"].as_str()),
            (200, Some(expected.as_str())),
            "case {label:?}: DeleteDomain must report the same bucket, got {deleted:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: the key a domain reports is the key it is encrypted with
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_domain_reports_the_key_it_is_encrypted_with() {
    let router = emulator_with_kms();

    // One customer key, created through KMS itself, plus an alias for it: the four
    // spellings `encryptionKey` accepts are four ways of naming THIS key.
    let (status, key) = kms(
        &router,
        "CreateKey",
        json!({ "Description": "codeartifact domain key" }),
    )
    .await;
    assert_eq!(status, 200, "expected CreateKey 200, got {key:?}");
    let key_id = key["KeyMetadata"]["KeyId"]
        .as_str()
        .expect("CreateKey reports a key id")
        .to_string();
    let key_arn = key["KeyMetadata"]["Arn"]
        .as_str()
        .expect("CreateKey reports a key ARN")
        .to_string();
    let alias = "alias/codeartifact-domains";
    let (status, answer) = kms(
        &router,
        "CreateAlias",
        json!({ "AliasName": alias, "TargetKeyId": key_id }),
    )
    .await;
    assert_eq!(status, 200, "expected CreateAlias 200, got {answer:?}");
    let alias_arn = format!("arn:aws:kms:{REGION}:{ACCOUNT}:{alias}");

    // A reference KMS holds no key for: the caller's own key, in a store this emulator
    // does not keep, which is recorded as the caller wrote it.
    let foreign = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/not-a-key-this-store-holds");

    // Rows differ along the FORM of the reference (none, key id, key ARN, alias name,
    // alias ARN, a reference naming no key here) — the dimension the rule generalises
    // over.
    let rows: Vec<(&str, String, Option<String>, String)> = vec![
        (
            "no key named: the managed key of the account and Region",
            "managed-one".to_string(),
            None,
            String::new(),
        ),
        (
            "a second domain with no key named: the SAME managed key",
            "managed-two".to_string(),
            None,
            String::new(),
        ),
        (
            "the key's own ARN",
            "by-arn".to_string(),
            Some(key_arn.clone()),
            key_arn.clone(),
        ),
        (
            "a bare key id",
            "by-id".to_string(),
            Some(key_id.clone()),
            key_arn.clone(),
        ),
        (
            "an alias name",
            "by-alias".to_string(),
            Some(alias.to_string()),
            key_arn.clone(),
        ),
        (
            "an alias ARN",
            "by-alias-arn".to_string(),
            Some(alias_arn.clone()),
            key_arn.clone(),
        ),
        (
            "a reference this store holds no key for",
            "by-foreign-key".to_string(),
            Some(foreign.clone()),
            foreign.clone(),
        ),
    ];

    // The managed key is whatever KMS associated with the predefined alias the first time
    // it was asked for; the two rows that name no key must agree on it.
    let mut managed: Option<String> = None;

    for (label, domain, requested, expected) in &rows {
        let created = create_domain(&router, domain, requested.as_deref()).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: expected CreateDomain 200, got {created:?}"
        );
        let reported = created.body["domain"]["encryptionKey"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        if requested.is_none() {
            // The managed key is a key KMS ACTUALLY HOLDS, and it is the one the
            // predefined service alias resolves to — asked of KMS itself, which is the
            // service that owns the key.
            let (status, described) =
                kms(&router, "DescribeKey", json!({ "KeyId": reported })).await;
            assert_eq!(
                (status, described["KeyMetadata"]["Arn"].as_str()),
                (200, Some(reported.as_str())),
                "case {label:?}: KMS must hold the key the domain reports, got {described:?}"
            );
            assert_eq!(
                described["KeyMetadata"]["KeyManager"].as_str(),
                Some("AWS"),
                "case {label:?}: the fallback key must be an Amazon Web Services managed \
                 key, got {described:?}"
            );
            let (status, by_alias) =
                kms(&router, "DescribeKey", json!({ "KeyId": MANAGED_KEY_ALIAS })).await;
            assert_eq!(
                (status, by_alias["KeyMetadata"]["Arn"].as_str()),
                (200, Some(reported.as_str())),
                "case {label:?}: the service's predefined alias must name that key, got \
                 {by_alias:?}"
            );
            match &managed {
                None => managed = Some(reported.clone()),
                Some(first) => assert_eq!(
                    &reported, first,
                    "case {label:?}: one account and Region has ONE managed key, got \
                     {reported:?} after {first:?}"
                ),
            }
        } else {
            assert_eq!(
                &reported, expected,
                "case {label:?}: expected the key the reference names, got {created:?}"
            );
        }

        // Every later read of the domain reports that same key: the record is what the
        // readers project, so none of them resolves anything again.
        let described = describe_domain(&router, domain).await;
        assert_eq!(
            described.body["domain"]["encryptionKey"].as_str(),
            Some(reported.as_str()),
            "case {label:?}: DescribeDomain must report the same key, got {described:?}"
        );
        let listed = list_domains(&router).await;
        assert_eq!(
            listed_key(&listed, domain).as_deref(),
            Some(reported.as_str()),
            "case {label:?}: ListDomains must report the same key, got {listed:?}"
        );
        let deleted = delete_domain(&router, domain).await;
        assert_eq!(
            (
                deleted.status,
                deleted.body["domain"]["encryptionKey"].as_str()
            ),
            (200, Some(reported.as_str())),
            "case {label:?}: DeleteDomain must report the same key, got {deleted:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: a policy revision is the digest of the stored document
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_policy_revision_is_the_digest_of_the_stored_document() {
    let router = emulator_with_kms();
    let domain = "revisions";
    let repository = "Revisions.repo";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "CreateDomain must succeed"
    );
    assert_eq!(
        create_repository(&router, domain, repository, None, &[])
            .await
            .status,
        200,
        "CreateRepository must succeed"
    );

    // Rows differ along the document: how it is spaced, whether a collapsing element is a
    // one-element array or a string, how many statements it carries, and what its string
    // literals hold (non-ASCII included) — everything the STORED form generalises over.
    let documents: Vec<(&str, String)> = vec![
        (
            "a compact one-statement document",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"One","Effect":"Allow","Principal":"*","Action":"codeartifact:DescribeDomain","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "the same policy, spaced and with a one-element action array",
            r#"{"Version": "2012-10-17", "Statement": [ {"Sid": "One", "Effect": "Allow", "Principal": "*", "Action": ["codeartifact:DescribeDomain"], "Resource": "*"} ]}"#
                .to_string(),
        ),
        (
            "two statements",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"One","Effect":"Allow","Principal":"*","Action":"codeartifact:DescribeDomain","Resource":"*"},{"Sid":"Two","Effect":"Deny","Principal":"*","Action":"codeartifact:DeleteDomain","Resource":"*"}]}"#
                .to_string(),
        ),
        (
            "a non-ASCII Sid",
            r#"{"Version":"2012-10-17","Statement":[{"Sid":"caffè","Effect":"Allow","Principal":"*","Action":"codeartifact:DescribeDomain","Resource":"*"}]}"#
                .to_string(),
        ),
    ];

    // Both policy-bearing resources, so the two families cannot mint tokens differently.
    for (family, is_domain) in [("domain", true), ("repository", false)] {
        let mut seen: Vec<(String, String)> = Vec::new();
        for (label, document) in &documents {
            let written = if is_domain {
                put_domain_policy(&router, domain, document, None).await
            } else {
                put_repository_policy(&router, domain, repository, document, None).await
            };
            assert_eq!(
                written.status, 200,
                "case {family}/{label:?}: the write must succeed, got {written:?}"
            );
            let stored = policy(&written)["document"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let revision = policy(&written)["revision"]
                .as_str()
                .unwrap_or_default()
                .to_string();

            // THE RULE: the token is the digest of the document that was stored.
            assert_eq!(
                revision,
                digest_revision(&stored),
                "case {family}/{label:?}: expected base64(SHA-256(stored document)), got \
                 {written:?}"
            );
            // 44 characters of base64 ending in '=', which is inside `PolicyRevision`'s
            // modelled length 1..100 and matches its `^\S+$` pattern.
            assert!(
                revision.chars().count() == 44
                    && revision.ends_with('=')
                    && !revision.chars().any(char::is_whitespace),
                "case {family}/{label:?}: expected a 44-character base64 token, got \
                 {revision:?}"
            );

            // Stable across reads: the token identifies the attached document, so reading
            // it back reports the same one.
            let read = if is_domain {
                get_domain_policy(&router, domain).await
            } else {
                get_repository_policy(&router, domain, repository).await
            };
            assert_eq!(
                policy(&read)["revision"].as_str(),
                Some(revision.as_str()),
                "case {family}/{label:?}: the revision must be stable across reads, got \
                 {read:?}"
            );

            // Two documents that STORE the same bytes report the same token, and bytes
            // that differ report a different one.
            for (earlier_stored, earlier_revision) in &seen {
                if *earlier_stored == stored {
                    assert_eq!(
                        &revision, earlier_revision,
                        "case {family}/{label:?}: one stored document must have one \
                         revision, got {revision:?} after {earlier_revision:?}"
                    );
                } else {
                    assert_ne!(
                        &revision, earlier_revision,
                        "case {family}/{label:?}: documents that differ must report \
                         different revisions, got {revision:?} twice"
                    );
                }
            }
            seen.push((stored, revision));
        }

        // The precondition: the revision of the attached document unlocks the next write,
        // and the revision of a document that is no longer attached does not.
        let (held_document, held_revision) =
            seen.last().cloned().expect("a document was attached");
        let stale = seen
            .iter()
            .find(|(document, _)| *document != held_document)
            .map(|(_, revision)| revision.clone())
            .expect("the rows store more than one document");
        let refused = if is_domain {
            put_domain_policy(&router, domain, &documents[0].1, Some(&stale)).await
        } else {
            put_repository_policy(&router, domain, repository, &documents[0].1, Some(&stale)).await
        };
        assert_eq!(
            (refused.status, refused.code()),
            (409, "ConflictException".to_string()),
            "case {family}: a revision naming another document must conflict, got {refused:?}"
        );
        let accepted = if is_domain {
            put_domain_policy(&router, domain, &documents[0].1, Some(&held_revision)).await
        } else {
            put_repository_policy(
                &router,
                domain,
                repository,
                &documents[0].1,
                Some(&held_revision),
            )
            .await
        };
        assert_eq!(
            accepted.status, 200,
            "case {family}: the attached document's revision must unlock the write, got \
             {accepted:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 4: deleting a package group that is not there is a success
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_a_package_group_that_is_not_there_succeeds() {
    let router = emulator_with_kms();
    let domain = "idempotent-groups";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "CreateDomain must succeed"
    );

    // One group the domain DOES hold, deleted once so the second delete of it is the
    // retry an idempotent teardown makes.
    let held = "/npm/kept/*";
    let removed = "/npm/gone/*";
    for pattern in [held, removed] {
        assert_eq!(
            create_package_group(&router, domain, pattern, None, None)
                .await
                .status,
            200,
            "CreatePackageGroup {pattern} must succeed"
        );
    }
    let first = delete_package_group(&router, domain, removed).await;
    assert_eq!(
        (first.status, first.group()["pattern"].as_str()),
        (200, Some(removed)),
        "the first delete must describe the group it removed, got {first:?}"
    );

    // Rows differ along HOW the group is absent: never created, just deleted, a deeper
    // path, a pattern naming another format, and a pattern that is not a path from the
    // domain root at all.
    let absent = [
        ("a pattern the domain never held", "/npm/never-created/*"),
        ("the group the delete above removed", removed),
        ("a deeper path", "/npm/@scope/nested/*"),
        ("another package format", "/pypi/absent/*"),
        ("a pattern that is not a path", "npm"),
    ];
    for (label, pattern) in &absent {
        let answer = delete_package_group(&router, domain, pattern).await;
        assert_eq!(
            (answer.status, answer.code(), answer.body.clone()),
            (200, String::new(), json!({})),
            "case {label:?}: deleting a group that is not there must answer an empty 200, \
             got {answer:?}"
        );
    }

    // The ROOT group is still refused: it is created with the domain, not by a caller.
    let root = delete_package_group(&router, domain, ROOT_GROUP).await;
    assert_eq!(
        (root.status, root.code()),
        (400, "ValidationException".to_string()),
        "the root group must still be refused, got {root:?}"
    );

    // And none of those calls changed the domain: the root group and the group the domain
    // holds both still read back.
    for pattern in [ROOT_GROUP, held] {
        let described = describe_package_group(&router, domain, pattern).await;
        assert_eq!(
            described.status, 200,
            "{pattern} must survive every delete above, got {described:?}"
        );
    }
    // ...while the group that WAS deleted stays deleted.
    let gone = describe_package_group(&router, domain, removed).await;
    assert_eq!(
        (gone.status, gone.code()),
        (404, "ResourceNotFoundException".to_string()),
        "a deleted group must not come back, got {gone:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 5: an external connection this service does not offer is not found
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unsupported_external_connection_is_refused_as_not_found() {
    let router = emulator_with_kms();
    let domain = "connections";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "CreateDomain must succeed"
    );

    // Rows differ along HOW the name misses the catalog: an unknown registry, a bare
    // registry name with no scope, a prefix of a published name, a case variant, and a
    // published name with a suffix. Each satisfies `ExternalConnectionName` (len 2..100,
    // pattern `^[A-Za-z0-9][A-Za-z0-9._\-:]{1,99}$`), so the boundary admits it.
    let unsupported = [
        "public:does-not-exist",
        "npmjs",
        "public:maven",
        "PUBLIC:NPMJS",
        "public:npmjs.org",
    ];
    for (index, connection) in unsupported.iter().enumerate() {
        let repository = format!("Conn.{index}");
        assert_eq!(
            create_repository(&router, domain, &repository, None, &[])
                .await
                .status,
            200,
            "case {connection:?}: CreateRepository must succeed"
        );
        let answer = associate(&router, domain, &repository, connection).await;
        assert_eq!(
            (answer.status, answer.code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {connection:?}: a name that is no registry must be a 404, got {answer:?}"
        );
        assert_eq!(
            answer.message(),
            external_connection_not_found(connection),
            "case {connection:?}: expected the service's not-found sentence, got {answer:?}"
        );
        assert_eq!(
            answer.body["resourceId"].as_str(),
            Some(*connection),
            "case {connection:?}: the refusal must name the connection asked for, got \
             {answer:?}"
        );
    }

    // The control: a name the catalog DOES hold is associated, so the rows above are
    // refused for the name and not for the request.
    let repository = "Conn.supported";
    assert_eq!(
        create_repository(&router, domain, repository, None, &[])
            .await
            .status,
        200,
        "CreateRepository must succeed"
    );
    let associated = associate(&router, domain, repository, "public:npmjs").await;
    assert_eq!(
        associated.status, 200,
        "a supported connection must associate, got {associated:?}"
    );
}
