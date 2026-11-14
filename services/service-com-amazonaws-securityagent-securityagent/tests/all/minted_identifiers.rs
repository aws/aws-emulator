// SPDX-License-Identifier: CC0-1.0

//! The values the SecurityAgent emulator MINTS — every resource identifier, and the
//! ownership token a target domain's owner is told to publish — end to end over the
//! wire.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots (see
//! `crate::support`), and every minted id is read back through its own resource type's
//! reader before it counts, so nothing here passes against a handler that answers a
//! canned body.
//!
//! RULE: every identifier and every verification token this service mints is a fresh
//! opaque value of the account's ONE mint — unique across every resource the account
//! holds whatever its type, carrying nothing the request supplied, and never the
//! ordinal of the call that asked for it, so no two mints hand out one value. An
//! identifier is rendered in the form its RESOURCE CLASS is read by (that class's
//! prefix, then the draw as this resource's own body); `crate::minted_identifier_forms`
//! owns that form rule, and what the cases here add is the uniqueness it sits on.
//!
//! The cases assert that rule over a TABLE whose rows span the dimension it
//! generalises over — the resource TYPE, since a collision across types is exactly what
//! it forbids — and, within a type, the input: byte-identical requests, a request that
//! carries nothing at all, and every `DomainVerificationMethod` the model admits.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    agent_space_arn, default_vpc_id, region_subnets, AGENT_SPACE_ID_PREFIX,
    LATTICE_ID_SUFFIX_LEN, RESOURCE_CONFIGURATION_ID_PREFIX, RESOURCE_GATEWAY_ID_PREFIX,
    SECURITY_REQUIREMENT_PACK_ID_PREFIX, TARGET_DOMAIN_ID_PREFIX, VERIFICATION_METHOD_DNS_TXT,
    VERIFICATION_METHOD_HTTP_ROUTE, VERIFICATION_METHOD_PRIVATE_VPC,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{assert_uuid_body, call, emulator, member, ok};

/// The region every case signs for, and so the region the ARNs and the account's
/// network below are read in.
const REGION: &str = "us-east-1";

/// How many characters one of this service's own OPAQUE identifiers carries — the form
/// a resource class the service is not known to hand a prefixed identifier out for is
/// minted in. The form is the emulator's to choose there (no identifier shape of this
/// service carries a `@pattern` or a `@length` in the model), so what a case may assert
/// about it is the form the service does hand out, and above all that it is hex, which
/// is what makes it impossible for the value to echo the names the requests below carry.
const MINTED_ID_LEN: usize = 32;

/// How many characters a domain-ownership token carries.
const VERIFICATION_TOKEN_LEN: usize = 64;

