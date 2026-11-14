// SPDX-License-Identifier: CC0-1.0

//! Custom domains and the API association one carries, end to end over HTTP:
//! `CreateDomainName`, `GetDomainName` and the three operations on
//! `/v1/domainnames/{domainName}/apiassociation` (`AssociateApi`, `GetApiAssociation`,
//! `DisassociateApi`).
//!
//! Five rules are asserted here, each over a TABLE or a class of inputs rather than
//! over one request:
//!
//! 1. every operation addressed to a custom domain resolves that domain from the
//!    store, so a domain name this account does not hold is refused with the modelled
//!    `NotFoundException` (404) and AppSync's text for an absent domain — whatever else
//!    the request names, and independently for each of several callers asking at the
//!    same instant;
//! 2. a created domain round-trips: the stored `DomainNameConfig` is what the create
//!    answers and what `GetDomainName` answers later, with an `appsyncDomainName` the
//!    service minted (no two domains share one), and a second create under a name this
//!    account already holds conflicts instead of replacing it;
//! 3. `AssociateApi` maps ONE API — of either resource family — onto the domain, and
//!    the mapping is reported in the non-terminal `PROCESSING` status until the clock
//!    has made the deployment due, after which it reports `SUCCESS` with no call in
//!    between;
//! 4. `DisassociateApi` leaves the domain carrying no association (and the domain
//!    itself untouched), so the read that follows is a not-found and repeating the call
//!    changes nothing;
//! 5. an API that cannot be mapped is refused and nothing is stored: an `apiId` that
//!    names no API — whether or not it could have named one — is a 400 naming the
//!    value it refused, and a PRIVATE API a 400 — "Private APIs can not be associated
//!    with custom domains".
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together.
//!
//! The harness (`call`, `router`, `Answer`, `create_api`) is the source-API association
//! suite's.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::source_api_association::{
    call, create_api, router, Answer, ACCOUNT, ID_ALPHABET, REGION,
};

/// AppSync's fixed text for a custom domain this account does not hold, on the two
/// operations that MODIFY the domain's association (`AssociateApi`,
/// `DisassociateApi`).
const DOMAIN_NOT_FOUND_ON_CHANGE: &str = "Domain name not found.";

/// AppSync's fixed text for the same condition on the operations that READ a custom
/// domain (`GetDomainName`, `GetApiAssociation`), which AppSync words without the
/// sentence's full stop.
const DOMAIN_NOT_FOUND_ON_READ: &str = "Domain name not found";

/// AppSync's fixed text for a domain that exists and carries no API association, which
/// states the condition about the DOMAIN rather than about the association.
const ASSOCIATION_NOT_FOUND: &str = "Domain name is not associated.";

/// The suffix of every `appsyncDomainName`: a custom domain is served through a
/// CloudFront distribution domain.
const APPSYNC_DOMAIN_SUFFIX: &str = ".cloudfront.net";

/// The Route 53 hosted zone id every CloudFront distribution — and therefore every
/// AppSync custom domain — resolves through. Source: the AWS-published hosted zone id
/// of CloudFront distributions.
const CLOUDFRONT_HOSTED_ZONE_ID: &str = "Z2FDTNDATAQYW2";

/// The largest `domainName` the model admits: `DomainName` is `@length` 1..253.
/// Source: `smithy_query(query="operation", operation="CreateDomainName")`.
const DOMAIN_NAME_MAX_LEN: usize = 253;

/// How long after the service's own deployment span a row that must observe the far
/// side of it waits. Only the TEST waits — the handler stores the due instant.
const PAST_THE_DEPLOYMENT: Duration = Duration::from_millis(250);

/// The `AssociationStatus` a mapping carries while the service is deploying it, and
/// the one it reaches once the deployment is done. Source: the `AssociationStatus`
/// enum, `smithy_query(query="shape", shape="AssociationStatus")`.
const PROCESSING: &str = "PROCESSING";
const SUCCESS: &str = "SUCCESS";

