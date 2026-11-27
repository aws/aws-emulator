// SPDX-License-Identifier: CC0-1.0

//! What a publication the MESSAGE BROKER took is evaluated by: this service's rules engine,
//! reached from the broker's own wire.
//!
//! A device does not call an API to publish — it opens an MQTT connection to the address
//! `DescribeEndpoint` published and sends a PUBLISH on it — so the publication that has to be
//! evaluated against this account's topic rules arrives on a socket the broker serves
//! ([`::service_common::iot_mqtt_broker::wire`]) and not on a request of this service's own.
//! The broker is below this service and holds no rules, so it asks: the sink installed here
//! ([`::service_common::iot_mqtt_broker::PublicationSink`]) is what it hands every publication
//! it serves to, and what this module then runs is the SAME walk the HTTP data plane's
//! `Publish` runs ([`super::dispatch_broker_publication`]).
//!
//! What the sink remembers is this EMULATOR and not a client: the shared engine the rules live
//! in, the in-process transport every action's call rides
//! ([`::smithy_server_runtime::OperationContext::callback`]), this service's own identity, and
//! the Region of the endpoint whose address was published — the Region an action's target is
//! then reached in, which a publication arriving on a socket carries no credential scope to
//! read. No caller, no credential of a caller's and no request body is held.

use ::std::future::Future;
use ::std::pin::Pin;
use ::std::sync::Arc;

use ::service_common::iot_mqtt_broker::{Publication, PublicationSink};
use ::smithy_server_runtime::{OperationContext, ServiceIdentity};

/// The rules engine as the broker reaches it: everything one evaluation needs, and nothing of
/// any one request.
pub struct BrokerPublications<Eng> {
    /// The shared engine the account's topic rules and the stores their actions read live in.
    engine: Arc<Eng>,
    /// The in-process transport an action's call rides, exactly as a handler's does.
    callback: ::smithy_server_runtime::EmbeddedEmulator,
    /// This service's identity, which the context an evaluation runs under is for.
    identity: ServiceIdentity,
    /// The Region the endpoint the publication arrived at belongs to.
    region: String,
}

/// Written by hand rather than derived: the engine a sink holds carries no `Debug`. What is
/// diagnosable about a sink is the Region it evaluates for.
impl<Eng> ::std::fmt::Debug for BrokerPublications<Eng> {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("BrokerPublications")
            .field("region", &self.region)
            .finish()
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> PublicationSink
    for BrokerPublications<Eng>
{
    fn evaluate<'sink>(
        &'sink self,
        publication: Publication,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'sink>> {
        Box::pin(async move {
            // A publication a device made on one of the broker's reserved FLEET PROVISIONING
            // topics is SERVED as well as evaluated: the certificate it asks for is created,
            // or the template it names is carried out, and the device is answered on that
            // topic's own `accepted` or `rejected` topic
            // ([`crate::state::mqtt_provisioning`]). Every other topic reaches the rules
            // below and nothing else, and a provisioning request reaches them too — a rule
            // whose statement covers a reserved topic is a rule about that topic either way,
            // and nothing of the exchange is reported to the publisher.
            let _answered_a_provisioning_request = crate::state::served_provisioning_request(
                &*self.engine,
                &self.region,
                &publication.topic,
                &publication.payload,
            )
            .await;
            // ... and so is one a device made on a reserved JOBS topic: the executions it
            // asks for, the job document it is handed and the status it reports are served
            // from the shared execution store and answered on that topic's own `accepted` or
            // `rejected` topic ([`crate::state::mqtt_jobs`]). Every other topic reaches the
            // rules below and nothing else, and a jobs request reaches them too — a rule
            // whose statement covers a reserved topic is a rule about that topic either way.
            let _answered_a_jobs_request = crate::state::served_jobs_request(
                &*self.engine,
                &publication.topic,
                &publication.payload,
            );
            // The parts a context is built over name nothing: a publication on the broker's
            // wire serves no request, so there is no caller, no credential and no body — the
            // Region the actions run in is the one this sink was installed for, and the calls
            // they make are signed by the callback with the emulator's own credentials
            // exactly as a handler's are.
            let parts = ::http::Request::new(()).into_parts().0;
            let ctx = OperationContext::new(&parts, &self.identity, self.callback.clone());
            super::dispatch_broker_publication(&*self.engine, &ctx, &self.region, publication)
                .await;
        })
    }
}

/// Install this service's rules engine as what the broker evaluates its publications by, for
/// the endpoint of `region`.
///
/// Called by the operation that PUBLISHES the broker's address, because that is the call a
/// device learns where to connect from: from the moment an address exists, a publication made
/// on it is evaluated. Idempotent — the first sink is kept
/// ([`::service_common::iot_mqtt_broker::MqttBrokerState::install_publication_sink`]), and
/// every sink evaluates the same rules out of the same store — and it changes no session. An
/// engine that vends no broker store installs nothing.
pub fn install_rules_engine<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Arc<Eng>,
    ctx: &OperationContext<'_>,
    region: &str,
) {
    let Some(broker) = ::service_common::iot_mqtt_broker::broker_state(&**engine) else {
        return;
    };
    let sink = Arc::new(BrokerPublications {
        engine: Arc::clone(engine),
        callback: ctx.callback().clone(),
        identity: ctx.identity().clone(),
        region: region.to_string(),
    });
    broker.lock().install_publication_sink(sink);
}