/// What form a minted value takes — the only thing about it a caller may rely on
/// besides its uniqueness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    /// An identifier of one of this service's resource CLASSES, in that class's own
    /// form: its prefix, then the uuid body minted for this one resource.
    ClassId(&'static str),
    /// An identifier of a class the service is not known to prefix: [`MINTED_ID_LEN`]
    /// lowercase hex.
    OpaqueId,
    /// A domain-ownership token: [`VERIFICATION_TOKEN_LEN`] lowercase hex.
    Token,
    /// An identifier of a resource this service creates in VPC Lattice, in that
    /// vendor's own `<prefix><17 of [0-9a-z]>` id form.
    LatticeId(&'static str),
}

/// One value the account's mint handed out, and the call it was handed to.
#[derive(Clone, Debug)]
struct Minted {
    case: String,
    /// The response member that carried it.
    member: &'static str,
    value: String,
    form: Form,
}

/// A string member a response must carry.
fn required(payload: &Value, name: &str, case: &str) -> String {
    member(payload, name)
        .unwrap_or_else(|| panic!("case {case:?}: the response carries no {name}: {payload}"))
}

/// The ONE record a read of a single id reports.
fn single(payload: &Value, list: &str, case: &str) -> Value {
    let records = payload
        .get(list)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: the read carries no {list} array: {payload}"));
    assert_eq!(
        records.len(),
        1,
        "case {case:?}: expected exactly one {list} record, got {payload}"
    );
    records
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the one {list} record is missing: {payload}"))
}

/// The verification token the challenge of a target domain projection carries, or
/// `None` for a method that publishes none (`PRIVATE_VPC` is proven by reaching the
/// domain from the VPC, so it carries the method alone).
fn challenge_token(payload: &Value) -> Option<String> {
    let details = payload.get("verificationDetails")?;
    ["dnsTxt", "httpRoute"]
        .iter()
        .find_map(|challenge| details.get(challenge).and_then(|held| member(held, "token")))
}

/// Create one resource of every type this service mints values for, and answer every
/// value the mint handed out — each one read back through that type's own reader first.
async fn mint_every_resource(router: &MultiServiceRouter) -> Vec<Minted> {
    let mut minted: Vec<Minted> = Vec::new();

    // Agent spaces. The rows differ along the request — a name alone, a name and a
    // description, and the FIRST name a second time — because nothing the request
    // carries decides the identity.
    let mut spaces: Vec<String> = Vec::new();
    for (case, request) in [
        ("an agent space", json!({"name": "mint-space-a"})),
        (
            "an agent space carrying a description",
            json!({"name": "mint-space-b", "description": "the second space"}),
        ),
        (
            "an agent space created again with the first name",
            json!({"name": "mint-space-a"}),
        ),
    ] {
        let created = ok(router, "CreateAgentSpace", &request, case).await;
        let id = required(&created, "agentSpaceId", case);
        // One resource, one identity: the recorded `id` beside the modelled
        // `agentSpaceId`, and the ARN's resource suffix, all carry the value just
        // minted.
        assert_eq!(
            required(&created, "id", case),
            id,
            "case {case:?}: id and agentSpaceId must carry the one minted value: {created}"
        );
        assert_eq!(
            required(&created, "arn", case),
            agent_space_arn(REGION, &id),
            "case {case:?}: the ARN must name the id just minted: {created}"
        );
        let read = ok(
            router,
            "BatchGetAgentSpaces",
            &json!({"agentSpaceIds": [&id]}),
            case,
        )
        .await;
        assert_eq!(
            required(&single(&read, "agentSpaces", case), "agentSpaceId", case),
            id,
            "case {case:?}: the agent space must read back under the id it was minted \
             with: {read}"
        );
        spaces.push(id.clone());
        minted.push(Minted {
            case: case.to_string(),
            member: "agentSpaceId",
            value: id,
            form: Form::ClassId(AGENT_SPACE_ID_PREFIX),
        });
    }

    // Security requirement packs, whose create answers the modelled 201. Two rows
    // carrying one name: a pack is a resource, not a name.
    for (case, request) in [
        ("a security requirement pack", json!({"name": "mint-pack-a"})),
        (
            "a pack created again with the same name",
            json!({"name": "mint-pack-a"}),
        ),
    ] {
        let answer = call(router, "CreateSecurityRequirementPack", &request).await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected CreateSecurityRequirementPack to answer 201, got \
             {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        let id = required(&answer.body, "packId", case);
        let read = ok(
            router,
            "GetSecurityRequirementPack",
            &json!({"packId": &id}),
            case,
        )
        .await;
        assert_eq!(
            required(&read, "packId", case),
            id,
            "case {case:?}: the pack must read back under the id it was minted with: \
             {read}"
        );
        minted.push(Minted {
            case: case.to_string(),
            member: "packId",
            value: id,
            form: Form::ClassId(SECURITY_REQUIREMENT_PACK_ID_PREFIX),
        });
    }

    // Target domains: one per `DomainVerificationMethod` the model admits
    // (`smithy_query(query="shape",shape="DomainVerificationMethod")`), plus a repeat of
    // a name already registered. Each registration is handed an id AND, where its method
    // has a challenge to publish, a token of its own.
    for (case, domain, method) in [
        (
            "a DNS_TXT target domain",
            "mint-one.example.com",
            VERIFICATION_METHOD_DNS_TXT,
        ),
        (
            "an HTTP_ROUTE target domain",
            "mint-two.example.com",
            VERIFICATION_METHOD_HTTP_ROUTE,
        ),
        (
            "a PRIVATE_VPC target domain",
            "mint-three.example.com",
            VERIFICATION_METHOD_PRIVATE_VPC,
        ),
        (
            "the first domain name registered a second time",
            "mint-one.example.com",
            VERIFICATION_METHOD_DNS_TXT,
        ),
    ] {
        let created = ok(
            router,
            "CreateTargetDomain",
            &json!({"targetDomainName": domain, "verificationMethod": method}),
            case,
        )
        .await;
        let id = required(&created, "targetDomainId", case);
        let read = ok(
            router,
            "BatchGetTargetDomains",
            &json!({"targetDomainIds": [&id]}),
            case,
        )
        .await;
        let record = single(&read, "targetDomains", case);
        assert_eq!(
            required(&record, "targetDomainId", case),
            id,
            "case {case:?}: the domain must read back under the id it was minted with: \
             {read}"
        );
        minted.push(Minted {
            case: case.to_string(),
            member: "targetDomainId",
            value: id.clone(),
            form: Form::ClassId(TARGET_DOMAIN_ID_PREFIX),
        });
        match challenge_token(&created) {
            Some(token) => {
                // The token is the record's own state, so the read reports the one the
                // registration minted rather than recomputing anything.
                assert_eq!(
                    challenge_token(&record),
                    Some(token.clone()),
                    "case {case:?}: the read must report the token the registration \
                     minted: {record}"
                );
                assert_ne!(
                    token, id,
                    "case {case:?}: the ownership token is a value of its own, not the \
                     domain's id"
                );
                minted.push(Minted {
                    case: case.to_string(),
                    member: "verificationDetails.token",
                    value: token,
                    form: Form::Token,
                });
            }
            None => assert_eq!(
                method, VERIFICATION_METHOD_PRIVATE_VPC,
                "case {case:?}: a {method} registration must carry the token its owner \
                 publishes: {created}"
            ),
        }
    }

    // Applications, whose request carries nothing at all: the whole identity is the
    // service's to mint.
    for case in ["an application", "a second application"] {
        let created = ok(router, "CreateApplication", &json!({}), case).await;
        let id = required(&created, "applicationId", case);
        let read = ok(
            router,
            "GetApplication",
            &json!({"applicationId": &id}),
            case,
        )
        .await;
        assert_eq!(
            required(&read, "applicationId", case),
            id,
            "case {case:?}: the application must read back under the id it was minted \
             with: {read}"
        );
        minted.push(Minted {
            case: case.to_string(),
            member: "applicationId",
            value: id,
            form: Form::OpaqueId,
        });
    }

    // Two artifacts uploaded into ONE agent space with the same file name and the same
    // bytes: two uploads are two resources.
    let space = spaces
        .first()
        .cloned()
        .expect("the rows above created an agent space");
    for case in [
        "an artifact",
        "a second artifact of the same name and bytes",
    ] {
        let answer = call(
            router,
            "AddArtifact",
            &json!({
                "agentSpaceId": &space,
                "fileName": "finding.json",
                "artifactType": "TXT",
                // A blob travels as base64 on this protocol.
                "artifactContent": aws_smithy_types::base64::encode(b"{}"),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected AddArtifact to answer 201, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        let id = required(&answer.body, "artifactId", case);
        let read = ok(
            router,
            "GetArtifact",
            &json!({"agentSpaceId": &space, "artifactId": &id}),
            case,
        )
        .await;
        let artifact = read
            .get("artifact")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the read carries no artifact: {read}"));
        assert!(
            member(&artifact, "contents").is_some(),
            "case {case:?}: the read must report the stored artifact: {read}"
        );
        assert_eq!(
            required(&read, "artifactId", case),
            id,
            "case {case:?}: the artifact must read back under the id it was minted \
             with: {read}"
        );
        minted.push(Minted {
            case: case.to_string(),
            member: "artifactId",
            value: id,
            form: Form::OpaqueId,
        });
    }

    // A service-managed private connection, whose gateway and resource configuration
    // this service creates in VPC Lattice: two more identifiers it mints, in that
    // vendor's id form rather than its own.
    let subnet = region_subnets(REGION)
        .into_iter()
        .map(|subnet| subnet.subnet_id)
        .next()
        .expect("the account's network has a subnet in this Region");
    let case = "a service-managed private connection";
    let name = "mint-connection";
    let answer = call(
        router,
        "CreatePrivateConnection",
        &json!({
            "privateConnectionName": name,
            "mode": {"serviceManaged": {
                "hostAddress": "provider.internal.example",
                "vpcId": default_vpc_id(REGION),
                "subnetIds": [subnet],
            }},
        }),
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected CreatePrivateConnection to answer 201, got {} ({}) \
         with {}",
        answer.status, answer.error_code, answer.body
    );
    let read = ok(
        router,
        "DescribePrivateConnection",
        &json!({"privateConnectionName": name}),
        case,
    )
    .await;
    for (part, prefix) in [
        ("resourceGatewayId", RESOURCE_GATEWAY_ID_PREFIX),
        ("resourceConfigurationId", RESOURCE_CONFIGURATION_ID_PREFIX),
    ] {
        let id = required(&answer.body, part, case);
        assert_eq!(
            required(&read, part, case),
            id,
            "case {case:?}: the read must report the {part} the create minted: {read}"
        );
        minted.push(Minted {
            case: case.to_string(),
            member: match part {
                "resourceGatewayId" => "resourceGatewayId",
                _ => "resourceConfigurationId",
            },
            value: id,
            form: Form::LatticeId(prefix),
        });
    }

    minted
}

/// Every minted value is of the form its resource type carries, and nothing else: its
/// class's prefix where it has one, and hex beyond it, so it can carry no text from the
/// request that asked for it.
fn assert_well_formed(minted: &Minted) {
    let Minted {
        case,
        member,
        value,
        form,
    } = minted;
    match form {
        Form::ClassId(prefix) => {
            let body = value.strip_prefix(prefix).unwrap_or_else(|| {
                panic!("case {case:?}: expected {member} to start {prefix:?}, got {value:?}")
            });
            assert_uuid_body(body, case, member);
        }
        Form::OpaqueId | Form::Token => {
            let expected = if *form == Form::Token {
                VERIFICATION_TOKEN_LEN
            } else {
                MINTED_ID_LEN
            };
            assert_eq!(
                value.len(),
                expected,
                "case {case:?}: expected a {expected}-character {member}, got {value:?}"
            );
            assert!(
                value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
                "case {case:?}: expected a lowercase hex {member}, got {value:?}"
            );
        }
        Form::LatticeId(prefix) => {
            let suffix = value.strip_prefix(prefix).unwrap_or_else(|| {
                panic!("case {case:?}: expected {member} to start {prefix:?}, got {value:?}")
            });
            assert_eq!(
                suffix.len(),
                LATTICE_ID_SUFFIX_LEN,
                "case {case:?}: expected a {LATTICE_ID_SUFFIX_LEN}-character suffix on \
                 {member}, got {value:?}"
            );
            assert!(
                suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
                "case {case:?}: expected a lowercase base-36 {member}, got {value:?}"
            );
        }
    }
}

/// RULE, first half: no two resources of one account are handed one minted value —
/// across resource TYPES as much as within one — and every minted value is of the form
/// its type carries.
#[tokio::test]
async fn no_two_resources_of_an_account_are_handed_one_minted_value() {
    let router = emulator();
    let minted = mint_every_resource(&router).await;
    assert!(
        minted.len() >= 14,
        "the table must drive every create path, got {} minted values",
        minted.len()
    );

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for value in &minted {
        assert_well_formed(value);
        assert!(
            seen.insert(value.value.clone()),
            "case {:?}: the {} {:?} was already handed to another resource of this \
             account; the minted values so far are {seen:?}",
            value.case,
            value.member,
            value.value
        );
    }
}

/// RULE, second half: a minted value is the mint's, not the ORDER of the calls — so two
/// accounts driven through byte-identical calls in the same order are handed no value in
/// common, and a client that creates the same resources twice can tell the two apart.
#[tokio::test]
async fn two_mints_hand_out_no_value_in_common() {
    let first_router = emulator();
    let second_router = emulator();
    let first = mint_every_resource(&first_router).await;
    let second = mint_every_resource(&second_router).await;
    assert_eq!(
        first.len(),
        second.len(),
        "the two accounts must have been driven through the same table, got {} and {} \
         minted values",
        first.len(),
        second.len()
    );

    let held: BTreeSet<&str> = first.iter().map(|value| value.value.as_str()).collect();
    for (one, other) in first.iter().zip(second.iter()) {
        assert_eq!(
            (one.case.as_str(), one.member),
            (other.case.as_str(), other.member),
            "the two walks of the table must line up, got {one:?} against {other:?}"
        );
        assert!(
            !held.contains(other.value.as_str()),
            "case {:?}: the second account's {} {:?} is a value the first account was \
             already handed, so it is the ordinal of the call rather than a minted value",
            other.case,
            other.member,
            other.value
        );
    }
}

/// RULE, applied to the one value that is a SECRET: the ownership token a target domain
/// is told to publish is that domain's own, so no other domain — and no other
/// verification method — is ever told to publish it, and it is state of the record
/// rather than a value a projection computes.
#[tokio::test]
async fn each_target_domain_is_told_to_publish_its_own_token() {
    let router = emulator();

    // The rows differ along what a shared token would otherwise be shared across: the
    // verification method, the domain name, and the SAME name registered twice.
    let rows = [
        (
            "the first domain, verified by DNS",
            "token-one.example.com",
            VERIFICATION_METHOD_DNS_TXT,
        ),
        (
            "the same domain name, verified by DNS again",
            "token-one.example.com",
            VERIFICATION_METHOD_DNS_TXT,
        ),
        (
            "the same domain name, verified over HTTP",
            "token-one.example.com",
            VERIFICATION_METHOD_HTTP_ROUTE,
        ),
        (
            "another domain, verified over HTTP",
            "token-two.example.com",
            VERIFICATION_METHOD_HTTP_ROUTE,
        ),
    ];

    let mut tokens: BTreeSet<String> = BTreeSet::new();
    for (case, domain, method) in rows {
        let created = ok(
            &router,
            "CreateTargetDomain",
            &json!({"targetDomainName": domain, "verificationMethod": method}),
            case,
        )
        .await;
        let id = required(&created, "targetDomainId", case);
        let token = challenge_token(&created).unwrap_or_else(|| {
            panic!("case {case:?}: a {method} registration carries a token: {created}")
        });
        assert!(
            tokens.insert(token.clone()),
            "case {case:?}: the token {token:?} was already handed to another domain, so \
             two domains are told to publish one challenge; the tokens so far are \
             {tokens:?}"
        );

        // The token is stored on the record, so every later read of that record reports
        // it unchanged — a verification call therefore compares against the domain's
        // own token.
        for attempt in ["the first read", "the second read"] {
            let read = ok(
                &router,
                "BatchGetTargetDomains",
                &json!({"targetDomainIds": [&id]}),
                case,
            )
            .await;
            assert_eq!(
                challenge_token(&single(&read, "targetDomains", case)),
                Some(token.clone()),
                "case {case:?}, {attempt}: expected the stored token {token:?}, got {read}"
            );
        }
    }
}