/// The number of characters an AppSync API id has; `state::API_ID_LEN` is the
/// service's own copy, and an id of exactly this length is well formed.
const API_ID_LEN: usize = 26;

/// How many callers race on one domain name.
const RACERS: usize = 3;

/// The AppSync service quota for custom domain names an account holds in one Region;
/// `ops::create_domain_name::DOMAIN_NAMES_PER_REGION` is the service's own copy of it.
const DOMAIN_NAMES_PER_REGION: usize = 25;

/// `CreateDomainName`'s path.
const DOMAIN_NAMES_PATH: &str = "/v1/domainnames";

/// `GetDomainName`'s path for `domain_name`.
fn domain_path(domain_name: &str) -> String {
    format!("/v1/domainnames/{domain_name}")
}

/// The path all three association operations share, for `domain_name`.
fn association_path(domain_name: &str) -> String {
    format!("/v1/domainnames/{domain_name}/apiassociation")
}

/// A domain name of `len` characters ending in a registrable suffix, so the whole
/// value still matches the modelled `DomainName` pattern.
fn domain_of_length(len: usize) -> String {
    let suffix = ".example.com";
    format!("{}{suffix}", "a".repeat(len - suffix.len()))
}

/// One row of the domain table: a `CreateDomainName` request and the API family the
/// domain is then mapped onto.
struct DomainRow {
    label: &'static str,
    domain: String,
    /// The `certificateArn` the create names — an ACM certificate or an IAM server
    /// certificate, the two forms `CertificateArn`'s pattern admits.
    certificate_arn: &'static str,
    description: Option<&'static str>,
    tags: Option<Value>,
    /// Which resource family the API mapped onto this domain is created in.
    family: Family,
}

/// Which resource family an API was created in — both are mapped onto a custom domain
/// through the same `apiId` member.
#[derive(Debug, Clone, Copy)]
enum Family {
    /// A `GraphqlApi`, created by `CreateGraphqlApi` (POST /v1/apis).
    Graphql,
    /// An `Api` (an Event API), created by `CreateApi` (POST /v2/apis).
    Event,
}

/// The domain table: the rows differ along the FORM of the domain name (label count,
/// digits and hyphens, both wildcard forms the pattern admits, the modelled maximum
/// length), along the certificate form, along `description` and `tags` being present or
/// absent, and along the family of the API the domain is mapped onto.
fn domain_rows() -> Vec<DomainRow> {
    vec![
        DomainRow {
            label: "a two-label domain",
            domain: "events.example".to_string(),
            certificate_arn: "arn:aws:acm:us-east-1:000000000000:certificate/two-label",
            description: None,
            tags: None,
            family: Family::Graphql,
        },
        DomainRow {
            label: "a three-label domain with a description",
            domain: "api.events.example.com".to_string(),
            certificate_arn: "arn:aws:acm:us-east-1:000000000000:certificate/three-label",
            description: Some("the events endpoint"),
            tags: None,
            family: Family::Event,
        },
        DomainRow {
            label: "labels carrying digits and hyphens, with tags",
            domain: "api-7.events-2.example.com".to_string(),
            certificate_arn: "arn:aws:iam::000000000000:server-certificate/digits_7",
            description: Some(""),
            tags: Some(json!({"stage": "beta", "owner": "events"})),
            family: Family::Graphql,
        },
        DomainRow {
            label: "a bare wildcard domain",
            domain: "*.events.example.com".to_string(),
            certificate_arn: "arn:aws:acm:us-east-1:000000000000:certificate/wildcard",
            description: None,
            tags: Some(json!({"stage": "prod"})),
            family: Family::Event,
        },
        DomainRow {
            label: "a prefixed wildcard domain",
            domain: "*dev.events.example.com".to_string(),
            certificate_arn: "arn:aws:acm:us-east-1:000000000000:certificate/prefixed",
            description: Some("the per-stage wildcard"),
            tags: None,
            family: Family::Graphql,
        },
        DomainRow {
            label: "a domain of the modelled maximum length",
            domain: domain_of_length(DOMAIN_NAME_MAX_LEN),
            certificate_arn: "arn:aws:acm:us-east-1:000000000000:certificate/longest",
            description: None,
            tags: None,
            family: Family::Event,
        },
    ]
}

