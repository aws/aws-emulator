// SPDX-License-Identifier: CC0-1.0

//! Whether the policies in force let the request perform the action it asks for
//! — the decision every operation of this service is served behind.
//!
//! Every operation of this service documents the permission it takes ("You must
//! have the s3tables:GetTableBucketEncryption permission to use this
//! operation"), so a signed call is decided against the CALLER's own
//! identity-based policies before the service reads or writes anything. And a
//! table bucket policy is not a document the control plane merely stores and
//! reads back: "a table bucket policy is a resource-based AWS Identity and Access
//! Management (IAM) policy" and it is part of the authorization decision for
//! every call that acts on that bucket or on anything inside it.
//!
//! Both halves are one decision, and it belongs here, in front of the handler,
//! for every operation: IAM reads the two sides together — "an explicit deny in
//! any policy overrides any allow", and within one account "access is granted if
//! either" side allows — and a call it refuses has NO effect, which is why the
//! decision is taken before the handler reads or writes anything. Which
//! operations are decided is not a choice this gate makes: a permission is
//! documented for each of them, so each of them is decided.
//!
//! Three pieces:
//!
//! 1. **which resource the request acts on** ([`addressed`]) — read off the
//!    BUILT input through the tree's one rendering of a shape as a document, so
//!    an operation is decided against the resource its own input names rather
//!    than through a second hand-kept table of URI templates. The ARN is the one
//!    this service MINTS for that resource: a table's own ARN when the store
//!    holds the table the request names, and otherwise the table bucket it is
//!    addressed within, which is the only resource such a request names — a
//!    table's permission being granted on the table, which "Actions, resources,
//!    and condition keys for Amazon S3 Tables" publishes as a resource of its
//!    own under its bucket's ARN;
//! 2. **the resource-based document in force** — the policy that bucket carries,
//!    read through this service's own store, so the document a decision is taken
//!    on is the one `PutTableBucketPolicy` wrote and `DeleteTableBucketPolicy`
//!    removed; `None` for a resource whose bucket carries none, which withholds
//!    nothing;
//! 3. **the decision and the sentence**
//!    ([`crate::state::authorization::refusal`]), which is the shared evaluation
//!    of the caller's own documents and that one against `(action, resource,
//!    principal)` and IAM's own wording for a refusal — so nothing of either is
//!    spelled here.
//!
//! A request whose input names no resource of this service at all — the create of
//! a table bucket, a listing of them — is served: there is nothing for this
//! decision, or for the sentence a refusal carries, to name as the resource.
//!
//! The wrapper is also where the one other decision taken for every operation of
//! this service is made: whether the ARNs the request addresses a resource by
//! name the Region this endpoint serves ([`super::addressed_region`]), which is
//! asked first, since an ARN of another Region names no resource for a policy
//! decision to be about.

use ::std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{BoxFuture, ErasedOperation, Operation, OperationContext, Service};

use super::addressing::Addressing;

/// The resource a request acts on and the resource-based policy in force on the
/// table bucket that holds it.
struct Governed {
    /// The ARN of what the operation acts on, as this service mints it.
    resource: String,
    /// The table bucket's policy document, as the store reads at this instant,
    /// and `None` when that bucket carries none — or when the store holds no
    /// bucket at the ARN the request addressed, which carries no document
    /// either.
    policy: Option<String>,
}

/// The resource the request addresses and the resource-based policy in force on
/// it, or `None` when it names no resource of this service at all (the create of
/// a table bucket, a listing of them), which leaves this decision nothing to be
/// taken on.
///
/// The members are this model's own ([`super::addressing::ARN_MEMBERS`]), read in
/// the order a request narrows what it addresses, with `namespace` and `name`
/// when the operation addresses a table inside the bucket by its coordinates.
///
/// The ARN the request NAMED is the resource whatever the store holds at it, so a
/// request for a resource this account does not hold is decided — and refused —
/// before anything discloses whether it exists. The store is read ONCE, under one
/// guard, and nothing in it is written.
fn addressed<Eng: Send + Sync + 'static>(
    engine: &Eng,
    addressed: &Addressing,
) -> Option<Governed> {
    let text = |member: &str| addressed.text(member).map(str::to_string);
    let state = crate::state::service_state(engine)?;
    let store = state.lock();
    // The policy of the bucket a table ARN names: a table's permissions are
    // decided against the table, under the policy of the bucket holding it.
    let policy_of_bucket_holding = |arn: &str| -> Option<String> {
        let bucket = &store.tables_by_arn.get(arn)?.table_bucket_arn;
        store.table_buckets.get(bucket)?.policy.clone()
    };
    if let Some(arn) = text("resourceArn") {
        let policy = match crate::state::taggable_resource_kind(&arn) {
            Some(crate::state::TaggableResourceKind::TableBucket) => store
                .table_buckets
                .get(&arn)
                .and_then(|bucket| bucket.policy.clone()),
            Some(crate::state::TaggableResourceKind::Table) => policy_of_bucket_holding(&arn),
            // An ARN of neither kind names no resource this service holds a
            // document for; it is still what the request addressed.
            None => None,
        };
        return Some(Governed {
            resource: arn,
            policy,
        });
    }
    if let Some(arn) = text("tableArn") {
        let policy = policy_of_bucket_holding(&arn);
        return Some(Governed {
            resource: arn,
            policy,
        });
    }
    // The same thing under the other spelling this model uses for a table's own
    // ARN, which the migration operations address a destination table by.
    if let Some(arn) = text("tableARN") {
        let policy = policy_of_bucket_holding(&arn);
        return Some(Governed {
            resource: arn,
            policy,
        });
    }
    let bucket_arn = text("tableBucketARN")?;
    let bucket = store.table_buckets.get(&bucket_arn);
    let policy = bucket.and_then(|bucket| bucket.policy.clone());
    // A table operation addresses the table by name inside the bucket; the
    // decision is taken on the table's OWN ARN, and falls back to the bucket for
    // a request naming a table this account does not hold — which is the only
    // resource such a request names.
    let resource = match (text("namespace"), text("name")) {
        (Some(namespace), Some(name)) => bucket
            .and_then(|bucket| bucket.namespaces.get(&namespace))
            .and_then(|namespace| namespace.tables.get(&name))
            .map_or(bucket_arn, |table| table.table_arn.clone()),
        _ => bucket_arn,
    };
    Some(Governed { resource, policy })
}

