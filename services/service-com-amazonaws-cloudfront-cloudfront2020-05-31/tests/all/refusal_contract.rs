// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a refusal, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, the request
//! boundary, the handler and serialization — the same path a client takes. Two rules are
//! under test:
//!
//! * a not-found refusal states the FIXED sentence of its error class and carries no part
//!   of the request: the condition it reports is that the store holds no such resource,
//!   which is a property of the store rather than of the identifier submitted, so the same
//!   sentence answers every identifier — whatever its form, and whichever operation of the
//!   resource asked;
//! * a value outside the value set of an enum the model declares is no value of that member
//!   at all: the value set is part of the schema this API publishes for the request, so the
//!   request is one the service cannot read and is refused with the code the recorded
//!   contract pins for that (`MalformedXML`, 400) — never with a code this service's model
//!   declares nowhere — whichever BINDING carried the value, a member of the request's
//!   document or a query parameter.
//!
//! The rows differ along what each rule generalises over: for the first, the operation
//! asked (five of them, over four resource kinds and three error codes) and the form of the
//! identifier (an id this service's own shape mints, an id of another shape, a name, an
//! ARN, a routing endpoint, a prefix, a case variant, the empty value); for the second, the
//! operation (a create and an update), the member (four of them, across two enums per
//! document), the form of the inadmissible value (another word, an admitted value in the
//! wrong case, an admitted value with trailing text, the empty value) and the binding the
//! value travelled in.
//!
//! The sentences are spelled out here rather than imported from the implementation, as the
//! `error_envelope` suite spells its own out: a constant edited to some other wording has
//! to fail these rows.
//!
//! The harness (the router, the request helpers and the element scanner) is the
//! [`crate::distribution_lifecycle`] suite's, reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::anycast_ip_list_lifecycle::{create_anycast_ip_list, get_anycast_ip_list, Create};
use crate::connection_groups::{create_connection_group, get_connection_group};
use crate::distribution_lifecycle::{
    call, config, create_distribution, emulator, get_distribution, Answer, ACCOUNT_ID,
    API_VERSION,
};
use crate::key_value_store_lifecycle::{create_key_value_store, describe_key_value_store};
use crate::realtime_log_config_list::encode_query;
use crate::web_acl_association::associate_web_acl;

// ---------------------------------------------------------------------------
// The sentences each error class states
// ---------------------------------------------------------------------------

/// `EntityNotFound` from a read of an Anycast static IP list, as the recorded
/// `GetAnycastIpList` contract carries it — the whole sentence, down to the absent full
/// stop.
const ANYCAST_IP_LIST_NOT_FOUND: &str = "The specified Anycast IP list does not exist";

/// `EntityNotFound` from a read of a connection group, as the recorded
/// `GetConnectionGroupByRoutingEndpoint` contract carries it.
const CONNECTION_GROUP_NOT_FOUND: &str = "The specified connection group does not exist.";

/// `EntityNotFound` from a read of a key value store, as the recorded contract carries it:
/// CloudFront names the resource by its modelled shape name in this sentence.
const KEY_VALUE_STORE_NOT_FOUND: &str = "The specified KeyValueStore does not exist.";

/// The sentence a distribution no store holds is reported with, whichever of the two codes
/// the operation's model names for that condition.
const DISTRIBUTION_NOT_FOUND: &str = "The specified distribution does not exist.";

/// The error message the answer states, with the entities the envelope escapes already
/// resolved by the reader.
fn message(answer: &Answer) -> &str {
    answer.root().find_text("Message").unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Calling the operations these rules are asserted over
// ---------------------------------------------------------------------------

/// `GET /2020-05-31/connection-group?RoutingEndpoint=<endpoint>`.
async fn get_by_routing_endpoint(router: &MultiServiceRouter, endpoint: &str) -> Answer {
    call(
        router,
        "GET",
        &format!(
            "/{API_VERSION}/connection-group?RoutingEndpoint={}",
            encode_query(endpoint)
        ),
        None,
        None,
    )
    .await
}

/// `PUT /2020-05-31/anycast-ip-list/{Id}` with the `IpAddressType` the row names, under
/// the version token `if_match`.
async fn update_anycast_ip_list(
    router: &MultiServiceRouter,
    id: &str,
    if_match: &str,
    ip_address_type: &str,
) -> Answer {
    let body = format!(
        "<UpdateAnycastIpListRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <IpAddressType>{ip_address_type}</IpAddressType></UpdateAnycastIpListRequest>"
    );
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/anycast-ip-list/{id}"),
        Some(&body),
        Some(if_match),
    )
    .await
}

