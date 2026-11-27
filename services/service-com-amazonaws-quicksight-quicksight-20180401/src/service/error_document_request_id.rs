// SPDX-License-Identifier: CC0-1.0

//! Which of this service's refusals state the request id of the call in their error
//! DOCUMENT.
//!
//! RULE: every failure of this service reports the id of the call in the
//! `x-amzn-RequestId` response header ([`super::wire`]); its error DOCUMENT states that
//! same id, in the `RequestId` member its exception shapes declare, exactly when the call
//! was addressed to a route SCOPED TO A NAMESPACE —
//! `/accounts/{AwsAccountId}/namespaces/{Namespace}/…`, the identity surface, which serves
//! a namespace itself and the users, groups, group memberships, role memberships and IAM
//! policy assignments inside it. A call addressed to an account-scoped route
//! (`/accounts/{AwsAccountId}/data-sources/…`, `/folders/…`, `/analyses/…`, `/themes/…`)
//! reports the id in the header alone, and its document is the members its class fills.
//!
//! The two surfaces are the model's own: the route of every operation is its `@http` uri
//! (`smithy_query(query="operation",operation="DescribeGroup,DescribeIAMPolicyAssignment,
//! DescribeUser,DescribeDataSource,CreateFolder",
//! model_path="services/service-com-amazonaws-quicksight-quicksight-20180401/smithy-model/com-amazonaws-quicksight-quicksight-20180401.json")`),
//! and `RequestId` is OPTIONAL in every exception shape that declares it, so both documents
//! are ones the model allows.
//!
//! A SUCCESS is untouched here, on either surface: every output shape declares `RequestId`
//! as a body member and an answer that succeeds states it there.
//!
//! # Why it is written here
//!
//! The codec seam a service overrides is handed the error, its shape, its code and its
//! status and NOTHING about the request
//! (`aws_smithy_schema_server::protocol::ServerProtocol::serialize_error`), and `vendor/` is
//! read-only, so the surface a refusal was addressed to cannot be read there. So the header
//! half of the rule lives in that seam — the one place every modelled refusal passes, and
//! the one place the id the handler allocated can be read off the document as it is written
//! — and the document half lives HERE, at the one seam that sees the REQUEST: the operation
//! wrapper [`smithy_server_runtime::Service::override_operation`] hands back
//! ([`NamespaceScopedRequestIdOperation`]).
//!
//! It RESTATES the id the answer already reports in its header and mints nothing, so the
//! header and the document of one answer can never name different ids, and an answer that
//! reports no id — a refusal rendered before a handler ran, whose synthetic shape declares
//! no request-id member — keeps the document it has.
//!
//! # What the rule is read off, and what it INFERS
//!
//! The surfaces that have been OBSERVED are a group create and read and an IAM policy
//! assignment read, whose refusals state the id in their document, and a data source create
//! and read and a folder create and read, whose refusals do not. The route scoping is what
//! separates those two sets, and it is what every other route is then decided by — so a
//! route no answer has been observed for (a group membership, a namespace's own read, a
//! role membership) follows the rule by INFERENCE rather than by observation. An observation
//! that contradicts one of those is new evidence about where the line falls, not a
//! regression of this module: the thing to change then is this predicate, in one place, for
//! every route on the surface it moves.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

use super::wire::{REQUEST_ID_HEADER, REQUEST_ID_MEMBER};

/// The first path segment of every route this service serves — an account's.
const ACCOUNTS_SEGMENT: &str = "accounts";

/// The segment every route of the identity surface scopes its resource under, and the
/// 0-based position it sits at: `/accounts/{AwsAccountId}/namespaces/{Namespace}/…`, so the
/// account id is at 1, this segment at 2 and the namespace's own label at 3.
const NAMESPACES_SEGMENT: &str = "namespaces";
const NAMESPACES_SEGMENT_AT: usize = 2;

/// Whether `path` addresses a resource of ONE namespace: the account's `namespaces`
/// collection with the namespace's own label after it.
///
/// The POSITION is part of it, not just the word: the listing of an account's namespaces
/// (`/accounts/{AwsAccountId}/namespaces`) and the create that takes the name in its body
/// (`/accounts/{AwsAccountId}`) address the ACCOUNT, and a resource of the account whose own
/// id happens to be that word (`/accounts/{AwsAccountId}/data-sets/namespaces/…`) addresses
/// no namespace either. The label itself may be the empty string, which `Namespace` admits
/// (`length 0..64`), so what is required after it is that a segment is THERE rather than
/// that it is non-empty.
fn names_a_namespace(path: &str) -> bool {
    let segment = |index: usize| ::service_common::nth_path_segment(path, index);
    segment(0) == Some(ACCOUNTS_SEGMENT)
        && segment(NAMESPACES_SEGMENT_AT) == Some(NAMESPACES_SEGMENT)
        && segment(NAMESPACES_SEGMENT_AT + 1).is_some()
}

