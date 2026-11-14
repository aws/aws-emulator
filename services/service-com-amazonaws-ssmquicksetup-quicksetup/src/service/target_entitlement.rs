// SPDX-License-Identifier: CC0-1.0

//! Whether the resource ARN a route carries as a path label is one the caller is ENTITLED to,
//! and the refusal of a request that carries one it is not.
//!
//! Several routes of this endpoint address their resource by an ARN in the path: the manager's
//! own `/configurationManager/{ManagerArn}`, the tag plane's `/tags/{ResourceArn}`, and
//! `/configurationDefinition/{ManagerArn}/{Id}`, whose label is the ARN of the manager that
//! owns the definition addressed. An ARN names the ACCOUNT that owns the resource, so the
//! layer in front of this service decides, from the ARN alone, whether the caller may address
//! that resource at all: it must be a resource ARN of this service belonging to the account
//! the request is signed for. A request naming anything else asks to act on a resource that is
//! not the caller's — another account's, or a value that is no resource ARN of this service at
//! all — and is refused there, with [`crate::service::UNAUTHORIZED_STATUS`],
//! [`crate::service::UNAUTHORIZED_CODE`] and the one fixed sentence
//! [`crate::service::UNAUTHORIZED_MESSAGE`], before any handler reads or writes anything and
//! before the modelled constraint walk judges the request's members.
//!
//! The predicate is the ARN's ENTITLEMENT, never the store's contents: whether a resource
//! answers to an ARN the caller IS entitled to is a resource outcome, so such a request goes
//! straight through to its handler, which resolves the ARN against this service's state and
//! answers its own modelled `ResourceNotFoundException` (HTTP 404) when nothing answers to it.
//! A manager that has been torn down, one this endpoint never minted, and one another Region's
//! endpoint minted — all of them the caller's own account — are therefore the modelled 404 and
//! not this refusal.
//!
//! Only a resource ARN in the PATH is decided here. An ARN a request carries in its BODY
//! (`ListConfigurations`) is an argument the operation resolves, and a label that is no ARN
//! (`GetConfiguration`'s configuration id) is read by the operation that owns it.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The first path segment of every route whose NEXT segment is a resource ARN.
///
/// From this service's own routes (`smithy_query(query="digest")`):
/// `/configurationManager/{ManagerArn}` (GET, PUT, DELETE), `/tags/{ResourceArn}` (GET, PUT,
/// DELETE) and `/configurationDefinition/{ManagerArn}/{Id}` (PUT). Every other route of this
/// endpoint takes no resource ARN in its path.
const ARN_LABELLED_ROUTES: [&str; 3] = ["configurationManager", "tags", "configurationDefinition"];

/// The resource ARN `uri` carries as its path label, or `None` when the path is not one of the
/// ARN-labelled routes.
///
/// The PATH alone is read — a query string, which `UntagResource`'s `tagKeys` arrives in, is no
/// part of the label — and the label is percent-decoded: an AWS SDK encodes the `:` and `/` of
/// an ARN when it writes it into a single path segment, so the value compared below is the ARN
/// the caller meant rather than its encoded spelling.
fn addressed_arn(uri: &str) -> Option<String> {
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let route = segments.next()?;
    if !ARN_LABELLED_ROUTES.contains(&route) {
        return None;
    }
    let label = segments.next()?;
    Some(
        ::percent_encoding::percent_decode_str(label)
            .decode_utf8_lossy()
            .to_string(),
    )
}

/// Whether `arn` names a resource this caller may address.
///
/// An ARN of this service is `arn:<partition>:ssm-quicksetup:<region>:<account>:<resource>`
/// (the modelled `ManagerArn` pattern
/// `^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/<uuid>$` is the manager's
/// case of it). Entitlement is the ARN's own ACCOUNT: it must be the account the caller speaks
/// for, because a resource of another account is not this caller's to address. Neither the
/// Region the ARN names nor what its resource part names is decided here — which Region's
/// resources this endpoint holds, and whether any resource answers to the ARN, are the
/// questions the handler answers against the store.
fn is_entitled(arn: &str) -> bool {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, partition, service, region, account, resource] = fields.as_slice() else {
        return false;
    };
    *prefix == "arn"
        && partition.starts_with("aws")
        && *service == crate::state::ARN_SERVICE
        && !region.is_empty()
        && *account == ::engine::emulator_account_id()
        && !resource.is_empty()
}

/// Whether `req` addresses a resource ARN the caller is not entitled to, and so is one the
/// front door refuses.
///
/// A request whose path carries no resource ARN, and one whose ARN is the caller's own, are
/// both served by the operation the model's routing table names.
pub fn is_refused(req: &RtRequest) -> bool {
    addressed_arn(req.uri()).is_some_and(|arn| !is_entitled(&arn))
}

/// The front door's refusal of a request that addresses a resource ARN the caller is not
/// entitled to.
///
/// Held as a field on the service and handed back from `Service::override_operation`, which is
/// the seam that sees the request before the model's routing table dispatches it — so no
/// handler runs, nothing is read or written, and the modelled constraint walk never judges the
/// request's members.
pub struct UnauthorizedTargetOperation;

impl ErasedOperation for UnauthorizedTargetOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { crate::service::unauthorized(svc, req) })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` defers to the modelled operation the request's route names, so this refusal
        // is reached under exactly the auth the model declares for that route and cannot
        // weaken it.
        None
    }
}
