// SPDX-License-Identifier: CC0-1.0

//! This service's LOOKUP layer: the name members a request resolves against
//! stored state rather than format-checks.
//!
//! The model constrains a namespace name and a table name with the character set
//! and the length S3 Tables will CREATE one under (`NamespaceName`, `TableName`:
//! `@length(1, 255)`, `@pattern(^[0-9a-z_]*$)`). Those constraints belong to the
//! members that NAME a resource being created or renamed — `CreateNamespace`'s
//! namespace, `CreateTable`'s table name, `RenameTable`'s new coordinates — and a
//! value outside them is refused there, with the fixed sentence this service
//! states that class with.
//!
//! The SAME shapes are also what every other operation ADDRESSES an existing
//! resource by, and there the string is a lookup key: S3 Tables resolves it
//! against what the account holds and answers `NotFoundException` for a name
//! nothing is stored under, whatever characters it is spelled with. A client
//! that treats 404 as "absent" and 400 as "my request is malformed" depends on
//! the difference: an existence probe, a "delete if present" or a "create if
//! missing" over a name some other system handed it must come back as absent
//! rather than as a malformed request.
//!
//! The framework validates EVERY modelled constraint of an input before a
//! handler runs, which is the right default and is where the create-time
//! refusals come from. This layer is the exception for the lookup keys, made at
//! the one seam a service has in front of dispatch
//! ([`smithy_server_runtime::Service::override_operation`], the same seam
//! [`super::request_layer`] uses):
//!
//! * the request is deserialized and validated with the framework's OWN
//!   deserializer and validator, so this layer cannot disagree with the walk
//!   about what the model says;
//! * when every constraint the request breaks is the name FORMAT of a member the
//!   operation looks up, the operation's handler runs on the input as it stands
//!   and resolves the name against state — which answers the modelled
//!   `NotFoundException` for a name nothing is stored under. It runs behind the
//!   SAME authorization gate the generated walk serves it behind
//!   ([`super::authorization`]), so taking this exception does not serve a call
//!   the policies in force refuse;
//! * anything else — a create's own name, an ARN, a value outside an enumerated
//!   set, an absent required member — is left to the framework, which refuses it
//!   exactly as before.
//!
//! WHICH members an operation resolves rather than checks is the one list below.
//!
//! The cost of making the decision at this seam is that a request addressed to
//! one of those operations is read TWICE — once here, to decide, and once by the
//! walk (or by this layer) that serves it — because the seam hands the router a
//! borrowed operation rather than a value it can carry the built input in. It is
//! the same cost [`super::request_layer`] pays to answer the right error class,
//! and an emulator pays it on a path that is a refusal in every other case.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::{self, Violation, ViolationKind};
use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, OperationError,
    ServerHttpProtocol, Service,
};

/// Whether `violation` is the name FORMAT of a member this operation resolves
/// against state: the character set or the length of one of `resolved`.
///
/// Read off the MEMBER and the CONSTRAINT the value failed, never off the value,
/// so the arm covers every spelling a lookup key can arrive in. An absent
/// `@required` member is NOT this class — a request that carries no name
/// addresses nothing and stays the framework's refusal.
fn resolves_rather_than_checks(violation: &Violation, resolved: &[&str]) -> bool {
    let member = violation.path.split('.').next().unwrap_or(&violation.path);
    resolved.contains(&member)
        && matches!(
            violation.kind,
            ViolationKind::Pattern(_) | ViolationKind::MinLength(_) | ViolationKind::MaxLength(_)
        )
}

/// The input of `O` in `req`, when every constraint it breaks is the name format
/// of a member in `resolved` — the one case this layer serves.
///
/// `None` covers every other outcome: a request that breaks no constraint at all
/// (the framework's walk serves it), one that breaks a constraint of some other
/// member, and a body that cannot be read into the input at all (which
/// [`super::request_layer`] is what answers).
fn resolvable_input<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    resolved: &[&str],
) -> Option<O::Input> {
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let input = <O::Input as DeserializableStruct>::deserialize(&mut *deserializer).ok()?;
    drop(deserializer);
    let violations = constraints::validate(&input, O::INPUT_SCHEMA);
    let resolvable = !violations.is_empty()
        && violations
            .iter()
            .all(|violation| resolves_rather_than_checks(violation, resolved));
    resolvable.then_some(input)
}

/// Run `op` on an input the framework's walk would have refused for a lookup
/// key's format, and answer what it returns.
///
/// Everything but the constraint step of
/// [`smithy_server_runtime::run_operation_http`], driven through the same
/// protocol: the handler decides, and its answer — a record, or the modelled
/// `NotFoundException` for a name nothing is stored under — is serialized by the
/// service's own codec.
async fn serve_resolved<O: Operation>(
    op: &O,
    svc: &dyn Service,
    req: &RtRequest,
    ctx: &OperationContext<'_>,
    resolved: &[&str],
) -> RtResponse {
    let protocol = svc
        .override_protocol()
        .unwrap_or_else(|| svc.definition().protocol());
    let Some(input) = resolvable_input::<O>(protocol, req, resolved) else {
        // The request is not this layer's case after all, which only a request
        // that changed between the two reads could be: it is answered as the
        // framework answers a body it cannot read into the input.
        return svc.definition().framework_error_response(
            ::aws_smithy_schema_server::ProtocolError::UnknownOperation(O::NAME.to_string()),
            req,
        );
    };
    match op.handle(input, ctx).await {
        Ok(output) => protocol
            .serialize_output(&output, O::OUTPUT_SCHEMA, O::NAME, O::SUCCESS_STATUS)
            .unwrap_or_else(|error| svc.definition().framework_error_response(error, req)),
        Err(failure) => protocol
            .serialize_error(
                failure.as_serializable(),
                failure.variant_schema(),
                failure.shape_name(),
                failure.http_status(),
            )
            .unwrap_or_else(|error| svc.definition().framework_error_response(error, req)),
    }
}

