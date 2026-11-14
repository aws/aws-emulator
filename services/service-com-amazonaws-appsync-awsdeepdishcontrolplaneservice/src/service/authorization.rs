// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step taken BEFORE the operation it names is dispatched.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at the
//! request's own input, so a caller whose identity-based policies do not grant the action
//! — or that an explicit `Deny` covers — is told it is not authorized rather than told
//! anything about the resource it named, and the call performs nothing: no resource is
//! minted, no status moves, nothing is read back. This service models that refusal as
//! `AccessDeniedException` (`@error("client")`, `@httpError(403)`,
//! `smithy_query(query="shape",shape="AccessDeniedException")`), and it is the answer to
//! EVERY action this service publishes ([`GATED_OPERATIONS`], which is the whole bound
//! operation set, `smithy_query(query="list_operations")`).
//!
//! The decision belongs to the authorization layer in FRONT of the service rather than to
//! the operation behind it, so which operations it judges does not follow from the error
//! set an operation declares: an operation's modelled errors are the ones the SERVICE
//! raises once it is reached, and a caller IAM refuses never reaches it. Judging only the
//! operations that happen to declare the shape would serve an unauthorized caller the very
//! resources the policy withholds — a collection read among them — so the decision is
//! taken for all of them alike.
//!
//! The decision is a property of the request's identity, the action it names and the
//! resource it addresses rather than of what a handler goes on to do, so it is taken ONCE
//! — in the layer every gated operation is served behind ([`Gated`], wired in
//! `crate::service::EmulatedService::override_operation`) — and nowhere else. That is also
//! what makes it fire for an operation that is still a scaffold: such a caller is refused
//! before the 501, exactly as AWS refuses it before running anything.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same record
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    owns the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`),
//!    where the collection rule itself lives — so a document `iam:PutUserPolicy` wrote
//!    takes effect on the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny. The refusal's WORDING comes from that module too, for the
//!    same reason the decision does.
//!
//! Only the store LOOKUPS live here; neither the decision nor its sentence does. The
//! lookups stay in this crate because hoisting them into `service-common` would make every
//! service that depends on that crate link the IAM and STS service crates. WHICH IAM entity
//! a principal ARN names carries no such dependency, so it is read through the shared
//! [`::service_common::iam_role`] rather than parsed again here.
//!
//! AppSync publishes no resource-based policy of its own — the model declares no policy
//! operation and no policy member — so the decision here is the identity-based one alone.
//! The resource a request is judged against is the one its OWN URI addresses
//! ([`judged_resource`]), built with this service's own ARN spellings, so a policy written
//! about a domain, an API or a channel namespace covers exactly the calls that address it.
//! A request that addresses a sub-resource this service publishes no ARN spelling for
//! carries no such ARN to be judged by, so the resource is the request's own URI path hung
//! off this service's ARN prefix ([`Addressed::Path`]) — which is the resource IAM names in
//! the refusal, and the one a policy has to state to lift it.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root
//!   credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to this account, so the IAM store holds no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits.
//!   An explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused. (A session the
//! authenticating layer has already refused never reaches a handler at all.)
//!
//! LOCK ORDER: the STS registry's lock and then the IAM store's, each taken and released
//! in turn BEFORE this service's own store lock is taken — the decision is reached ahead of
//! dispatch, so no handler of this service holds its store while this runs. Neither of
//! those services reads this one's state.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_schema_server::ProtocolError;
use ::aws_smithy_types::body::SdkBody;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::service_common::validation::SubmittedEcho;
use ::smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Refusal, ServerHttpProtocol, Service,
    Violation,
};

use crate::ops::types::AccessDeniedException;

/// This service's IAM action prefix: an action is `appsync:<OperationName>` ("Actions
/// defined by AWS AppSync", IAM Service Authorization Reference), which is this service's
/// `arnNamespace` / `endpointPrefix` (`smithy_query(query="service_info")`) — the same
/// string [`crate::state::ARN_SERVICE`] spells into every ARN this service builds.
pub const ACTION_PREFIX: &str = crate::state::ARN_SERVICE;

/// The resource an action that addresses no resource of its own is judged against, as IAM
/// writes it for such a request — and, handed to one of this service's resource-path
/// builders, the identifier a call that addresses a COLLECTION rather than one member of it
/// is judged under.
pub const ANY_RESOURCE: &str = "*";

/// The wire name of the refusal this gate answers with, and the status the model gives it:
/// `AccessDeniedException`, `@error("client")` at `@httpError(403)`
/// (`smithy_query(query="shape",shape="AccessDeniedException")`).
pub const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The media type this restJson1 service renders an error in.
const JSON_CONTENT_TYPE: &str = "application/json";

/// Every action this service publishes, i.e. the whole set this gate is asked about, as
/// `Operation::NAME` spells each one — which is also the name of the IAM action
/// (`appsync:<OperationName>`) a policy states about it.
///
/// Source: the operations bound to the service shape
/// (`smithy_query(query="list_operations")`); the list is asserted against the model by
/// this module's own test, so an operation the model gains is judged rather than quietly
/// served to a caller no policy allows.
pub const GATED_OPERATIONS: &[&str] = &[
    "AssociateApi",
    "AssociateMergedGraphqlApi",
    "AssociateSourceGraphqlApi",
    "CreateApi",
    "CreateApiCache",
    "CreateApiKey",
    "CreateChannelNamespace",
    "CreateDataSource",
    "CreateDomainName",
    "CreateFunction",
    "CreateGraphqlApi",
    "CreateResolver",
    "CreateType",
    "DeleteApi",
    "DeleteApiCache",
    "DeleteApiKey",
    "DeleteChannelNamespace",
    "DeleteDataSource",
    "DeleteDomainName",
    "DeleteFunction",
    "DeleteGraphqlApi",
    "DeleteResolver",
    "DeleteType",
    "DisassociateApi",
    "DisassociateMergedGraphqlApi",
    "DisassociateSourceGraphqlApi",
    "EvaluateCode",
    "EvaluateMappingTemplate",
    "FlushApiCache",
    "GetApi",
    "GetApiAssociation",
    "GetApiCache",
    "GetChannelNamespace",
    "GetDataSource",
    "GetDataSourceIntrospection",
    "GetDomainName",
    "GetFunction",
    "GetGraphqlApi",
    "GetGraphqlApiEnvironmentVariables",
    "GetIntrospectionSchema",
    "GetResolver",
    "GetSchemaCreationStatus",
    "GetSourceApiAssociation",
    "GetType",
    "ListApiKeys",
    "ListApis",
    "ListChannelNamespaces",
    "ListDataSources",
    "ListDomainNames",
    "ListFunctions",
    "ListGraphqlApis",
    "ListResolvers",
    "ListResolversByFunction",
    "ListSourceApiAssociations",
    "ListTagsForResource",
    "ListTypes",
    "ListTypesByAssociation",
    "PutGraphqlApiEnvironmentVariables",
    "StartDataSourceIntrospection",
    "StartSchemaCreation",
    "StartSchemaMerge",
    "TagResource",
    "UntagResource",
    "UpdateApi",
    "UpdateApiCache",
    "UpdateApiKey",
    "UpdateChannelNamespace",
    "UpdateDataSource",
    "UpdateDomainName",
    "UpdateFunction",
    "UpdateGraphqlApi",
    "UpdateResolver",
    "UpdateSourceApiAssociation",
    "UpdateType",
];

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The resource the request's URI addresses, in the form the decision needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Addressed {
    /// A resource of THIS service, as the resource portion of its ARN — the same spelling
    /// the operation that creates it mints.
    Resource(String),
    /// An ARN the request itself carries, which is already the whole resource: the tagging
    /// surface addresses its target that way (`/v1/tags/{resourceArn}`).
    Arn(String),
    /// A sub-resource of this service that has no ARN spelling of its own, addressed by the
    /// request's own URI path — version prefix and leading `/` included — as the resource
    /// portion of this service's ARN.
    Path(String),
    /// The request names no single resource of its own — a listing, or a sandbox
    /// evaluation — so the action is judged against every resource of this service the
    /// account holds in the region addressed, which is how IAM writes the resource of such
    /// a request.
    Account,
}

/// The child space of an API that carries its environment variables, and the one child
/// space of this service that no ARN spelling of its own covers — neither this service's
/// own ARN builders nor the resource types IAM publishes for it name an environment
/// variable set — so a call addressing it is judged by its URI path ([`Addressed::Path`]).
///
/// Source: `smithy_query(query="operation",
/// operation="GetGraphqlApiEnvironmentVariables,PutGraphqlApiEnvironmentVariables")` — both
/// are bound to `/v1/apis/{apiId}/environmentVariables`, and no other operation of this
/// service is bound under this segment.
const ENVIRONMENT_VARIABLES_SEGMENT: &str = "environmentVariables";

/// The member a create names the custom domain it is about to create by, which is where
/// that request carries the resource its URI does not.
///
/// Source: `smithy_query(query="operation", operation="CreateDomainName")` — `domainName`,
/// `@required`, the same member `GetDomainName` binds to its path label.
const DOMAIN_NAME_MEMBER: &str = "domainName";

/// The ARN the request NAMES, which is what the action is judged against.
///
/// Read from the REQUEST rather than from a stored record, so the decision does not depend
/// on whether that resource exists — which is also what keeps a refused call from telling
/// an unauthorized caller whether it does. A CREATE therefore names the resource it is
/// about to create: it is judged against that resource's own ARN, not against the
/// collection it will join.
#[must_use]
pub fn judged_resource(ctx: &OperationContext<'_>, req: &RtRequest) -> String {
    let region = crate::state::request_region(ctx);
    match addressed(
        ::service_common::request_path(req.uri()),
        named_resources(req).as_ref(),
    ) {
        // Already whole: the tagging surface carries the ARN of its target.
        Addressed::Arn(arn) => arn,
        // Both are the resource portion of an ARN of this service: the spelling its own
        // builders mint for a resource that has one, the request's URI path for a
        // sub-resource that has none.
        Addressed::Resource(resource) | Addressed::Path(resource) => {
            ::engine::emulator_arn(ACTION_PREFIX, &region, &resource)
        }
        Addressed::Account => ::engine::emulator_arn(ACTION_PREFIX, &region, ANY_RESOURCE),
    }
}

/// The request's own input, as the JSON object a restJson1 body carries, or `None` for a
/// request with no readable body — a GET, or a body no reader can make an object of, which
/// is a request the framework's own walk refuses.
///
/// Only the member that NAMES a resource is read out of it ([`addressed`]): the decision
/// rests on the action and the resource, and nothing else of the body takes part.
fn named_resources(req: &RtRequest) -> Option<::serde_json::Map<String, ::serde_json::Value>> {
    let body = req.body().bytes()?;
    match ::serde_json::from_slice(body) {
        Ok(::serde_json::Value::Object(members)) => Some(members),
        _ => None,
    }
}

/// The string member `member` of the request's input, when it carries one.
fn named(
    input: Option<&::serde_json::Map<String, ::serde_json::Value>>,
    member: &str,
) -> Option<String> {
    input?
        .get(member)?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// What the request addresses: its URI space first, and the member its INPUT names a
/// resource by where the URI carries no label for one.
///
/// The spaces this service's operations are bound to (`smithy_query(query="digest")`):
/// `/v{n}/domainnames…` (a custom domain name and its API association), `/v{n}/apis…` (a
/// GraphQL API, an Event API, and the children each addresses under it — a data source, a
/// function, a type, a resolver on a field of a type, a channel namespace), the
/// `/v1/{mergedApis,sourceApis}/{identifier}…` association spaces (whose label identifies
/// one side's API, by its id or by its ARN) and `/v1/tags/{resourceArn}` (the tagging
/// surface, whose label IS the target's ARN). Anything else — the two sandbox evaluation
/// paths and the data-source introspection space among them — names no resource of this
/// service.
///
/// A child is named under its API's own `apis/{apiId}` path, as this service's own ARN
/// builders spell it (`crate::state::data_source_resource` and the rest), so a policy
/// written about one covers exactly the calls that address it and a policy written about
/// `apis/{apiId}/*` covers every child of that API. A call that addresses a child
/// COLLECTION rather than one member of it is judged against the parent it collects under,
/// which is the resource such a call names.
///
/// A child that no ARN spelling covers ([`ENVIRONMENT_VARIABLES_SEGMENT`]) is named by the
/// request's own URI path instead: there is no ARN of it to judge the call by, and the path
/// is the only spelling that identifies it — so a policy written about the parent API does
/// not reach it, and one written about the path does.
fn addressed(
    path: &str,
    input: Option<&::serde_json::Map<String, ::serde_json::Value>>,
) -> Addressed {
    let segment = |index: usize| {
        ::service_common::nth_path_segment(path, index)
            .filter(|segment| !segment.is_empty())
            .map(::service_common::percent_decode_path_label)
    };
    match segment(1).as_deref() {
        Some("domainnames") => match segment(2).or_else(|| named(input, DOMAIN_NAME_MEMBER)) {
            Some(domain_name) => {
                Addressed::Resource(crate::state::domain_name_resource(&domain_name))
            }
            // The collection path carrying no domain name at all: a listing, which names no
            // one resource.
            None => Addressed::Account,
        },
        Some("apis") => {
            // An API is addressed by the id the SERVICE minted for it, so a request that
            // carries none names no API: a create names the API it will mint an id for only
            // once that id exists.
            let Some(api_id) = segment(2) else {
                return Addressed::Account;
            };
            // The one child space with no ARN spelling of its own, which the request's URI
            // path identifies instead. Rebuilt from the decoded segments, so the resource
            // the decision is taken about does not depend on how the caller escaped them.
            if segment(3).as_deref() == Some(ENVIRONMENT_VARIABLES_SEGMENT) {
                let version = segment(0).unwrap_or_default();
                return Addressed::Path(format!(
                    "/{version}/apis/{api_id}/{ENVIRONMENT_VARIABLES_SEGMENT}"
                ));
            }
            Addressed::Resource(match (segment(3).as_deref(), segment(4)) {
                (Some("channelNamespaces"), Some(name)) => {
                    crate::state::channel_namespace_resource(&api_id, &name)
                }
                (Some("datasources"), Some(name)) => {
                    crate::state::data_source_resource(&api_id, &name)
                }
                (Some("functions"), Some(function_id)) => {
                    crate::state::function_resource(&api_id, &function_id)
                }
                (Some("types"), Some(type_name)) => match (segment(5).as_deref(), segment(6)) {
                    (Some("resolvers"), Some(field_name)) => {
                        crate::state::resolver_resource(&api_id, &type_name, &field_name)
                    }
                    _ => crate::state::type_resource(&api_id, &type_name),
                },
                // Every other child space — the API keys, the cache, the schema, the
                // source-API associations — is addressed under the API itself, which is the
                // resource those calls name.
                _ => crate::state::api_resource(&api_id),
            })
        }
        // One side's API, named by either form its `…Identifier` label admits
        // ([`crate::state::is_api_identifier`]): an ARN is already the whole resource, an
        // API id is the resource portion of one.
        Some("mergedApis" | "sourceApis") => match segment(2) {
            Some(identifier) if identifier.starts_with(crate::state::ARN_PREFIX) => {
                Addressed::Arn(identifier)
            }
            Some(api_id) => Addressed::Resource(crate::state::api_resource(&api_id)),
            None => Addressed::Account,
        },
        Some("tags") => match segment(2) {
            Some(arn) => Addressed::Arn(arn),
            None => Addressed::Account,
        },
        _ => Addressed::Account,
    }
}

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `resource` the ARN the request
/// ADDRESSES.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise. Taken BEFORE the store is reached, so
/// a refused call writes nothing.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises
/// the session expiries the clock has already made due; that write is owed at the same
/// instant whether or not anybody called, which is why a REPORTING operation of this
/// service may take this decision too.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, and this service publishes no resource-based policy, so the decision
    // below rests on the action and the resource.
    let request = iam::Request::new(&action, resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        ));
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            &iam::no_identity_policy_allows(&action),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow is
    // not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => iam::no_permissions_boundary_allows(&action),
        };
        return Err(not_authorized(&caller.arn, &action, resource, &reason));
    }
    Ok(())
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which of IAM's rules refused it — the one sentence every authorizing
/// service in this tree words it as ([`iam::not_authorized_by_identity_policy`]). Every
/// value in it comes from the request and from the stored policies.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(iam::not_authorized_by_identity_policy(
            principal, action, resource, reason,
        )),
    }
}

