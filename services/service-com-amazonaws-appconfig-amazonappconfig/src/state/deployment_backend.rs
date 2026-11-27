// SPDX-License-Identifier: CC0-1.0

//! The component AppConfig settles a deployment through, and how a refusal that component
//! answers reaches the caller of `StopDeployment`.
//!
//! Stopping a deployment is not served out of the control-plane store alone: AppConfig acts
//! on the deployment through the component that owns a deployment in flight, and the
//! recorded refusal for a deployment that component holds no record of is that component's
//! own answer, RELAYED rather than re-worded. So the caller reads the component's sentence
//! followed by the clause the SDK layer that relayed the call states around a failed call:
//! which component answered, with what HTTP status, under which error code, and for which
//! call.
//!
//! The clause is the workspace's ONE rendering of that envelope
//! ([`::service_common::relayed_refusal::caught_exception_message`]) rather than a form
//! written out here, so a client parsing the service, the status or the code back out of a
//! message reads the same form whichever emulated service relayed the call. Only the facts
//! are this service's: the component's name, and — from the refusal itself — the status and
//! the code it is answered under.

use ::service_common::query_error_envelope::new_request_id;
use ::service_common::relayed_refusal::{caught_exception_message, CaughtCall};

/// The component a stop settles a deployment through, as the SDK layer that relayed the
/// call names it.
///
/// The token the recorded refusal carries, which is neither this service's `sdkId`
/// (`AppConfig`) nor its endpoint prefix: the call is relayed to AppConfig's deployment
/// component, and the envelope names the component that actually answered rather than the
/// service the client addressed.
const DEPLOYMENT_COMPONENT: &str = "AWSAppConfigData";

/// `reason`, the refusal the deployment component answered, as AppConfig reports it: that
/// sentence verbatim, then the clause naming the call it came from.
///
/// `code` and `status` are the refusal's OWN wire error code and HTTP status, read off the
/// modelled error the handler is about to answer with rather than written out beside it, so
/// the clause and the response can never disagree about either. The request id belongs to
/// the relayed CALL, so one is minted per call and is never derived from the request.
pub fn relayed_refusal(reason: &str, code: &str, status: u16) -> String {
    caught_exception_message(
        "",
        "",
        reason,
        CaughtCall {
            service: DEPLOYMENT_COMPONENT,
            status,
            code,
            request_id: &new_request_id(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{relayed_refusal, DEPLOYMENT_COMPONENT};
    use ::service_common::query_error_envelope::is_request_id;

    /// RULE: the relayed sentence is carried through untouched and the clause states the
    /// component, the refusal's own status and code, a per-call request id and no proxy.
    #[test]
    fn a_relayed_refusal_states_the_reason_then_the_call_it_came_from() {
        // (case, the component's sentence, the refusal's code, its status)
        let rows = [
            (
                "a deployment number nothing issued",
                "Deployment with Application Id abcd123, Environment Id efgh456, and \
                 deploymentNumber 999 could not be found for account 000000000000.",
                "ResourceNotFoundException",
                404u16,
            ),
            (
                "a refusal of another class",
                "Deployment 1 of application abcd123 cannot be stopped.",
                "BadRequestException",
                400,
            ),
        ];
        for (case, reason, code, status) in rows {
            let message = relayed_refusal(reason, code, status);
            let head = format!(
                "{reason} (Service: {DEPLOYMENT_COMPONENT}; Status Code: {status}; Error Code: \
                 {code}; Request ID: "
            );
            assert!(
                message.starts_with(&head),
                "case {case:?}: expected the message to start {head:?}, got {message:?}"
            );
            assert!(
                message.ends_with("; Proxy: null)"),
                "case {case:?}: expected the message to end \"; Proxy: null)\", got {message:?}"
            );
            let request_id = message
                .strip_prefix(&head)
                .and_then(|rest| rest.strip_suffix("; Proxy: null)"))
                .unwrap_or_default();
            assert!(
                is_request_id(request_id),
                "case {case:?}: expected a request id, got {request_id:?}"
            );
        }
    }

    /// RULE: the request id belongs to the relayed CALL, so two refusals never report one.
    #[test]
    fn each_relayed_call_reports_its_own_request_id() {
        let first = relayed_refusal("no such deployment.", "ResourceNotFoundException", 404);
        let second = relayed_refusal("no such deployment.", "ResourceNotFoundException", 404);
        assert_ne!(
            first, second,
            "case \"two relayed calls\": expected each call its own request id, got {first:?} \
             twice"
        );
    }
}
