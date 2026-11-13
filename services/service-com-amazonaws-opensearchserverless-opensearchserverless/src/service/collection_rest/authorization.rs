// SPDX-License-Identifier: CC0-1.0

//! Whether a request may reach a collection's data plane at all, and whether the principal
//! it is signed by may perform what it asks.
//!
//! Two of the collection's own policies decide that, and AWS's own order is the order they
//! are read in here:
//!
//! 1. the `network` security policies, which say whether the endpoint is reachable — a
//!    collection no network policy covers answers nothing to anybody
//!    ([`crate::state::network_access`]);
//! 2. the `data` access policies, which say what the signing principal may do to the
//!    collection and its indexes ([`crate::state::data_access`]).
//!
//! The decision itself lives in those two state modules, because a data-access grant is
//! also asked about by a service that resolves a vector store it was handed. What lives
//! HERE is what only the REST surface knows: which principal a request speaks for, and
//! which permission the verb and route it asks for takes.
//!
//! # Which permission a request takes
//!
//! A data access policy grants named permissions on a `collection/<name>` or an
//! `index/<collection>/<index>` resource, so every route of this surface is one of those
//! resources plus one permission ([`needed`]). The mapping is AWS's own "Supported policy
//! permissions" table read against the OpenSearch REST route each permission describes —
//! `aoss:CreateIndex` for the index create, `aoss:DescribeIndex` for a read of an index's
//! own definition, `aoss:ReadDocument` for a search, a count or a document read,
//! `aoss:WriteDocument` for a document write, a bulk write or a refresh — so no route is
//! authorized under a permission the policy language does not have. A route this surface
//! serves nothing for takes no permission: it is answered the way the engine answers a path
//! it has no handler for, and nothing is authorized for it.
//!
//! # Which principal a request speaks for
//!
//! A data access policy names IAM principals, so the access key id the request signed with
//! has to be resolved to one: a key the account's IAM holds names the user that holds it,
//! and a key this emulator's STS minted for a role session names that session AND the role
//! it is a session of — the two spellings a grant may name it by, as every other reading of
//! a principal in this tree takes them.
//!
//! Three callers are NOT bounded by the data access policies, for the reasons every IAM
//! gate in this tree leaves them unbounded: the account's own published credential (the
//! position an AWS account root credential is in, which holds no IAM records at all), a
//! request this emulator can resolve no identity registry for (an engine carrying no STS or
//! IAM store), and an unsigned request, which presents no credential to resolve. The
//! NETWORK decision above is not about a principal and applies to every request, so an
//! unbounded caller is still refused at a collection no network policy covers.

use ::aws_smithy_runtime_api::http::Response as RtResponse;

use crate::state::{
    data_access, ComAmazonawsOpensearchserverlessOpensearchserverlessState as ServiceState,
};

/// The OpenSearch engine error type an authorization failure carries, which is the type
/// the engine's security layer answers one under.
const SECURITY_EXCEPTION: &str = "security_exception";

/// The reason a request to a collection the network policies do not cover is refused with.
///
/// The STATUS is the requirement — a client of the endpoint reads a 401 as an
/// authentication failure, which is what the recorded refusal of a collection with no
/// network policy is — and this names what was not admitted.
const UNAUTHORIZED: &str = "Unauthorized";

/// The reason a request whose principal no data access policy grants is refused with: the
/// sentence that refusal is reported by, as a service that forwards it names it (Amazon
/// Bedrock renders this leg as "error message: Bad Authorization").
const BAD_AUTHORIZATION: &str = "Bad Authorization";

/// Which resource of a collection a request addresses.
#[derive(Debug, PartialEq, Eq)]
enum Addressed {
    /// The collection itself.
    Collection,
    /// One named index of it.
    Index(String),
    /// Every index of it, which is what a collection-wide read or write reaches.
    EveryIndex,
}

impl Addressed {
    /// The `Resource` pattern a data access policy names this by.
    fn resource(&self, collection: &str) -> String {
        match self {
            Self::Collection => data_access::collection_resource(collection),
            Self::Index(index) => data_access::index_resource(collection, index),
            Self::EveryIndex => data_access::index_resource(collection, "*"),
        }
    }
}

/// Who a request speaks for: every spelling of the principal a grant may name it by, and
/// whether the data access policies bound it at all (see the module docs).
#[derive(Debug, Default)]
pub(super) struct Caller {
    /// The principal spellings, most specific first. Empty for an unbounded caller, and for
    /// a credential that speaks for nobody.
    principals: Vec<String>,
    /// Set for a caller the data access policies do not bound.
    unbounded: bool,
}

/// The access key id `authorization` presents, read by the emulator's own reading of a
/// SigV4 `Authorization` header so this surface states no part of that header's grammar.
fn access_key_id(authorization: Option<&str>) -> Option<String> {
    let value = ::http::HeaderValue::from_str(authorization?).ok()?;
    let mut headers = ::http::HeaderMap::new();
    headers.insert(::http::header::AUTHORIZATION, value);
    Some(::engine::extract_akid(&headers))
}