/// `CreateDomainName` for `row`.
async fn create_domain(router: &MultiServiceRouter, row: &DomainRow) -> Answer {
    let mut body = json!({
        "domainName": row.domain,
        "certificateArn": row.certificate_arn,
    });
    if let Some(description) = row.description {
        body["description"] = json!(description);
    }
    if let Some(tags) = row.tags.clone() {
        body["tags"] = tags;
    }
    call(router, "POST", DOMAIN_NAMES_PATH, Some(body)).await
}

/// Create an Event API (`CreateApi`) whose only authorization provider is an API key,
/// and return the `apiId` the service minted.
async fn create_event_api(router: &MultiServiceRouter, name: &str) -> String {
    let modes = json!([{"authType": "API_KEY"}]);
    let answer = call(
        router,
        "POST",
        "/v2/apis",
        Some(json!({
            "name": name,
            "eventConfig": {
                "authProviders": [{"authType": "API_KEY"}],
                "connectionAuthModes": modes,
                "defaultPublishAuthModes": modes,
                "defaultSubscribeAuthModes": modes,
            },
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case create event API {name:?}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["api"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Create an API of `family` and return its `apiId`.
async fn create_api_of(router: &MultiServiceRouter, family: Family, name: &str) -> String {
    match family {
        Family::Graphql => create_api(router, name, state::GRAPHQL_API_TYPE).await.id,
        Family::Event => create_event_api(router, name).await,
    }
}

/// Create a GraphQL API whose `visibility` is PRIVATE and return its `apiId`.
///
/// Source: `CreateGraphqlApi`'s `visibility` member ("Sets the value of the GraphQL API
/// to public (GLOBAL) or private (PRIVATE)").
async fn create_private_api(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": name,
            "authenticationType": "API_KEY",
            "visibility": "PRIVATE",
            "queryDepthLimit": 0,
            "resolverCountLimit": 0,
            "xrayEnabled": false,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case create private API {name:?}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `message` an answer carries, or the empty string when it carries none.
fn message(answer: &Answer) -> &str {
    answer.body["message"].as_str().unwrap_or_default()
}

/// Assert `answer` is the modelled not-found carrying `expected_message`.
fn assert_not_found(case: &str, answer: &Answer, expected_message: &str) {
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (404, "NotFoundException"),
        "case {case}: expected 404 NotFoundException, got {} {} {:?}",
        answer.status,
        answer.error_type,
        answer.body
    );
    assert_eq!(
        message(answer),
        expected_message,
        "case {case}: expected the message {expected_message:?}, got {:?}",
        answer.body["message"]
    );
}

/// Domain names that are never created, spanning the same forms the create table
/// does: the refusal is a property of the domain being absent, not of its shape.
fn missing_domains() -> Vec<(&'static str, String)> {
    vec![
        ("a two-label domain", "missing.example".to_string()),
        (
            "a three-label domain",
            "api.missing.example.com".to_string(),
        ),
        (
            "labels carrying digits and hyphens",
            "api-9.missing-3.example.com".to_string(),
        ),
        ("a wildcard domain", "*.missing.example.com".to_string()),
        (
            "a domain of the modelled maximum length",
            domain_of_length(DOMAIN_NAME_MAX_LEN),
        ),
    ]
}

#[tokio::test]
async fn every_domain_addressed_operation_refuses_a_domain_that_was_never_created() {
    let router = router();
    // One stored domain and one stored API, so the refusals below cannot be "the store
    // is empty": the request names a real API, and another domain does exist.
    let api_id = create_api_of(&router, Family::Graphql, "Domain_control").await;
    let control = DomainRow {
        label: "the control domain",
        domain: "control.example.com".to_string(),
        certificate_arn: "arn:aws:acm:us-east-1:000000000000:certificate/control",
        description: None,
        tags: None,
        family: Family::Graphql,
    };
    let created = create_domain(&router, &control).await;
    assert_eq!(
        created.status, 200,
        "case the control domain: expected 200, got {} {:?}",
        created.status, created.body
    );

    for (label, domain) in missing_domains() {
        let associate = call(
            &router,
            "POST",
            &association_path(&domain),
            Some(json!({"apiId": api_id})),
        )
        .await;
        assert_not_found(
            &format!("AssociateApi on {label}"),
            &associate,
            DOMAIN_NOT_FOUND_ON_CHANGE,
        );

        let disassociate = call(&router, "DELETE", &association_path(&domain), None).await;
        assert_not_found(
            &format!("DisassociateApi on {label}"),
            &disassociate,
            DOMAIN_NOT_FOUND_ON_CHANGE,
        );

        let read_association = call(&router, "GET", &association_path(&domain), None).await;
        assert_not_found(
            &format!("GetApiAssociation on {label}"),
            &read_association,
            DOMAIN_NOT_FOUND_ON_READ,
        );

        let read_domain = call(&router, "GET", &domain_path(&domain), None).await;
        assert_not_found(
            &format!("GetDomainName on {label}"),
            &read_domain,
            DOMAIN_NOT_FOUND_ON_READ,
        );

        // None of the four refusals created the domain they were refused for.
        let after = call(&router, "GET", &domain_path(&domain), None).await;
        assert_not_found(
            &format!("GetDomainName after the refusals on {label}"),
            &after,
            DOMAIN_NOT_FOUND_ON_READ,
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_disassociates_on_a_missing_domain_refuse_every_caller() {
    let router = Arc::new(router());

    for (label, domain) in missing_domains() {
        let mut racers = Vec::with_capacity(RACERS);
        for _ in 0..RACERS {
            let router = Arc::clone(&router);
            let path = association_path(&domain);
            racers.push(tokio::spawn(async move {
                call(&router, "DELETE", &path, None).await
            }));
        }

        let mut answers = Vec::with_capacity(RACERS);
        for racer in racers {
            answers.push(racer.await.expect("the racer task completes"));
        }

        // The refusal is derived from the domain being absent, so there is no
        // winner/loser split: every caller is refused the same way, and never with the
        // concurrent-modification error, which needs an association to be in progress.
        for (index, answer) in answers.iter().enumerate() {
            assert_not_found(
                &format!("concurrent DisassociateApi {index} on {label}"),
                answer,
                DOMAIN_NOT_FOUND_ON_CHANGE,
            );
        }
    }
}

#[tokio::test]
async fn a_custom_domain_round_trips_and_carries_one_api_association() {
    let router = router();

    let mut endpoints: Vec<String> = Vec::new();
    for row in domain_rows() {
        let case = row.label;
        let created = create_domain(&router, &row).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected CreateDomainName 200, got {} {:?}",
            created.status, created.body
        );
        let config = created.body["domainNameConfig"].clone();

        assert_eq!(
            config["domainName"].as_str(),
            Some(row.domain.as_str()),
            "case {case}: expected the created domain to report the name it was created \
             under, got {config:?}"
        );
        assert_eq!(
            config["certificateArn"].as_str(),
            Some(row.certificate_arn),
            "case {case}: expected the certificate the create named, got {config:?}"
        );
        assert_eq!(
            config["description"].as_str(),
            row.description,
            "case {case}: expected the description the create named, got {config:?}"
        );
        assert_eq!(
            config["domainNameArn"].as_str(),
            Some(
                format!("arn:aws:appsync:{REGION}:{ACCOUNT}:domainnames/{}", row.domain).as_str()
            ),
            "case {case}: expected the domain's own ARN, got {config:?}"
        );
        assert_eq!(
            config["hostedZoneId"].as_str(),
            Some(CLOUDFRONT_HOSTED_ZONE_ID),
            "case {case}: expected the published CloudFront hosted zone id, got {config:?}"
        );
        let expected_tags = row.tags.clone().unwrap_or_else(|| json!({}));
        assert_eq!(
            config["tags"], expected_tags,
            "case {case}: expected the tags the create named, got {:?}",
            config["tags"]
        );

        // The endpoint AppSync serves the domain through is the service's own: a
        // CloudFront distribution domain whose label is drawn from the alphabet this
        // service mints opaque ids over, and no two domains share one.
        let endpoint = config["appsyncDomainName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let label_part = endpoint.strip_suffix(APPSYNC_DOMAIN_SUFFIX);
        assert!(
            label_part.is_some_and(|label| !label.is_empty()
                && label.chars().all(|ch| ID_ALPHABET.contains(ch))),
            "case {case}: expected an appsyncDomainName of minted label plus \
             {APPSYNC_DOMAIN_SUFFIX:?}, got {endpoint:?}"
        );
        assert!(
            !endpoints.contains(&endpoint),
            "case {case}: expected an appsyncDomainName no other domain carries, got \
             {endpoint:?} among {endpoints:?}"
        );
        endpoints.push(endpoint);

        // The create answered from the store, so the read answers the same body.
        let read = call(&router, "GET", &domain_path(&row.domain), None).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected GetDomainName 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["domainNameConfig"], config,
            "case {case}: expected GetDomainName to report the created configuration, got \
             {:?}",
            read.body["domainNameConfig"]
        );

        // A fresh domain carries no association at all.
        let before = call(&router, "GET", &association_path(&row.domain), None).await;
        assert_not_found(
            &format!("{case}: GetApiAssociation before any AssociateApi"),
            &before,
            ASSOCIATION_NOT_FOUND,
        );

        // Mapping an API onto the domain reports the mapping being deployed.
        let api_id = create_api_of(&router, row.family, &format!("Domain_api_{}", endpoints.len()))
            .await;
        let associated = call(
            &router,
            "POST",
            &association_path(&row.domain),
            Some(json!({"apiId": api_id})),
        )
        .await;
        assert_eq!(
            associated.status, 200,
            "case {case}: expected AssociateApi 200, got {} {:?}",
            associated.status, associated.body
        );
        let association = associated.body["apiAssociation"].clone();
        assert_eq!(
            (
                association["domainName"].as_str(),
                association["apiId"].as_str(),
                association["associationStatus"].as_str(),
            ),
            (
                Some(row.domain.as_str()),
                Some(api_id.as_str()),
                Some(PROCESSING),
            ),
            "case {case}: expected the fresh mapping to name the domain and API and be \
             reported as being deployed, got {association:?}"
        );
        assert!(
            association["deploymentDetail"]
                .as_str()
                .is_some_and(|detail| !detail.is_empty()),
            "case {case}: expected the mapping to report its deployment detail, got \
             {association:?}"
        );

        // The same body is what the read reports while the deployment is still owed.
        let read_association = call(&router, "GET", &association_path(&row.domain), None).await;
        assert_eq!(
            read_association.status, 200,
            "case {case}: expected GetApiAssociation 200, got {} {:?}",
            read_association.status, read_association.body
        );
        assert_eq!(
            read_association.body["apiAssociation"], association,
            "case {case}: expected GetApiAssociation to report the stored mapping, got {:?}",
            read_association.body["apiAssociation"]
        );

        // Once the clock has made the deployment due the mapping is successful, with no
        // call in between.
        tokio::time::sleep(state::API_ASSOCIATION_SPAN + PAST_THE_DEPLOYMENT).await;
        let settled = call(&router, "GET", &association_path(&row.domain), None).await;
        assert_eq!(
            settled.body["apiAssociation"]["associationStatus"].as_str(),
            Some(SUCCESS),
            "case {case}: expected the deployed mapping to report success, got {:?}",
            settled.body["apiAssociation"]
        );
        assert_eq!(
            settled.body["apiAssociation"]["apiId"].as_str(),
            Some(api_id.as_str()),
            "case {case}: expected the deployed mapping to still name its API, got {:?}",
            settled.body["apiAssociation"]
        );

        // Unmapping leaves the domain carrying no association, and repeating it reports
        // the same outcome.
        for attempt in 0..2 {
            let disassociated = call(&router, "DELETE", &association_path(&row.domain), None).await;
            assert_eq!(
                disassociated.status, 200,
                "case {case} attempt {attempt}: expected DisassociateApi 200, got {} {:?}",
                disassociated.status, disassociated.body
            );
            let after = call(&router, "GET", &association_path(&row.domain), None).await;
            assert_not_found(
                &format!("{case} attempt {attempt}: GetApiAssociation after DisassociateApi"),
                &after,
                ASSOCIATION_NOT_FOUND,
            );
        }

        // Unmapping the API left the DOMAIN exactly as it was created.
        let reread = call(&router, "GET", &domain_path(&row.domain), None).await;
        assert_eq!(
            (reread.status, reread.body["domainNameConfig"].clone()),
            (200, config),
            "case {case}: expected the domain itself to outlive its association, got {} {:?}",
            reread.status,
            reread.body
        );
    }
}

#[tokio::test]
async fn an_api_the_domain_cannot_carry_is_refused_and_stores_nothing() {
    let router = router();
    let domain = "refusals.example.com".to_string();
    let created = create_domain(
        &router,
        &DomainRow {
            label: "the refusal domain",
            domain: domain.clone(),
            certificate_arn: "arn:aws:acm:us-east-1:000000000000:certificate/refusals",
            description: None,
            tags: None,
            family: Family::Graphql,
        },
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case the refusal domain: expected 200, got {} {:?}",
        created.status, created.body
    );

    let private_api = create_private_api(&router, "Domain_private").await;
    // An id of exactly the length this service mints, over the alphabet it mints ids
    // in, that nothing created: it could name an API, and names none.
    let unknown_api = "a".repeat(API_ID_LEN);
    // One character past that length: a value no AppSync API id could be.
    let overlong_api = "a".repeat(API_ID_LEN + 1);

    let rows: Vec<(&str, String, u16, &str, String)> = vec![
        (
            "an apiId that names no API",
            unknown_api.clone(),
            400,
            "BadRequestException",
            // This operation's refusal for an `apiId` it cannot resolve: the value it
            // refused. The `NotFoundException` it models is the custom domain's.
            format!("Invalid API ID {unknown_api}"),
        ),
        (
            "an apiId longer than an AppSync API id",
            overlong_api.clone(),
            400,
            "BadRequestException",
            // AppSync's text for an id that could name no API: the value it refused.
            format!("Invalid API ID {overlong_api}"),
        ),
        (
            "a PRIVATE API",
            private_api,
            400,
            "BadRequestException",
            "Private APIs can not be associated with custom domains.".to_string(),
        ),
    ];

    for (case, api_id, status, error_type, expected_message) in rows {
        let answer = call(
            &router,
            "POST",
            &association_path(&domain),
            Some(json!({"apiId": api_id})),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (status, error_type),
            "case {case}: expected {status} {error_type}, got {} {} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
        assert_eq!(
            message(&answer),
            expected_message.as_str(),
            "case {case}: expected the message {expected_message:?}, got {:?}",
            answer.body["message"]
        );

        // The refused mapping stored nothing: the domain still carries no association.
        let after = call(&router, "GET", &association_path(&domain), None).await;
        assert_not_found(
            &format!("{case}: GetApiAssociation after the refusal"),
            &after,
            ASSOCIATION_NOT_FOUND,
        );
    }
}

#[tokio::test]
async fn a_second_create_conflicts_with_the_domain_this_account_holds() {
    let router = router();
    let first = DomainRow {
        label: "the first create",
        domain: "once.example.com".to_string(),
        certificate_arn: "arn:aws:acm:us-east-1:000000000000:certificate/once",
        description: Some("the first description"),
        tags: None,
        family: Family::Graphql,
    };
    let created = create_domain(&router, &first).await;
    assert_eq!(
        created.status, 200,
        "case the first create: expected 200, got {} {:?}",
        created.status, created.body
    );
    let stored = created.body["domainNameConfig"].clone();

    let again = create_domain(
        &router,
        &DomainRow {
            label: "the second create",
            domain: first.domain.clone(),
            certificate_arn: "arn:aws:iam::000000000000:server-certificate/second",
            description: Some("the second description"),
            tags: Some(json!({"stage": "prod"})),
            family: Family::Graphql,
        },
    )
    .await;
    assert_eq!(
        (again.status, again.error_type.as_str()),
        (400, "BadRequestException"),
        "case the second create: expected 400 BadRequestException, got {} {} {:?}",
        again.status,
        again.error_type,
        again.body
    );
    assert_eq!(
        message(&again),
        "Domain name already exists.",
        "case the second create: expected AppSync's already-exists text, got {:?}",
        again.body["message"]
    );

    // The refused create replaced nothing: the stored domain is still the first one.
    let read = call(&router, "GET", &domain_path(&first.domain), None).await;
    assert_eq!(
        (read.status, read.body["domainNameConfig"].clone()),
        (200, stored),
        "case the second create: expected the first configuration to survive, got {} {:?}",
        read.status,
        read.body
    );
}

#[tokio::test]
async fn the_domain_store_is_bounded_by_the_service_quota() {
    let router = router();

    for index in 0..DOMAIN_NAMES_PER_REGION {
        let row = DomainRow {
            label: "a domain inside the quota",
            domain: format!("quota-{index}.example.com"),
            certificate_arn: "arn:aws:acm:us-east-1:000000000000:certificate/quota",
            description: None,
            tags: None,
            family: Family::Graphql,
        };
        let answer = create_domain(&router, &row).await;
        assert_eq!(
            answer.status,
            200,
            "case create {} of {DOMAIN_NAMES_PER_REGION}: expected 200, got {} {:?}",
            index + 1,
            answer.status,
            answer.body
        );
    }

    let past_the_quota = DomainRow {
        label: "a domain one past the quota",
        domain: "quota-past.example.com".to_string(),
        certificate_arn: "arn:aws:acm:us-east-1:000000000000:certificate/quota",
        description: None,
        tags: None,
        family: Family::Graphql,
    };
    let refused = create_domain(&router, &past_the_quota).await;
    assert_eq!(
        (refused.status, refused.error_type.as_str()),
        (400, "BadRequestException"),
        "case a domain one past the quota: expected 400 BadRequestException, got {} {} {:?}",
        refused.status,
        refused.error_type,
        refused.body
    );
    assert!(
        message(&refused).contains(&DOMAIN_NAMES_PER_REGION.to_string()),
        "case a domain one past the quota: expected the refusal to state the quota \
         {DOMAIN_NAMES_PER_REGION}, got {:?}",
        refused.body["message"]
    );

    // The refused create stored nothing, and the domains inside the quota are intact.
    let after = call(&router, "GET", &domain_path(&past_the_quota.domain), None).await;
    assert_not_found(
        "a domain one past the quota: GetDomainName after the refusal",
        &after,
        DOMAIN_NOT_FOUND_ON_READ,
    );
    let first = call(&router, "GET", &domain_path("quota-0.example.com"), None).await;
    assert_eq!(
        first.status, 200,
        "case a domain one past the quota: expected the stored domains to survive, got {} {:?}",
        first.status, first.body
    );
}

#[tokio::test]
async fn an_association_being_deployed_cannot_be_modified() {
    let router = router();
    let domain = "inflight.example.com".to_string();
    let created = create_domain(
        &router,
        &DomainRow {
            label: "the in-flight domain",
            domain: domain.clone(),
            certificate_arn: "arn:aws:acm:us-east-1:000000000000:certificate/inflight",
            description: None,
            tags: None,
            family: Family::Graphql,
        },
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case the in-flight domain: expected 200, got {} {:?}",
        created.status, created.body
    );
    let first_api = create_api_of(&router, Family::Graphql, "Domain_inflight_one").await;
    let second_api = create_api_of(&router, Family::Event, "Domain_inflight_two").await;

    let associated = call(
        &router,
        "POST",
        &association_path(&domain),
        Some(json!({"apiId": first_api})),
    )
    .await;
    assert_eq!(
        associated.body["apiAssociation"]["associationStatus"].as_str(),
        Some(PROCESSING),
        "case the in-flight domain: expected the mapping to be reported as being deployed, \
         got {:?}",
        associated.body
    );

    // While the deployment is owed, neither modification is accepted: each operation
    // answers with the error ITS model declares for a modification in progress.
    let unmap = call(&router, "DELETE", &association_path(&domain), None).await;
    assert_eq!(
        (unmap.status, unmap.error_type.as_str()),
        (409, "ConcurrentModificationException"),
        "case the in-flight domain: expected DisassociateApi to be refused with 409 \
         ConcurrentModificationException, got {} {} {:?}",
        unmap.status,
        unmap.error_type,
        unmap.body
    );
    let remap = call(
        &router,
        "POST",
        &association_path(&domain),
        Some(json!({"apiId": second_api})),
    )
    .await;
    assert_eq!(
        (remap.status, remap.error_type.as_str()),
        (400, "BadRequestException"),
        "case the in-flight domain: expected AssociateApi to be refused with 400 \
         BadRequestException, got {} {} {:?}",
        remap.status,
        remap.error_type,
        remap.body
    );
    for (case, answer) in [("DisassociateApi", &unmap), ("AssociateApi", &remap)] {
        let expected = match case {
            // `ConcurrentModificationException`'s own text, as the model documents the
            // shape.
            "DisassociateApi" => {
                "Another modification is in progress at this time and it must complete before \
                 you can make your change."
            }
            // The ground the request is malformed on, in `AssociationStatus`' words.
            _ => "You cannot modify association requests during processing.",
        };
        assert_eq!(
            message(answer),
            expected,
            "case the in-flight domain ({case}): expected the modification-in-progress \
             text, got {:?}",
            answer.body["message"]
        );
    }
    // Neither refusal changed the mapping.
    let read = call(&router, "GET", &association_path(&domain), None).await;
    assert_eq!(
        read.body["apiAssociation"]["apiId"].as_str(),
        Some(first_api.as_str()),
        "case the in-flight domain: expected the first API to still be mapped, got {:?}",
        read.body["apiAssociation"]
    );

    // Once the deployment the clock owed has been applied, both modifications are
    // accepted again: the second API replaces the first.
    tokio::time::sleep(state::API_ASSOCIATION_SPAN + PAST_THE_DEPLOYMENT).await;
    let remapped = call(
        &router,
        "POST",
        &association_path(&domain),
        Some(json!({"apiId": second_api})),
    )
    .await;
    assert_eq!(
        (
            remapped.status,
            remapped.body["apiAssociation"]["apiId"].as_str()
        ),
        (200, Some(second_api.as_str())),
        "case the in-flight domain: expected the deployed mapping to be replaceable, got {} \
         {:?}",
        remapped.status,
        remapped.body
    );
    tokio::time::sleep(state::API_ASSOCIATION_SPAN + PAST_THE_DEPLOYMENT).await;
    let unmapped = call(&router, "DELETE", &association_path(&domain), None).await;
    assert_eq!(
        unmapped.status, 200,
        "case the in-flight domain: expected DisassociateApi 200 once nothing is in \
         progress, got {} {:?}",
        unmapped.status, unmapped.body
    );
}