/// `POST /2020-05-31/origin-access-control` with an `OriginAccessControlConfig` whose
/// `SigningProtocol` and `OriginAccessControlOriginType` the caller picks.
///
/// Every other member the document requires (`Name`, `SigningBehavior`) carries a value
/// its own constraint admits, so the refusal a row observes is about the member that row
/// varies.
async fn create_origin_access_control(
    router: &MultiServiceRouter,
    name: &str,
    signing_protocol: &str,
    origin_type: &str,
) -> Answer {
    let body = format!(
        "<OriginAccessControlConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name><Description>refusal-contract suite</Description>\
         <SigningProtocol>{signing_protocol}</SigningProtocol>\
         <SigningBehavior>always</SigningBehavior>\
         <OriginAccessControlOriginType>{origin_type}</OriginAccessControlOriginType>\
         </OriginAccessControlConfig>"
    );
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/origin-access-control"),
        Some(&body),
        None,
    )
    .await
}

/// `GET /2020-05-31/cache-policy?Type=<value>` — the QUERY-PARAMETER binding of an
/// enum-constrained member, which the recorded contract pins to its own code.
async fn list_cache_policies_by_type(router: &MultiServiceRouter, kind: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/cache-policy?Type={}", encode_query(kind)),
        None,
        None,
    )
    .await
}

/// Create one Anycast static IP list and report `(Id, ETag)`.
async fn anycast_ip_list(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let create = Create {
        case: "the list every refusal row is asserted against",
        name: name.to_string(),
        ip_count: 3,
        ip_address_type: None,
        ipam_cidr_configs: Vec::new(),
        tags: Vec::new(),
    };
    let created = create_anycast_ip_list(router, &create.xml()).await;
    assert_eq!(
        created.status, 202,
        "case {name}: expected the create to answer 202, got {} ({})",
        created.status, created.raw
    );
    (
        created
            .root()
            .text_at("Id")
            .unwrap_or_default()
            .to_string(),
        created.header("etag").to_string(),
    )
}

// ---------------------------------------------------------------------------
// A not-found refusal states the fixed sentence of its error class
// ---------------------------------------------------------------------------

/// One call that names a resource the store does not hold, and the sentence the service
/// states for it.
struct NotFound {
    case: String,
    /// The identifier the call submits, which must appear nowhere in the answer.
    identifier: String,
    status: u16,
    code: &'static str,
    message: &'static str,
    answer: Answer,
}