/// `resp` with the id it already reports in [`REQUEST_ID_HEADER`] restated in the
/// [`REQUEST_ID_MEMBER`] member of its JSON error document.
///
/// Nothing is minted: a response reporting no id in that header is returned exactly as the
/// renderer wrote it. So are a success, a document that is not a JSON object and one that
/// already states the member — the shared writer owns that floor, so this module states
/// which id and which member and nothing else.
fn with_document_request_id(resp: RtResponse) -> RtResponse {
    let Some(request_id) = resp.headers().get(REQUEST_ID_HEADER).map(str::to_string) else {
        return resp;
    };
    ::service_common::query_error_envelope::with_stated_request_id_error_member_named(
        resp,
        &request_id,
        REQUEST_ID_MEMBER,
    )
}

/// Serves the modelled operation this service's routing table names for a request exactly
/// as that table would, then states the request id in the error document of a refusal whose
/// surface states it there (see the module docs).
///
/// It stores no operation of its own: the one it delegates to is the one the routing table
/// names for the request, so the framework still owns the whole deserialize → validate →
/// handle → serialize sequence and every status, code and message it answers with, while
/// this wrapper owns one member of one document.
pub struct NamespaceScopedRequestIdOperation;

impl ErasedOperation for NamespaceScopedRequestIdOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The operation this service's own generated routing table names for the
            // request, resolved through the shared lookup every such wrapper uses
            // (`service_common::query_error_envelope`), so no routing is restated here.
            let answer = match ::service_common::query_error_envelope::modeled_operation(svc, req)
            {
                Some(op) => op.run(svc, req, ctx).await,
                // Unreachable through a `Service::override_operation` that claims a request
                // only when the routing table holds its key; answered as the framework
                // answers an unroutable request rather than guessed at.
                None => svc.definition().routing_error_response(req),
            };
            if names_a_namespace(::service_common::request_path(req.uri())) {
                with_document_request_id(answer)
            } else {
                answer
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — the operation
        // this wrapper delegates to, so its model-declared schemes decide and this wrapper
        // cannot weaken them.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::names_a_namespace;

    /// The two surfaces, read off the routes the model declares: a route scoped to a
    /// namespace is the identity surface, every other is the account's.
    /// Source: smithy_query(query="operation", operation="DescribeGroup,CreateGroup,
    /// DescribeUser,CreateRoleMembership,DescribeIAMPolicyAssignment,DescribeNamespace,
    /// ListNamespaces,CreateNamespace,DescribeDataSource,CreateFolder,ListFolders").
    #[test]
    fn a_route_is_the_identity_surface_exactly_when_it_scopes_under_a_namespace() {
        let rows = [
            ("DescribeGroup", "/accounts/000000000000/namespaces/default/groups/analysts", true),
            ("CreateGroup", "/accounts/000000000000/namespaces/default/groups", true),
            ("DescribeUser", "/accounts/000000000000/namespaces/sales/users/ann", true),
            (
                "CreateRoleMembership",
                "/accounts/000000000000/namespaces/default/roles/ADMIN/members/analysts",
                true,
            ),
            (
                "DescribeIAMPolicyAssignment",
                "/accounts/000000000000/namespaces/default/iam-policy-assignments/a1",
                true,
            ),
            ("DescribeNamespace", "/accounts/000000000000/namespaces/default", true),
            // A namespace whose name is the empty string the model admits is still one
            // namespace's route.
            ("DescribeNamespace of the empty name", "/accounts/000000000000/namespaces/", true),
            ("ListNamespaces", "/accounts/000000000000/namespaces", false),
            ("CreateNamespace", "/accounts/000000000000", false),
            ("DescribeDataSource", "/accounts/000000000000/data-sources/sales", false),
            ("CreateFolder", "/accounts/000000000000/folders/reports", false),
            ("ListFolders", "/accounts/000000000000/folders", false),
            // A resource of the account whose own id reads like the segment: the surface
            // turns on the segment AT the namespace position, and this route scopes its
            // resource under the account instead.
            ("a folder named like the segment", "/accounts/000000000000/folders/namespaces", false),
            (
                "a data set named like the segment, with a sub-resource after it",
                "/accounts/000000000000/data-sets/namespaces/refresh-properties",
                false,
            ),
            ("a path of another shape entirely", "/__admin/health", false),
        ];
        for (case, path, expected) in rows {
            assert_eq!(
                names_a_namespace(path),
                expected,
                "case {case:?}: expected names_a_namespace({path:?}) to be {expected}, got {}",
                names_a_namespace(path)
            );
        }
    }
}