/// The sentence a request to `operation` is refused with when the policies in
/// force on the caller and on what it addresses do not let it perform that
/// action, and `None` when nothing refuses it.
///
/// `addressed` is what the request's own input addresses, which is where the
/// resource the decision is taken on comes from. The store guard is released
/// before the decision is asked.
#[must_use]
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
    operation: &str,
    addressed_by: &Addressing,
) -> Option<String> {
    let governed = addressed(engine, addressed_by)?;
    crate::state::authorization::refusal(
        engine,
        ctx,
        operation,
        &governed.resource,
        governed.policy.as_deref(),
    )
}

/// One modelled operation served behind this gate: the policies in force decide
/// the call, and only a call they allow reaches the operation.
///
/// It wraps the operation at the TYPED seam ([`Operation`]), so the gate is handed
/// the deserialized input and reads the resource the request addresses off the
/// built shape rather than parsing the request a second time; and the refusal
/// travels through the operation's own error channel, so it is rendered in this
/// service's envelope like every other failure.
pub struct Authorized<O, Eng> {
    /// The operation that serves a call the policies allow.
    operation: O,
    /// The shared engine: this service's store, and the IAM and STS stores the
    /// caller is resolved from, live there.
    engine: Arc<Eng>,
}

impl<O, Eng> Authorized<O, Eng> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: O, engine: Arc<Eng>) -> Self {
        Self { operation, engine }
    }
}

impl<O, E, Eng> Operation for Authorized<O, Eng>
where
    O: Operation<Error = crate::scaffolded::Scaffolded<E>>,
    E: smithy_server_runtime::OperationError + Send + 'static,
    Eng: Send + Sync + 'static,
{
    type Input = O::Input;
    type Output = O::Output;
    type Error = O::Error;

    const NAME: &'static str = O::NAME;
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = O::INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = O::OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = O::SUCCESS_STATUS;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // What the request addresses, read once: both decisions below are about
        // the resource its own input names.
        let addressed_by = Addressing::of(&input, O::INPUT_SCHEMA);
        // The Region first: an ARN of another Region addresses no resource of
        // this endpoint, so there is nothing for a policy decision to be taken
        // on and nothing for a handler to resolve.
        if let Some(message) = super::addressed_region::refusal(ctx, &addressed_by) {
            return Err(crate::scaffolded::Scaffolded::bad_request(message));
        }
        if let Some(message) = refusal(&*self.engine, ctx, O::NAME, &addressed_by) {
            return Err(crate::scaffolded::Scaffolded::access_denied(message));
        }
        drop(addressed_by);
        self.operation.handle(input, ctx).await
    }
}

impl<O, E, Eng> ErasedOperation for Authorized<O, Eng>
where
    O: Operation<Error = crate::scaffolded::Scaffolded<E>> + ErasedOperation,
    E: smithy_server_runtime::OperationError + Send + 'static,
    Eng: Send + Sync + 'static,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        // The framework's own per-request sequence, driven with THIS wrapper as
        // the operation, so the handler it calls is the gated one — and in the
        // envelope the generated shim passes, so a framework-level failure is
        // rendered exactly as it is for an ungated operation.
        let envelope = svc.definition().wire_protocol(req).envelope();
        Box::pin(async move {
            smithy_server_runtime::run_operation_http(self, svc, req, ctx, envelope).await
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapped operation's modelled schemes: gating a call must not change
        // which credentials sign it.
        self.operation.auth_schemes()
    }
}