/// The wire answer to a request this gate refused: the modelled `AccessDeniedException` at
/// its modelled 403, serialized through the SAME protocol a handler-returned error of that
/// shape goes through, so a refusal taken in the layer and one taken in a handler are
/// indistinguishable on the wire.
///
/// The hand-built fallback carries the same three things a client branches on — the status,
/// the shape in `x-amzn-errortype` and the `__type` body — for the one case the serializer
/// can fail: a value it cannot render.
#[must_use]
pub fn refusal_response(
    protocol: &dyn ServerHttpProtocol,
    denial: &AccessDeniedException,
) -> RtResponse {
    protocol
        .serialize_error(
            denial,
            &crate::ops::types::SCHEMA_ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_STATUS,
        )
        .unwrap_or_else(|_| {
            let body = ::serde_json::json!({
                "__type": ACCESS_DENIED_EXCEPTION,
                "message": denial.message.clone().unwrap_or_default(),
            });
            let mut response = RtResponse::new(
                StatusCode::try_from(ACCESS_DENIED_STATUS)
                    .expect("the status is a constant of this module"),
                SdkBody::from(body.to_string()),
            );
            response
                .headers_mut()
                .insert("content-type", JSON_CONTENT_TYPE);
            response
                .headers_mut()
                .insert("x-amzn-errortype", ACCESS_DENIED_EXCEPTION);
            response
        })
}