/// The operations that RESOLVE a name member against state, and which members
/// each resolves — this service's whole statement of the rule, in one list.
///
/// Every operation that addresses a resource by a namespace name or a table name
/// is here; the members that NAME a resource being created or renamed
/// (`CreateNamespace`'s and `CreateTableBucket`'s own `name`/`namespace`,
/// `RenameTable`'s `newName` and `newNamespaceName`, `CreateTable`'s `name`) are
/// deliberately absent, so a create still refuses a name it would not create.
///
/// The key each arm matches is the routing key the protocol resolved, which for
/// this restJson1 service is the operation's shape name — also the name of the
/// generated operation type, so there is no second spelling of the surface to
/// drift from the first.
macro_rules! lookup_operations {
    ( $( $field:ident : $module:ident :: $operation:ident => [ $( $member:literal ),+ ] ),+ $(,)? ) => {
        /// Whether `req`, which the protocol resolved to the operation `key`,
        /// breaks nothing but the name format of a member that operation
        /// resolves against state.
        pub(crate) fn resolves_names<Eng: Send + Sync + 'static>(
            key: &str,
            protocol: &dyn ServerHttpProtocol,
            req: &RtRequest,
        ) -> bool {
            match key {
                $(
                    stringify!($operation) => resolvable_input::<
                        crate::ops::$module::$operation<Eng>,
                    >(protocol, req, &[$($member),+])
                    .is_some(),
                )+
                _ => false,
            }
        }

        /// The operations this layer serves, one per lookup operation, built
        /// from the shared engine exactly as the generated entry builds them —
        /// each behind the authorization gate
        /// ([`super::authorization::Authorized`]), so serving a request the
        /// framework would have refused for a lookup key's FORMAT still takes
        /// the decision every operation of this service is served behind.
        pub(crate) struct NameLookups<Eng> {
            $( $field: super::authorization::Authorized<
                crate::ops::$module::$operation<Eng>,
                Eng,
            >, )+
        }

        impl<Eng> NameLookups<Eng> {
            pub(crate) fn new(engine: Arc<Eng>) -> Self {
                Self {
                    $( $field: super::authorization::Authorized::new(
                        crate::ops::$module::$operation::new(Arc::clone(&engine)),
                        Arc::clone(&engine),
                    ), )+
                }
            }
        }

        impl<Eng: Send + Sync + 'static> NameLookups<Eng> {
            /// Serve `req` through the operation `key` names, resolving that
            /// operation's own lookup members instead of format-checking them.
            async fn serve(
                &self,
                svc: &dyn Service,
                req: &RtRequest,
                ctx: &OperationContext<'_>,
                key: &str,
            ) -> Option<RtResponse> {
                match key {
                    $(
                        stringify!($operation) => Some(
                            serve_resolved(&self.$field, svc, req, ctx, &[$($member),+]).await,
                        ),
                    )+
                    _ => None,
                }
            }
        }
    };
}

lookup_operations! {
    get_namespace: get_namespace::GetNamespace => ["namespace"],
    delete_namespace: delete_namespace::DeleteNamespace => ["namespace"],
    list_tables: list_tables::ListTables => ["namespace"],
    list_table_migrations: list_table_migrations::ListTableMigrations => ["namespace", "name"],
    create_table: create_table::CreateTable => ["namespace"],
    get_table: get_table::GetTable => ["namespace", "name"],
    delete_table: delete_table::DeleteTable => ["namespace", "name"],
    rename_table: rename_table::RenameTable => ["namespace", "name"],
    get_table_metadata_location:
        get_table_metadata_location::GetTableMetadataLocation => ["namespace", "name"],
    update_table_metadata_location:
        update_table_metadata_location::UpdateTableMetadataLocation => ["namespace", "name"],
    get_table_encryption: get_table_encryption::GetTableEncryption => ["namespace", "name"],
    get_table_storage_class: get_table_storage_class::GetTableStorageClass => ["namespace", "name"],
    get_table_maintenance_configuration:
        get_table_maintenance_configuration::GetTableMaintenanceConfiguration
            => ["namespace", "name"],
    put_table_maintenance_configuration:
        put_table_maintenance_configuration::PutTableMaintenanceConfiguration
            => ["namespace", "name"],
    get_table_maintenance_job_status:
        get_table_maintenance_job_status::GetTableMaintenanceJobStatus => ["namespace", "name"],
    get_table_policy: get_table_policy::GetTablePolicy => ["namespace", "name"],
    put_table_policy: put_table_policy::PutTablePolicy => ["namespace", "name"],
    delete_table_policy: delete_table_policy::DeleteTablePolicy => ["namespace", "name"],
}

impl<Eng: Send + Sync + 'static> ErasedOperation for NameLookups<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The request resolved to this layer, so the routing table names one
            // of the operations above; a request that names any other is never
            // routed here.
            let key = svc
                .definition()
                .protocol()
                .routing_key(req)
                .unwrap_or_default()
                .to_string();
            match self.serve(svc, req, ctx, &key).await {
                Some(response) => response,
                None => svc.definition().framework_error_response(
                    ::aws_smithy_schema_server::ProtocolError::UnknownOperation(key),
                    req,
                ),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names", so
        // this layer cannot weaken the schemes the model declares for the
        // operation the request addressed.
        None
    }
}