/// Who the request signed with `authorization` speaks for, resolved against the account's
/// own identity records in the SAME shared engine.
///
/// Call with NO guard of this service's store held: the STS and IAM stores are read here
/// and this service's is locked by its caller afterwards, so no two of them are ever held
/// at once.
pub(super) fn caller<Eng: Send + Sync + 'static>(engine: &Eng, authorization: Option<&str>) -> Caller {
    use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

    let unbounded = Caller {
        principals: Vec::new(),
        unbounded: true,
    };
    let Some(access_key_id) = access_key_id(authorization) else {
        // An unsigned request presents no credential to resolve.
        return unbounded;
    };
    let Some(iam) = sts_store::iam_state(engine) else {
        // No IAM store on this engine: no principal can hold a grant at all.
        return unbounded;
    };
    // A key this emulator's STS minted for a role session speaks for that session; one it
    // did not speaks for the IAM user that holds it, which is the identity STS itself
    // reports for it. Both are read through STS's own accessors, so this plane sees exactly
    // what `sts:GetCallerIdentity` would answer.
    if let Some(sts) = sts_store::service_state(engine) {
        let store = sts.lock();
        if let Some(session) = store.session(&access_key_id) {
            if session.is_expired(::std::time::SystemTime::now()) {
                // An expired session speaks for nobody: no grant names it.
                return Caller::default();
            }
            let mut principals = vec![session.identity.arn.clone()];
            if let Some(role_arn) = &session.role_arn {
                principals.push(role_arn.clone());
            }
            return Caller {
                principals,
                unbounded: false,
            };
        }
    }
    let identity = sts_store::long_term_identity(Some(&iam.lock()), &access_key_id);
    if identity.arn == sts_store::ambient_identity().arn {
        // The account's own published credential, which holds no IAM records.
        return unbounded;
    }
    Caller {
        principals: vec![identity.arn],
        unbounded: false,
    }
}

/// Why the collection `collection_name` answers `request` nothing, or `None` when both of
/// its own policies admit it.
///
/// Read with the store's guard held, before the request is served, so a policy just written
/// decides the very next request and one just deleted stops deciding immediately.
pub(super) fn refusal(
    store: &ServiceState,
    collection_name: &str,
    caller: &Caller,
    request: &super::Request<'_>,
) -> Option<RtResponse> {
    if !store.network_policy_admits(collection_name) {
        return Some(super::error(401, SECURITY_EXCEPTION, UNAUTHORIZED));
    }
    if caller.unbounded {
        return None;
    }
    let (addressed, permission) = needed(request)?;
    let resource = addressed.resource(collection_name);
    if store.data_access_grants(&caller.principals, &resource, permission) {
        return None;
    }
    Some(super::error(403, SECURITY_EXCEPTION, BAD_AUTHORIZATION))
}

/// The resource `request` addresses and the permission it takes, or `None` for a route this
/// surface serves nothing for.
///
/// One arm per route the surface dispatches ([`super::serve_resolved`]), so a route that is
/// served is a route that is authorized.
fn needed(request: &super::Request<'_>) -> Option<(Addressed, &'static str)> {
    /// Whether `segment` names an index rather than one of the engine's own reserved
    /// endpoints, as the dispatch reads it.
    fn an_index(segment: &str) -> bool {
        !segment.starts_with('_')
    }

    /// The indexes a collection-wide or wildcarded read addresses.
    fn indexes(segment: &str) -> Addressed {
        if an_index(segment) {
            Addressed::Index(segment.to_string())
        } else {
            Addressed::EveryIndex
        }
    }

    let segments = request.segments();
    let reading = matches!(request.method, "GET" | "HEAD");
    match segments.as_slice() {
        [] => Some((
            Addressed::Collection,
            data_access::permission::DESCRIBE_COLLECTION_ITEMS,
        )),
        ["_bulk"] => Some((
            Addressed::EveryIndex,
            data_access::permission::WRITE_DOCUMENT,
        )),
        ["_refresh"] => Some((
            Addressed::EveryIndex,
            data_access::permission::WRITE_DOCUMENT,
        )),
        ["_search"] | ["_count"] => Some((
            Addressed::EveryIndex,
            data_access::permission::READ_DOCUMENT,
        )),
        [index] if an_index(index) => {
            let permission = match request.method {
                "PUT" | "POST" => data_access::permission::CREATE_INDEX,
                "DELETE" => data_access::permission::DELETE_INDEX,
                _ => data_access::permission::DESCRIBE_INDEX,
            };
            Some((Addressed::Index((*index).to_string()), permission))
        }
        [index, "_doc"] if an_index(index) => Some((
            Addressed::Index((*index).to_string()),
            data_access::permission::WRITE_DOCUMENT,
        )),
        [index, "_doc", _id] | [index, "_create", _id] if an_index(index) => {
            let permission = if reading {
                data_access::permission::READ_DOCUMENT
            } else {
                data_access::permission::WRITE_DOCUMENT
            };
            Some((Addressed::Index((*index).to_string()), permission))
        }
        [index, "_mapping" | "_mappings" | "_settings"] if an_index(index) => {
            let permission = if reading {
                data_access::permission::DESCRIBE_INDEX
            } else {
                data_access::permission::UPDATE_INDEX
            };
            Some((Addressed::Index((*index).to_string()), permission))
        }
        [index, "_search" | "_count"] => Some((
            indexes(index),
            data_access::permission::READ_DOCUMENT,
        )),
        [index, "_refresh"] => Some((
            indexes(index),
            data_access::permission::WRITE_DOCUMENT,
        )),
        [index, "_bulk"] if an_index(index) => Some((
            Addressed::Index((*index).to_string()),
            data_access::permission::WRITE_DOCUMENT,
        )),
        _ => None,
    }
}