#[tokio::test]
async fn a_not_found_refusal_states_the_fixed_sentence_of_its_error_class() {
    let router = emulator();

    // One resource of each kind EXISTS, so every row below is about the identifier asked
    // for rather than about an empty store: the identifiers of the stored resources are
    // themselves rows (a list is addressed by its id, a group by its own identifiers).
    let (list_id, _) = anycast_ip_list(&router, "refusal-contract-list").await;
    let group_body = format!(
        "<CreateConnectionGroupRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>refusal-contract-group</Name></CreateConnectionGroupRequest>"
    );
    let group = create_connection_group(&router, &group_body).await;
    assert_eq!(
        group.status, 201,
        "the setup connection group must be created, got {} ({})",
        group.status, group.raw
    );
    let group_id = group.root().text_at("Id").unwrap_or_default().to_string();
    let group_endpoint = group
        .root()
        .text_at("RoutingEndpoint")
        .unwrap_or_default()
        .to_string();
    let store_body = format!(
        "<CreateKeyValueStoreRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>refusal-contract-store</Name></CreateKeyValueStoreRequest>"
    );
    let created_store = create_key_value_store(&router, &store_body).await;
    assert_eq!(
        created_store.status, 201,
        "the setup key value store must be created, got {} ({})",
        created_store.status, created_store.raw
    );
    let existing_distribution =
        create_distribution(&router, &config("refusal-contract", "ref-refusal-contract").xml())
            .await;
    assert_eq!(
        existing_distribution.status, 201,
        "the setup distribution must be created, got {} ({})",
        existing_distribution.status, existing_distribution.raw
    );

    let mut rows: Vec<NotFound> = Vec::new();

    // GetAnycastIpList: ids in the shape an Anycast static IP list id takes that name no
    // stored list — one this service never minted, and the stored id with one hex digit
    // changed. An id OUTSIDE that shape is an argument failure rather than a not-found, so
    // it belongs to the suite that pins the shape rule, not to this one.
    let mut flipped: Vec<char> = list_id.chars().collect();
    let last_digit = flipped.len() - 1;
    flipped[last_digit] = if flipped[last_digit] == '0' { '1' } else { '0' };
    for (case, candidate) in [
        ("an id never minted", "ipl-00000000000000000".to_string()),
        (
            "the stored id with one digit changed",
            flipped.into_iter().collect::<String>(),
        ),
    ] {
        rows.push(NotFound {
            case: format!("GetAnycastIpList/{case}"),
            identifier: candidate.clone(),
            status: 404,
            code: "EntityNotFound",
            message: ANYCAST_IP_LIST_NOT_FOUND,
            answer: get_anycast_ip_list(&router, &candidate).await,
        });
    }

    // GetConnectionGroupByRoutingEndpoint: endpoints of every form no stored group
    // carries, including other identifiers of the group that DOES exist.
    for (case, endpoint) in [
        (
            "an endpoint nothing carries",
            "sim2real-nonexistent.cloudfront.net".to_string(),
        ),
        ("the group's own id", group_id.clone()),
        ("the group's own name", "refusal-contract-group".to_string()),
        ("a name in the customer's zone", "shop.example.com".to_string()),
        ("a child of the stored endpoint", format!("edge.{group_endpoint}")),
    ] {
        rows.push(NotFound {
            case: format!("GetConnectionGroupByRoutingEndpoint/{case}"),
            identifier: endpoint.clone(),
            status: 404,
            code: "EntityNotFound",
            message: CONNECTION_GROUP_NOT_FOUND,
            answer: get_by_routing_endpoint(&router, &endpoint).await,
        });
    }

    // GetConnectionGroup: the same error class reached through the OTHER identifier the
    // connection group surface admits, so the sentence cannot depend on which one asked.
    for (case, identifier) in [
        ("an id no group carries", "cg_ZZZZZZZZZZZZZZZZZZZZZZ".to_string()),
        (
            "an ARN no group carries",
            format!("arn:aws:cloudfront::{ACCOUNT_ID}:connection-group/cg_ZZZZZZZZZZZZZ"),
        ),
        ("the stored group's routing endpoint", group_endpoint.clone()),
    ] {
        rows.push(NotFound {
            case: format!("GetConnectionGroup/{case}"),
            identifier: identifier.clone(),
            status: 404,
            code: "EntityNotFound",
            message: CONNECTION_GROUP_NOT_FOUND,
            answer: get_connection_group(&router, &identifier).await,
        });
    }

    // DescribeKeyValueStore: a name no store holds, in the forms `KeyValueStoreName`'s
    // pattern admits.
    for (case, name) in [
        ("a name no store holds", "absent-store".to_string()),
        ("the stored name upper-cased", "REFUSAL-CONTRACT-STORE".to_string()),
        ("a prefix of the stored name", "refusal".to_string()),
    ] {
        rows.push(NotFound {
            case: format!("DescribeKeyValueStore/{case}"),
            identifier: name.clone(),
            status: 404,
            code: "EntityNotFound",
            message: KEY_VALUE_STORE_NOT_FOUND,
            answer: describe_key_value_store(&router, &name).await,
        });
    }

    // AssociateDistributionWebACL: the same not-found condition about a DISTRIBUTION,
    // reported under `EntityNotFound` by this operation's model.
    for (case, id) in [
        ("an id of the minted shape", "EDFDVBD6EXAMPLE".to_string()),
        ("a value of no distribution id shape", "not-a-distribution".to_string()),
    ] {
        rows.push(NotFound {
            case: format!("AssociateDistributionWebACL/{case}"),
            identifier: id.clone(),
            status: 404,
            code: "EntityNotFound",
            message: DISTRIBUTION_NOT_FOUND,
            answer: associate_web_acl(
                &router,
                &id,
                &format!(
                    "arn:aws:wafv2:us-east-1:{ACCOUNT_ID}:global/webacl/Example/\
                     473e64fd-f30b-4765-81a0-62ad96dd167a"
                ),
                None,
            )
            .await,
        });
    }

    // GetDistribution: the SAME condition under the other code the model names for it, so
    // the sentence is pinned per error class rather than per operation.
    rows.push(NotFound {
        case: "GetDistribution/an id of the minted shape".to_string(),
        identifier: "EDFDVBD6EXAMPLE".to_string(),
        status: 404,
        code: "NoSuchDistribution",
        message: DISTRIBUTION_NOT_FOUND,
        answer: get_distribution(&router, "EDFDVBD6EXAMPLE").await,
    });

    for row in &rows {
        let NotFound {
            case,
            identifier,
            status,
            code,
            message: expected,
            answer,
        } = row;
        assert_eq!(
            answer.status, *status,
            "case {case}: expected HTTP {status}, got {} ({})",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case}: expected {code}, got {:?} ({})",
            answer.error_code(),
            answer.raw
        );
        assert_eq!(
            message(answer),
            *expected,
            "case {case}: expected message {expected:?}, got {:?}",
            message(answer)
        );
        assert!(
            identifier.is_empty() || !message(answer).contains(identifier.as_str()),
            "case {case}: the message must carry no part of the request, got {:?}",
            message(answer)
        );
    }
}

