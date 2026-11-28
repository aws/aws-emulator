// SPDX-License-Identifier: CC0-1.0

//! The flow configuration a link runs, and the one it has taken and not yet put
//! in service.
//!
//! A link of any kind this store holds carries flows under the same rule:
//! `PutLinkFlows` replaces the whole configuration, the change applies
//! asynchronously, and the link serves its PRIOR flows until the change commits —
//! which happens as the link reaches service, since a flow runs exactly when the
//! link carrying it does. So the pair of fields and the commit live here once
//! rather than once per kind of link, and a link between two gateways cannot drift
//! apart from a link with one end outside the fabric about what its flows are.

use crate::ops::types::Flow;

/// The flows in service on one link, and the configuration awaiting it.
#[derive(Debug, Clone, Default)]
pub struct Flows {
    /// The flows the link serves, empty until a change puts one in service.
    in_service: Vec<Flow>,
    /// The flow configuration the link has taken and not yet put in service;
    /// `None` when none is awaiting it. `Some(Vec::new())` is a configuration of
    /// no flows, a change of its own: it is what removes every flow.
    pending: Option<Vec<Flow>>,
}

impl Flows {
    /// The flows in service, as a reader reports them: nothing at all when none
    /// is, since a configuration the link has taken and not yet put in service is
    /// not what the link serves.
    pub fn reported(&self) -> Option<Vec<Flow>> {
        (!self.in_service.is_empty()).then(|| self.in_service.clone())
    }

    /// Take `flows` as the configuration awaiting the link, which is what an
    /// accepted change leaves behind it.
    pub fn accept(&mut self, flows: Vec<Flow>) {
        self.pending = Some(flows);
    }

    /// Put the configuration awaiting the link in service, if one is: the change
    /// commits as the link reaches service.
    pub fn commit(&mut self) {
        if let Some(taken) = self.pending.take() {
            self.in_service = taken;
        }
    }

    /// Whether a change the link has taken is still awaiting it.
    pub fn change_pending(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::Flows;
    use crate::ops::types::{Flow, FlowPipeline, FlowModule, SequentialPipeline};

    /// A flow named `name`, with the one pipeline shape a flow needs to exist.
    fn flow(name: &str) -> Flow {
        Flow {
            name: name.to_string(),
            pipeline: FlowPipeline::Sequential(SequentialPipeline {
                modules: vec![FlowModule {
                    name: "Observe".to_string(),
                    module_version_arn: "arn:aws:rtbfabric:us-east-1::modules/module-observe/versions/2.0.0"
                        .to_string(),
                    ..FlowModule::default()
                }],
            }),
        }
    }

    /// The names of the flows a reader reports.
    fn reported(flows: &Flows) -> Vec<String> {
        flows
            .reported()
            .unwrap_or_default()
            .iter()
            .map(|flow| flow.name.clone())
            .collect()
    }

    /// An accepted change is not in service until it commits, and what commits is
    /// the whole configuration the change stated — the empty one included.
    #[test]
    fn a_change_is_served_only_once_it_commits() {
        for (case, accepted, expected) in [
            ("one flow", vec![flow("First")], vec!["First".to_string()]),
            ("another flow", vec![flow("Second")], vec!["Second".to_string()]),
            ("no flow at all", Vec::new(), Vec::new()),
        ] {
            let mut flows = Flows::default();
            assert!(
                !flows.change_pending(),
                "case {case:?}: expected a link with no accepted change to have none awaiting it"
            );
            flows.accept(vec![flow("InService")]);
            flows.commit();
            assert_eq!(
                reported(&flows),
                vec!["InService".to_string()],
                "case {case:?}: expected the committed configuration to be the one served"
            );

            flows.accept(accepted);
            assert!(
                flows.change_pending(),
                "case {case:?}: expected the accepted change to be awaiting the link"
            );
            assert_eq!(
                reported(&flows),
                vec!["InService".to_string()],
                "case {case:?}: expected the link to serve its prior flows until the change commits"
            );
            flows.commit();
            assert_eq!(
                reported(&flows),
                expected,
                "case {case:?}: expected the change to be in service once it committed"
            );
            assert!(
                !flows.change_pending(),
                "case {case:?}: expected nothing to be awaiting the link once the change committed"
            );
        }
    }
}