/// One modelled operation, served behind IAM's decision about the action it names.
///
/// It carries the operation TYPE — which is what names the action and resolves the delegate
/// — and a handle on the ONE shared engine, which is what the decision reads the caller's
/// identity and its policies through. Every request it does not refuse is delegated to the
/// one operation instance the generated routing table built from that same engine, so there
/// is no second instance of any operation and no second store.
pub struct Gated<Op, Eng> {
    operation: PhantomData<fn() -> Op>,
    engine: Arc<Eng>,
}

impl<Op, Eng> Gated<Op, Eng> {
    /// Wrap one operation, judged against the policies held in `engine`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            operation: PhantomData,
            engine,
        }
    }
}

impl<Op: Operation, Eng: Send + Sync + 'static> ErasedOperation for Gated<Op, Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // IAM's decision, taken before the operation reads or writes anything: the
            // action is this operation's own name under this service's prefix, and the
            // resource is the one the REQUEST names. A refused request never reaches the
            // delegate, so it mints nothing, moves no status and reports nothing.
            let resource = judged_resource(ctx, req);
            if let Err(denial) = authorize(&*self.engine, ctx, Op::NAME, &resource) {
                // The protocol is read, used and dropped inside this one statement:
                // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the
                // await below would make this future non-`Send`.
                return refusal_response(svc.definition().protocol(), &denial);
            }
            // The modelled input constraints, worded by this service's own seam
            // (`Service::override_refusal`, the one place its refusals are worded) over the
            // violations as a reading of the REQUEST restores them: the validating walk
            // summarises a COMPOSITE member's value as its size (`<n> entries`), which is
            // wire text AWS never emits, and that seam is handed the violations alone — so
            // the value has to be put back where the request is still in hand. A refusal the
            // seam declines is left to the walk behind the delegate exactly as before, and
            // the violations are the framework's own, so nothing is refused here that it
            // would accept.
            let refused = submitted_violations::<Op>(svc.definition().protocol(), req);
            if !refused.is_empty() {
                if let Some(stated) = svc.override_refusal(&Refusal::Invalid(refused.clone())) {
                    return svc.definition().refusal_response(
                        ProtocolError::Invalid(refused),
                        Some(stated),
                        req,
                    );
                }
            }
            let delegate = svc.definition().routing_table().get(Op::NAME);
            match delegate {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key the
                // generated table registered. Answer the framework's own no-such-operation
                // response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapper is not an operation of its own: the request is signed as the MODEL
        // says for the operation it names, so authentication is decided by the modelled
        // operation this wrapper stands in front of.
        None
    }
}

