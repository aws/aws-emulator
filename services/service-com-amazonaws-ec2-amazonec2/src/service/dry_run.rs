// SPDX-License-Identifier: CC0-1.0

//! The `DryRun` flag a request carries, and the answer EC2 gives it ahead of performing the
//! request — read from the request itself, for the operations this emulator has not written.
//!
//! `DryRun` "checks whether you have the required permissions for the action, WITHOUT
//! ACTUALLY MAKING THE REQUEST, and provides an error response. If you have the required
//! permissions, the error response is DryRunOperation. Otherwise, it is
//! UnauthorizedOperation" (the member's own doc, carried by every input shape that models
//! it). So the flag is a question about the CALLER, answered before the request is performed,
//! and the answer is one of two refusals: the `UnauthorizedOperation` half is the
//! authorization gate's ([`super::authorization`], asked ahead of every operation whether the
//! flag is set or not), and the permitted half is
//! [`crate::state::DRY_RUN_OPERATION`].
//!
//! ## Which half of the answer lives here
//!
//! An operation this emulator HAS written answers the flag itself, at the point its own
//! recorded contract puts it among the checks it makes — before it resolves what the request
//! names, after it, or before it reads the form of an id at all. That placement is a fact of
//! each operation and belongs in its handler.
//!
//! An operation this emulator has NOT written answers the scaffold's HTTP 501
//! `NotImplemented` ([`crate::scaffolded`]), which is the signal that no implementation read
//! the request and nothing was performed — and the flag's answer does not depend on the work
//! the operation would have done, only on the caller's permissions. So that 501 is replaced
//! here by the verdict EC2 gives the flag, which is what a client using `DryRun` as a
//! permission probe (a preflight, a `--dry-run` call) asks for: without it, every such probe
//! reads as a hard failure of the action rather than as the permission answer.
//!
//! A 501 is NOT always that signal, and the difference is the whole of this gate's judgement:
//! a handful of IMPLEMENTED operations answer the same 501 for a MEMBER of the request this
//! emulator holds no state for, where it is a statement about the request rather than about
//! the operation. Those operations are named in
//! [`OPERATIONS_WHOSE_HANDLER_ANSWERS_NOT_IMPLEMENTED`] and their answers are left exactly as
//! the handler gave them: a request for work this emulator cannot perform must never read
//! "the request would have succeeded".
//!
//! Nothing else about such a call changes: the request was performed either way (not at all),
//! and the Region, authorization and fault-injection gates ahead of dispatch have already had
//! their say, so the verdict is only ever given to a caller those gates admitted.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};

use super::management_events;
use super::request_parameters::{request_parameters, MAX_PARAMETERS_READ};

/// The member name an ec2Query request carries the flag under, as the model spells the
/// member every operation that takes it declares (`DryRun`).
const DRY_RUN_PARAMETER: &str = "DryRun";

/// The one value of a Smithy `Boolean` that sets the flag. `DryRun=false` is not a dry run:
/// it is an ordinary call.
const SET: &str = "true";

/// The HTTP status the scaffold answers an operation nobody has written with
/// (`crate::scaffolded::Scaffolded::NotImplemented`): the status that says no implementation
/// read the request and nothing was performed.
const NOT_IMPLEMENTED_STATUS: u16 = 501;

/// The operations this crate HAS written whose handler can still answer that 501 ITSELF, for
/// a MEMBER of the request this emulator holds no state for — "AssociateRouteTable with a
/// public IPv4 pool", a `CreateVpcEndpoint` of a kind nothing serves, an IPv6 subnet.
///
/// For those the 501 is a statement about the REQUEST rather than about the operation, so the
/// flag is the handler's own to answer and this gate leaves their answers exactly as they are:
/// a request for work this emulator cannot perform must not read "the request would have
/// succeeded". Every other operation's 501 can only be the whole-operation scaffold.
///
/// Sorted, and kept honest by a census over this crate's own sources rather than by hand: an
/// operation whose file carries no `SCAFFOLDED, NOT IMPLEMENTED` marker and still names
/// `not_implemented` belongs here, which is what
/// `tests/all/dry_run_verdict_and_ordering.rs` asserts.
pub const OPERATIONS_WHOSE_HANDLER_ANSWERS_NOT_IMPLEMENTED: &[&str] = &[
    "AssociateRouteTable",
    "AssociateSubnetCidrBlock",
    "AssociateVpcCidrBlock",
    "CreateCapacityReservation",
    "CreateNatGateway",
    "CreateNetworkInterface",
    "CreateRoute",
    "CreateSubnet",
    "CreateVpc",
    "CreateVpcEndpoint",
    "ModifyVpcEndpoint",
    "RequestSpotFleet",
    "RequestSpotInstances",
    "RunInstances",
];

/// Whether `request` sets the `DryRun` flag.
///
/// The parameters come out of the request's own list, read through the one walk every gate of
/// this front door takes over it ([`super::request_parameters`]) and bounded by the same
/// number, so this gate cannot read a request differently from the ones beside it.
#[must_use]
pub fn requested(request: &RtRequest) -> bool {
    request_parameters(request, MAX_PARAMETERS_READ)
        .any(|(name, value)| name == DRY_RUN_PARAMETER && value.eq_ignore_ascii_case(SET))
}

/// The verdict EC2 gives a `DryRun` request for an operation this emulator has not written,
/// or `None` when the request does not set the flag, when `answered` did not come from the
/// scaffold at all, or when `operation` is one whose own handler answers that 501 for a member
/// of the request ([`OPERATIONS_WHOSE_HANDLER_ANSWERS_NOT_IMPLEMENTED`]).
///
/// The code, the status and the sentence are EC2's own
/// ([`crate::state::DRY_RUN_OPERATION`], [`crate::state::DRY_RUN_OPERATION_STATUS`],
/// [`crate::state::DRY_RUN_OPERATION_MESSAGE`]) — the same three a handler answers the flag
/// with, so one verdict is worded one way whichever side of dispatch states it. The envelope
/// is written through the framework's ec2Query error body and closed by the caller's
/// `stamp_envelope`, so it is shaped exactly like every other error of this service.
#[must_use]
pub fn verdict_for_unwritten_operation(
    request: &RtRequest,
    operation: &str,
    answered: &RtResponse,
) -> Option<RtResponse> {
    if answered.status().as_u16() != NOT_IMPLEMENTED_STATUS
        || OPERATIONS_WHOSE_HANDLER_ANSWERS_NOT_IMPLEMENTED.contains(&operation)
        || !requested(request)
    {
        return None;
    }
    let body = ::aws_smithy_schema_server::query_error_body(
        true,
        crate::state::DRY_RUN_OPERATION,
        crate::state::DRY_RUN_OPERATION_MESSAGE,
    );
    let status =
        ::aws_smithy_runtime_api::http::StatusCode::try_from(crate::state::DRY_RUN_OPERATION_STATUS)
            .ok()?;
    // The AUDIT leg ([`management_events`]): the scaffold held "not implemented", which is
    // not what this call was answered with — a call answered the dry-run verdict WAS served,
    // exactly as one a handler answers the same verdict is, so the outcome recorded for it is
    // the code, the sentence and the status the caller really got.
    management_events::hold_refusal(
        crate::state::DRY_RUN_OPERATION,
        crate::state::DRY_RUN_OPERATION_MESSAGE,
        crate::state::DRY_RUN_OPERATION_STATUS,
        false,
    );
    Some(RtResponse::new(
        status,
        ::aws_smithy_types::body::SdkBody::from(body),
    ))
}