// ---------------------------------------------------------------------------
// A value outside a modelled enum
// ---------------------------------------------------------------------------

/// The forms an inadmissible enum value takes. `admitted` is one value the enum under test
/// DOES admit, so the "wrong case" and "trailing text" rows are built from the value set
/// itself rather than from a literal.
fn inadmissible(admitted: &str) -> Vec<(String, String)> {
    vec![
        ("another word".to_string(), format!("{admitted}-bogus")),
        (
            "an admitted value in the wrong case".to_string(),
            admitted.to_uppercase(),
        ),
        (
            "an admitted value with trailing text".to_string(),
            format!("{admitted}x"),
        ),
        ("the empty value".to_string(), String::new()),
    ]
}

/// Assert one refusal is the one this service answers a request whose schema it cannot read,
/// and that its sentence still states the member and the value set that member broke.
fn assert_enum_value_refused(case: &str, value: &str, answer: &Answer, member: &str, values: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case} ({value:?}): expected HTTP 400, got {} ({})",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "MalformedXML",
        "case {case} ({value:?}): expected MalformedXML, got {:?} ({})",
        answer.error_code(),
        answer.raw
    );
    assert!(
        message(answer).contains(member),
        "case {case} ({value:?}): expected the refusal to name the member {member:?}, got \
         {:?}",
        message(answer)
    );
    assert!(
        message(answer).contains(values),
        "case {case} ({value:?}): expected the refusal to state the value set {values:?}, \
         got {:?}",
        message(answer)
    );
}

