// SPDX-License-Identifier: CC0-1.0

//! What this service reports when a call it makes FOR THE CALLER in another
//! service is refused there.
//!
//! An Elastic Beanstalk environment is made of other services' resources — "the
//! Auto Scaling group, load balancer, etc." — and the operations that create or
//! recreate them do so on the caller's behalf, so those calls carry the CALLER's
//! permissions. A caller the owning service would refuse cannot have the work
//! done, and what this service answers is the refusal that service gave: Amazon
//! EC2 answers an operation the caller's policies do not allow with its fixed
//! sentence under the `UnauthorizedOperation` code, which the relayed answer's
//! own envelope then names the called service, the status and that call's id in.
//!
//! Neither the envelope nor the id is built here: both are the workspace's one
//! rendering of a relayed refusal ([`::service_common::relayed_refusal`]). What
//! this module states is only what is this service's own — WHICH action of the
//! owning service each operation cannot skip, the resource it is decided on, and
//! the sentence that owning service words — while the decision itself is the one
//! gate every other authorization question goes through
//! ([`crate::service::authorization::classified_refusal`]).

/// The Amazon EC2 action that LAUNCHES instances, which is the one operation
/// nothing an environment runs on can be recreated without ("Launches the
/// specified number of instances", `ec2:RunInstances`).
pub const ACTION_RUN_INSTANCES: &str = "ec2:RunInstances";

/// The resource an environment's Amazon EC2 instances are created under in
/// `region`: every instance of this account there, which is what a launch of new
/// instances is decided on — the instances do not exist yet, so none of them can
/// be named.
pub fn environment_instances_resource(region: &str) -> String {
    ::engine::emulator_arn("ec2", region, "instance/*")
}

/// Amazon EC2's own sentence for an operation the caller's policies do not allow,
/// which is what it answers under [`CODE_UNAUTHORIZED_OPERATION`]: WHOLLY FIXED
/// text, carrying nothing of the request.
const UNAUTHORIZED_OPERATION_SENTENCE: &str = "You are not authorized to perform this operation.";

/// The service Amazon EC2 is named as in the envelope of a relayed answer.
const SERVICE_EC2: &str = "AmazonEC2";

/// The error code Amazon EC2 refuses an operation the caller may not perform
/// with, and the status that refusal carries.
const CODE_UNAUTHORIZED_OPERATION: &str = "UnauthorizedOperation";
/// See [`CODE_UNAUTHORIZED_OPERATION`].
const STATUS_FORBIDDEN: u16 = 403;

/// How a refusal from the service that OWNS the resource is reported: that
/// service's own answer, as it arrived. Amazon EC2's fixed sentence, then IAM's
/// own statement of what was refused — who, which action, on which resource and
/// which of IAM's rules refused it — inside the envelope the relayed answer
/// carries.
///
/// `iam_refusal` is that statement
/// ([`crate::service::authorization::classified_refusal`]); every value in it
/// comes from the caller's identity and the request this service made for it. The
/// envelope's own request id belongs to the relayed CALL, so one is minted for it
/// through the same shared rendering rather than derived from anything in the
/// request.
pub fn unauthorized_operation_message(iam_refusal: &str) -> String {
    let request_id = ::service_common::query_error_envelope::new_request_id();
    ::service_common::relayed_refusal::caught_exception_message(
        "",
        "",
        &format!("{UNAUTHORIZED_OPERATION_SENTENCE} {iam_refusal}"),
        ::service_common::relayed_refusal::CaughtCall {
            service: SERVICE_EC2,
            status: STATUS_FORBIDDEN,
            code: CODE_UNAUTHORIZED_OPERATION,
            request_id: &request_id,
        },
    )
}