/// Every violation that refuses `req` before `Op`'s handler runs, each carrying the value
/// the CALLER SUBMITTED.
///
/// The violations are the framework's own — the same collecting walk the operation's own
/// sequence runs ([`::service_common::validation`]) — with one thing restored: the
/// validating walk has only the SIZE of a composite value to report (`<n> entries` for a
/// map), where AWS's clause quotes the value itself, so a rendering of the submitted map is
/// put back in its place ([`::service_common::validation::refusing_violations_with_mapped_echoes`],
/// which states the entries in key order). A value that walk cannot render keeps the
/// framework's own text, which for every scalar member IS the submitted value.
fn submitted_violations<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Vec<Violation> {
    let (refused, _lists) =
        ::service_common::validation::refusing_violations_with_mapped_echoes::<Op>(protocol, req);
    refused
        .into_iter()
        .take(::service_common::validation::MAX_VIOLATIONS)
        .map(|(mut violation, echo)| {
            // Only a value the walk RENDERED is put back, and only where the framework
            // recorded one: an absent member has no value to quote, and a value the walk
            // could not render leaves the framework's text alone.
            if let (SubmittedEcho::Rendered(rendered), Some(_)) = (echo, violation.value.as_ref()) {
                violation.value = Some(rendered);
            }
            violation
        })
        .collect()
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the IAM
    /// role behind an assumed-role session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no
    /// IAM policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's
    /// own accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's
            // own catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it.
fn caller<Eng: Send + Sync + 'static>(engine: &Eng, ctx: &OperationContext<'_>) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential.
        None => Ok(sts_store::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody: no documents, no
        // principal spelling, and every action it asks for falls to the default deny. The
        // key id it presented is NOT quoted back — this module echoes no value of the
        // request — and in the assembled emulator the layer that authenticates a request
        // refuses an expired credential before any handler runs.
        Err(expired) => {
            let _ = expired;
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                principals: Vec::new(),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
            };
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        arn: identity.arn,
        documents: Vec::new(),
        boundary: None,
        unreadable_attachment: false,
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the
    // user's policies rather than served as the account. A key the IAM store does not hold
    // either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the session
        // ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            let role_arn = held.arn.clone();
            caller.principals.push(role_arn.clone());
            caller.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-appsync-awsdeepdishcontrolplaneservice.json"
    );

    /// The decision one stored document reaches about an action on a resource, through the
    /// shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE applied to the whole surface: [`GATED_OPERATIONS`] is EXACTLY the set of
    /// operations the service shape binds, so every action this service publishes is judged.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// the model gains fails here instead of silently serving a caller whose policies allow
    /// nothing.
    #[test]
    fn the_gated_set_is_every_operation_the_service_publishes() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: ::serde_json::Value =
            ::serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let mut bound: Vec<&str> = shapes
            .values()
            .filter(|shape| shape["type"] == "service")
            .filter_map(|service| service["operations"].as_array())
            .flatten()
            .filter_map(|bound| bound["target"].as_str())
            .filter_map(|target| target.rsplit('#').next())
            .collect();
        bound.sort_unstable();
        let mut gated: Vec<&str> = GATED_OPERATIONS.to_vec();
        gated.sort_unstable();
        assert_eq!(
            gated, bound,
            "case \"the whole surface\": expected the gated set to be every operation the \
             service binds, got {gated:?} against {bound:?}"
        );
    }

    /// RULE: `AccessDeniedException` is a shape of THIS model, so the refusal this gate
    /// answers with is one the service declares — whether or not the operation a refused
    /// request named lists it among its own errors.
    #[test]
    fn the_refusal_is_a_shape_this_model_declares() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: ::serde_json::Value =
            ::serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let declared = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes")
            .iter()
            .find(|(id, _)| id.ends_with(&format!("#{ACCESS_DENIED_EXCEPTION}")))
            .map(|(_, shape)| shape["traits"]["smithy.api#httpError"].clone());
        assert_eq!(
            declared,
            Some(::serde_json::json!(ACCESS_DENIED_STATUS)),
            "case {ACCESS_DENIED_EXCEPTION:?}: expected the model to declare it at \
             {ACCESS_DENIED_STATUS}, got {declared:?}"
        );
    }

    /// RULE: the action this gate asks about is `appsync:<OperationName>`, so a policy
    /// scoped to this service's actions covers every gated operation and nothing outside
    /// this service.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"appsync:*","Resource":"*"}]}"#;
        for operation in GATED_OPERATIONS {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reached = decision(document, &action, ANY_RESOURCE);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected appsync:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "iam:CreateUser", ANY_RESOURCE);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected appsync:* to grant nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: the resource a call is judged against is the one the REQUEST names — the path
    /// label where the URI carries one, else the name its input carries for the resource it
    /// creates — in this service's own ARN spellings; a call that names no single resource
    /// is judged against every resource of this service in the account and region
    /// addressed.
    #[test]
    fn the_request_decides_the_resource_the_action_is_judged_against() {
        let created = ::serde_json::json!({
            "domainName": "created.example.com",
            "certificateArn": "arn:aws:acm:us-east-1:000000000000:certificate/one",
        });
        let created = created.as_object().expect("the row's input is an object");
        /// One row: the case, the request path, the input it carries, what it names.
        type Row<'a> = (
            &'a str,
            &'a str,
            Option<&'a ::serde_json::Map<String, ::serde_json::Value>>,
            Addressed,
        );
        let rows: &[Row<'_>] = &[
            (
                "a custom domain name",
                "/v1/domainnames/api.example.com",
                None,
                Addressed::Resource("domainnames/api.example.com".to_string()),
            ),
            (
                "a custom domain's API association",
                "/v1/domainnames/api.example.com/apiassociation",
                None,
                Addressed::Resource("domainnames/api.example.com".to_string()),
            ),
            (
                "a create, which names the domain it is about to create",
                "/v1/domainnames",
                Some(created),
                Addressed::Resource("domainnames/created.example.com".to_string()),
            ),
            (
                "a listing of custom domains, which names none",
                "/v1/domainnames",
                None,
                Addressed::Account,
            ),
            (
                "a GraphQL API",
                "/v1/apis/abcdefghijklmnopqrstuvwxyz",
                None,
                Addressed::Resource("apis/abcdefghijklmnopqrstuvwxyz".to_string()),
            ),
            (
                "a GraphQL API's environment variables, which no ARN spelling covers",
                "/v1/apis/abcdefghijklmnopqrstuvwxyz/environmentVariables",
                None,
                Addressed::Path(
                    "/v1/apis/abcdefghijklmnopqrstuvwxyz/environmentVariables".to_string(),
                ),
            ),
            (
                "an Event API",
                "/v2/apis/abcdefghijklmnopqrstuvwxyz",
                None,
                Addressed::Resource("apis/abcdefghijklmnopqrstuvwxyz".to_string()),
            ),
            (
                "a channel namespace of an Event API",
                "/v2/apis/abcdefghijklmnopqrstuvwxyz/channelNamespaces/orders",
                None,
                Addressed::Resource(
                    "apis/abcdefghijklmnopqrstuvwxyz/channelNamespace/orders".to_string(),
                ),
            ),
            (
                "a data source of a GraphQL API",
                "/v1/apis/abcdefghijklmnopqrstuvwxyz/datasources/orders_table",
                None,
                Addressed::Resource(
                    "apis/abcdefghijklmnopqrstuvwxyz/datasources/orders_table".to_string(),
                ),
            ),
            (
                "a function of a GraphQL API",
                "/v1/apis/abcdefghijklmnopqrstuvwxyz/functions/fn-1",
                None,
                Addressed::Resource(
                    "apis/abcdefghijklmnopqrstuvwxyz/functions/fn-1".to_string(),
                ),
            ),
            (
                "a type of a GraphQL API",
                "/v1/apis/abcdefghijklmnopqrstuvwxyz/types/Query",
                None,
                Addressed::Resource("apis/abcdefghijklmnopqrstuvwxyz/types/Query".to_string()),
            ),
            (
                "a resolver on a field of a type",
                "/v1/apis/abcdefghijklmnopqrstuvwxyz/types/Query/resolvers/orders",
                None,
                Addressed::Resource(
                    "apis/abcdefghijklmnopqrstuvwxyz/types/Query/resolvers/orders".to_string(),
                ),
            ),
            (
                "a resolver collection, which names the type it collects under",
                "/v1/apis/abcdefghijklmnopqrstuvwxyz/types/Query/resolvers",
                None,
                Addressed::Resource("apis/abcdefghijklmnopqrstuvwxyz/types/Query".to_string()),
            ),
            (
                "a GraphQL API's API keys, which name the API itself",
                "/v1/apis/abcdefghijklmnopqrstuvwxyz/apikeys/da2-key",
                None,
                Addressed::Resource("apis/abcdefghijklmnopqrstuvwxyz".to_string()),
            ),
            (
                "a Merged API addressed by its id",
                "/v1/mergedApis/abcdefghijklmnopqrstuvwxyz/sourceApiAssociations",
                None,
                Addressed::Resource("apis/abcdefghijklmnopqrstuvwxyz".to_string()),
            ),
            (
                "a source API addressed by its ARN",
                "/v1/sourceApis/arn%3Aaws%3Aappsync%3Aus-east-1%3A000000000000%3Aapis%2Fabc\
                 /mergedApiAssociations",
                None,
                Addressed::Arn("arn:aws:appsync:us-east-1:000000000000:apis/abc".to_string()),
            ),
            (
                "the data source introspection space, which names no resource",
                "/v1/datasources/introspections/abc",
                None,
                Addressed::Account,
            ),
            (
                "the tagging surface, whose label is the target's ARN",
                "/v1/tags/arn%3Aaws%3Aappsync%3Aus-east-1%3A000000000000%3Aapis%2Fabc",
                None,
                Addressed::Arn("arn:aws:appsync:us-east-1:000000000000:apis/abc".to_string()),
            ),
            (
                "the code evaluation sandbox, which names no resource",
                "/v1/dataplane-evaluatecode",
                None,
                Addressed::Account,
            ),
            (
                "the template evaluation sandbox, which names no resource",
                "/v1/dataplane-evaluatetemplate",
                None,
                Addressed::Account,
            ),
        ];
        for (case, path, input, expected) in rows {
            let reached = addressed(path, *input);
            assert_eq!(
                &reached, expected,
                "case {case:?}: expected {path:?} to name {expected:?}, got {reached:?}"
            );
        }
    }

    /// RULE: the refusal names the principal, the action, the resource the request
    /// addressed and which of IAM's rules refused — every part taken from the request and
    /// the stored policies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = "arn:aws:iam::000000000000:user/domain-reader";
        let resource = ::engine::emulator_arn(
            ACTION_PREFIX,
            "eu-west-1",
            &crate::state::domain_name_resource("api.example.com"),
        );
        let action = format!("{ACTION_PREFIX}:GetDomainName");
        for reason in [
            iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string(),
            iam::no_identity_policy_allows(&action),
            iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            iam::no_permissions_boundary_allows(&action),
        ] {
            let refusal = not_authorized(principal, &action, &resource, &reason);
            let message = refusal.message.unwrap_or_default();
            assert_eq!(
                message,
                format!(
                    "User: {principal} is not authorized to perform: {action} on resource: \
                     {resource} {reason}"
                ),
                "case {reason:?}: expected IAM's wording, got {message:?}"
            );
        }
    }
}