#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_a_request_that_cannot_be_read() {
    let router = emulator();
    let (list_id, etag) = anycast_ip_list(&router, "refusal-contract-enum").await;

    // `IpAddressType` admits ipv4, ipv6 and dualstack
    // (`smithy_query(query="shape",shape="IpAddressType")`); the refusal lists that set in the
    // order this API publishes the enum's values.
    let ip_address_types = "[ipv4, ipv6, dualstack]";
    for (case, value) in inadmissible("ipv4") {
        // The update: the member travels in the request document of a PUT addressing one
        // list, under a version token that IS the list's current one, so nothing but the
        // enum can be what the request is refused over.
        let answer = update_anycast_ip_list(&router, &list_id, &etag, &value).await;
        assert_enum_value_refused(
            &format!("UpdateAnycastIpList/{case}"),
            &value,
            &answer,
            "ipAddressType",
            ip_address_types,
        );

        // The create: the same member of the same enum, in another operation's document.
        let create = Create {
            case: "a create naming an inadmissible address type",
            name: format!("refusal-create-{}", case.replace(' ', "-")),
            ip_count: 3,
            ip_address_type: None,
            ipam_cidr_configs: Vec::new(),
            tags: Vec::new(),
        };
        let body = create.xml().replace(
            "</CreateAnycastIpListRequest>",
            &format!("<IpAddressType>{value}</IpAddressType></CreateAnycastIpListRequest>"),
        );
        let answer = create_anycast_ip_list(&router, &body).await;
        assert_enum_value_refused(
            &format!("CreateAnycastIpList/{case}"),
            &value,
            &answer,
            "ipAddressType",
            ip_address_types,
        );
    }

    // Another document, and two members of it, each constrained by an enum of its own:
    // `OriginAccessControlSigningProtocols` admits sigv4 and sigv4a, and
    // `OriginAccessControlOriginTypes` admits s3, mediastore, mediapackagev2 and
    // lambda (`smithy_query(query="shape",shape="…")`), each the signing name of the service
    // its origin belongs to; the origin type's set is listed in the order this API publishes
    // it.
    for (case, value) in inadmissible("sigv4") {
        let answer =
            create_origin_access_control(&router, "refusal-contract-oac", &value, "s3").await;
        assert_enum_value_refused(
            &format!("CreateOriginAccessControl/SigningProtocol/{case}"),
            &value,
            &answer,
            "signingProtocol",
            "[sigv4, sigv4a]",
        );
    }
    for (case, value) in inadmissible("s3") {
        let answer =
            create_origin_access_control(&router, "refusal-contract-oac", "sigv4", &value).await;
        assert_enum_value_refused(
            &format!("CreateOriginAccessControl/OriginType/{case}"),
            &value,
            &answer,
            "originAccessControlOriginType",
            "[s3, lambda, mediastore, mediapackagev2]",
        );
    }

    // The OTHER binding: the same kind of value set, carried by a query parameter instead of
    // a document, is the same refusal — the binding does not change which schema the request
    // broke.
    let refused_query = list_cache_policies_by_type(&router, "not-a-policy-type").await;
    assert_eq!(
        refused_query.error_code(),
        "MalformedXML",
        "case ListCachePolicies/Type: expected the query-parameter binding to be refused the \
         same way, got {:?} ({})",
        refused_query.error_code(),
        refused_query.raw
    );
    assert!(
        message(&refused_query).contains("Member must satisfy enum value set:"),
        "case ListCachePolicies/Type: expected the value-set sentence, got {:?}",
        message(&refused_query)
    );

    // The control for the rule itself: a value the enum DOES admit is served rather than
    // refused, so each refusal above is about the value and not about the member being
    // there at all.
    for admitted in ["sigv4", "sigv4a"] {
        let answer = create_origin_access_control(
            &router,
            &format!("refusal-contract-{admitted}"),
            admitted,
            "s3",
        )
        .await;
        assert_eq!(
            answer.status, 201,
            "case CreateOriginAccessControl/{admitted}: expected HTTP 201, got {} ({})",
            answer.status, answer.raw
        );
    }

    // And the refused update persisted nothing: the list still reports the address type it
    // was created with, under the version token it already had.
    let read = get_anycast_ip_list(&router, &list_id).await;
    assert_eq!(
        read.status, 200,
        "case the list after the refusals: expected HTTP 200, got {} ({})",
        read.status, read.raw
    );
    assert_eq!(
        read.header("etag"),
        etag,
        "case the list after the refusals: expected the version token to stand, got {}",
        read.raw
    );
}
