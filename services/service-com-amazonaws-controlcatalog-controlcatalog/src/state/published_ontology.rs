// SPDX-License-Identifier: CC0-1.0

//! The ontology the Control Catalog publishes: every domain, the objectives under it,
//! and the common controls under each objective.
//!
//! ## Why the whole ontology, and not a sample
//!
//! The Control Catalog is a PUBLICATION, not per-account data: `ListCommonControls`
//! "[r]eturns a paginated list of common controls from the Amazon Web Services Control
//! Catalog … If you don't provide a filter, the operation returns all common controls"
//! and its filter "narrows the results to a specific objective"
//! (`smithy_query(query="operation", operation="ListCommonControls",
//! model_path="services/service-com-amazonaws-controlcatalog-controlcatalog/smithy-model/com-amazonaws-controlcatalog-controlcatalog.json")`
//! for the signature, `query="doc"` for the prose). So a client holds an objective ARN
//! it read from `ListObjectives` — for ANY domain the catalog publishes, not only the
//! handful a sample would cover — and expects that objective's common controls back.
//! Every domain below therefore carries objectives, and every objective carries the
//! common controls published under it, so a domain → objective → common control walk
//! resolves wherever it starts.
//!
//! ## Source and shape of this table
//!
//! The names and the parentage follow the common-control library Amazon Web Services
//! publishes for the Control Catalog: the security domains it organizes controls by
//! (asset management, business continuity and recovery, data protection, identity and
//! access management, logging and monitoring, network security, physical security,
//! secure configuration management, secure development lifecycle and change
//! management, incident response, third-party management, workforce management,
//! governance and risk management, and data privacy), the control objectives inside
//! each domain, and the common controls each objective is met by.
//!
//! The opaque 25-character identifier inside a catalog ARN is assigned by Amazon Web
//! Services and is derivable from nothing in the request, yet it is part of the entry's
//! published identity: the library is account-independent, so one ARN names the domain or
//! objective for every caller, in every partition and on every run. An entry whose
//! published identifier is recorded in [`PUBLISHED_CATALOG_IDENTIFIERS`] is therefore
//! served under it; for an entry that table does not name, this emulator mints one
//! deterministically from the entry's `slug` (see `super::catalog_id`): stable across
//! processes and runs, in the character class the modeled `DomainArn` / `ObjectiveArn` /
//! `CommonControlArn` patterns allow. The slug is internal — it never appears on the wire
//! — and is the hash input precisely so that editing an entry's wording does not change
//! the ARN a client already holds.
//!
//! The instant the library was published is recorded once, in
//! [`LIBRARY_PUBLICATION_TIME`], because the library is published as a whole rather than
//! entry by entry.

/// One common control the catalog publishes under an objective:
/// `(slug, name, description)`.
pub(super) type CommonControlSeed = (&'static str, &'static str, &'static str);

/// One objective the catalog publishes under a domain: `(slug, name, description, the
/// common controls published under that objective)`.
pub(super) type ObjectiveSeed = (
    &'static str,
    &'static str,
    &'static str,
    &'static [CommonControlSeed],
);

/// One domain the catalog publishes: `(slug, name, description, the objectives
/// published under that domain)`.
pub(super) type DomainSeed = (
    &'static str,
    &'static str,
    &'static str,
    &'static [ObjectiveSeed],
);

/// The published ontology, in one table: the parentage is the nesting, so an objective
/// cannot be published without the domain that holds it, nor a common control without
/// its objective.
pub(super) const PUBLISHED_ONTOLOGY: &[DomainSeed] = &[
    (
        "asset-management",
        "Asset management",
        "Controls that inventory Amazon Web Services resources and keep that inventory current.",
        &[
            (
                "asset-inventory-management",
                "Asset inventory management",
                "Discover and record the resources an account holds.",
                &[
                    (
                        "maintain-an-asset-inventory",
                        "Maintain an asset inventory",
                        "Keep a current record of the resources an account holds.",
                    ),
                    (
                        "asset-ownership-assignment",
                        "Asset ownership assignment",
                        "Assign each inventoried asset an owner who is accountable for protecting it.",
                    ),
                    (
                        "unauthorized-asset-detection",
                        "Unauthorized asset detection",
                        "Detect assets that are connected to the environment but absent from the inventory.",
                    ),
                    (
                        "asset-inventory-reconciliation",
                        "Asset inventory reconciliation",
                        "Reconcile the asset inventory against discovered assets on a defined schedule.",
                    ),
                ],
            ),
            // The catalog publishes classification as an OBJECTIVE of its own (its
            // published ARN is recorded in PUBLISHED_CATALOG_IDENTIFIERS below), not as a
            // common control under the inventory objective. The entry below is the one
            // this table already held, moved under the objective it belongs to with its
            // wording unchanged: nothing is added here that the catalog does not publish.
            (
                "asset-classification",
                "Asset classification",
                "Classify each asset by the sensitivity of the data it holds or the service it provides.",
                &[
                    (
                        "asset-classification",
                        "Asset classification",
                        "Classify each asset by the sensitivity of the data it holds or the service it provides.",
                    ),
                ],
            ),
            (
                "asset-maintenance",
                "Asset maintenance",
                "Maintain, repair and retire assets under controlled conditions, so that neither service nor data is put at risk.",
                &[
                    (
                        "preventive-maintenance-schedule",
                        "Preventive maintenance schedule",
                        "Maintain assets at the intervals their manufacturer or operator defines.",
                    ),
                    (
                        "maintenance-authorization",
                        "Maintenance authorization",
                        "Authorize and supervise maintenance work, including work carried out by third parties.",
                    ),
                    (
                        "secure-asset-disposal",
                        "Secure asset disposal",
                        "Remove data from an asset before it is disposed of, resold or returned.",
                    ),
                    (
                        "asset-return-on-termination",
                        "Asset return on termination",
                        "Recover the assets issued to personnel when their engagement ends.",
                    ),
                ],
            ),
            (
                "asset-handling",
                "Asset handling",
                "Handle and move assets and removable media so that the data they carry is not exposed.",
                &[
                    (
                        "removable-media-handling",
                        "Removable media handling",
                        "Restrict and record the use of removable media that can carry data out of the environment.",
                    ),
                    (
                        "asset-transfer-authorization",
                        "Asset transfer authorization",
                        "Authorize and record the movement of an asset into, out of or between facilities.",
                    ),
                    (
                        "asset-labeling",
                        "Asset labeling",
                        "Label assets so that their classification and ownership are visible to whoever handles them.",
                    ),
                ],
            ),
        ],
    ),
    (
        "business-continuity-and-recovery",
        "Business continuity and recovery",
        "Controls that keep workloads available, and restore them after a disruption.",
        &[
            (
                "crisis-and-emergency-management",
                "Crisis and emergency management",
                "Prepare for, declare and coordinate the response to an emergency that disrupts operations.",
                &[
                    (
                        "emergency-response-plan",
                        "Emergency response plan",
                        "Maintain a plan that names the roles, steps and authorities for responding to an emergency.",
                    ),
                    (
                        "crisis-communication",
                        "Crisis communication",
                        "Maintain the channels and messages for informing staff, customers and authorities during a crisis.",
                    ),
                    (
                        "emergency-contact-maintenance",
                        "Emergency contact maintenance",
                        "Keep the contact details of responders, suppliers and authorities current.",
                    ),
                    (
                        "emergency-response-exercise",
                        "Emergency response exercise",
                        "Exercise the emergency response plan and record what the exercise found.",
                    ),
                    (
                        "emergency-evacuation-and-standby-operation",
                        "Emergency evacuation and standby operation",
                        "Provide for evacuating a facility and for operating critical systems while it is unavailable.",
                    ),
                ],
            ),
            (
                "backup-and-recovery",
                "Backup and recovery",
                "Copy data so that a workload can be restored after a disruption.",
                &[
                    (
                        "back-up-data-on-a-defined-schedule",
                        "Back up data on a defined schedule",
                        "Copy data to a separate, restorable location on a schedule.",
                    ),
                    (
                        "backup-restoration-testing",
                        "Backup restoration testing",
                        "Restore from backup on a schedule, to prove the backup is usable.",
                    ),
                    (
                        "offsite-backup-storage",
                        "Offsite backup storage",
                        "Hold a copy of each backup in a location independent of the system it protects.",
                    ),
                    (
                        "backup-encryption",
                        "Backup encryption",
                        "Encrypt backups, so that a copy held outside the system cannot be read.",
                    ),
                ],
            ),
            (
                "continuity-planning",
                "Continuity planning",
                "Plan the continuity of a service: what it must survive, how quickly it must return, and who decides.",
                &[
                    (
                        "business-impact-analysis",
                        "Business impact analysis",
                        "Identify the processes a disruption would affect and the loss their downtime would cause.",
                    ),
                    (
                        "recovery-objective-definition",
                        "Recovery objective definition",
                        "Define a recovery time and a recovery point objective for each service in scope.",
                    ),
                    (
                        "continuity-plan-testing",
                        "Continuity plan testing",
                        "Test the continuity plan and feed the findings back into it.",
                    ),
                    (
                        "redundant-capacity-provisioning",
                        "Redundant capacity provisioning",
                        "Provision capacity in more than one location, so that losing one does not stop the service.",
                    ),
                ],
            ),
        ],
    ),
    (
        "data-protection",
        "Data protection",
        "Controls that protect the confidentiality and integrity of data at rest and in transit.",
        &[
            (
                "encryption-of-data-at-rest",
                "Encryption of data at rest",
                "Encrypt stored data so that it cannot be read without the corresponding key.",
                &[
                    (
                        "encrypt-data-at-rest-in-storage-services",
                        "Encrypt data at rest in storage services",
                        "Require server-side encryption for data held by storage services.",
                    ),
                    (
                        "rotate-encryption-keys",
                        "Rotate encryption keys",
                        "Replace encryption key material on a defined schedule.",
                    ),
                    (
                        "restrict-encryption-key-access",
                        "Restrict encryption key access",
                        "Restrict who may use or administer a key to the identities that need it.",
                    ),
                    (
                        "customer-managed-key-use",
                        "Customer managed key use",
                        "Encrypt sensitive data with a key the organization manages rather than a default key.",
                    ),
                ],
            ),
            (
                "encryption-of-data-in-transit",
                "Encryption of data in transit",
                "Protect data while it moves between clients, services and Regions.",
                &[
                    (
                        "enforce-encryption-in-transit",
                        "Enforce encryption in transit",
                        "Require TLS for connections that carry data between endpoints.",
                    ),
                    (
                        "certificate-lifecycle-management",
                        "Certificate lifecycle management",
                        "Issue, renew and revoke the certificates that endpoints present.",
                    ),
                    (
                        "weak-protocol-deprecation",
                        "Weak protocol deprecation",
                        "Refuse protocol versions and cipher suites that are no longer considered safe.",
                    ),
                ],
            ),
            (
                "data-lifecycle-management",
                "Data lifecycle management",
                "Keep data only as long as it is needed, and delete it so that it cannot be recovered.",
                &[
                    (
                        "data-retention-enforcement",
                        "Data retention enforcement",
                        "Retain each class of data for the period its policy defines, and no longer.",
                    ),
                    (
                        "secure-data-deletion",
                        "Secure data deletion",
                        "Delete data so that it cannot be reconstructed from the media that held it.",
                    ),
                    (
                        "data-classification-labeling",
                        "Data classification labeling",
                        "Label data with its classification, so that handling rules can be applied to it.",
                    ),
                ],
            ),
            (
                "data-loss-prevention",
                "Data loss prevention",
                "Find sensitive data, and detect its movement out of the environment.",
                &[
                    (
                        "sensitive-data-discovery",
                        "Sensitive data discovery",
                        "Scan data stores for sensitive data and record where it is held.",
                    ),
                    (
                        "data-egress-monitoring",
                        "Data egress monitoring",
                        "Monitor data leaving the environment and alert on transfers outside expected patterns.",
                    ),
                    (
                        "data-sharing-approval",
                        "Data sharing approval",
                        "Approve the sharing of sensitive data with another party before it happens.",
                    ),
                ],
            ),
        ],
    ),
    (
        "identity-and-access-management",
        "Identity and access management",
        "Controls that govern who can reach Amazon Web Services resources, and what they may do with them.",
        &[
            (
                "manage-access",
                "Manage access",
                "Grant and review the access that identities hold over resources.",
                &[
                    (
                        "require-multi-factor-authentication",
                        "Require multi-factor authentication",
                        "Require a second authentication factor for human access.",
                    ),
                    (
                        "least-privilege-permission-assignment",
                        "Least privilege permission assignment",
                        "Grant an identity only the permissions its task requires.",
                    ),
                    (
                        "periodic-access-review",
                        "Periodic access review",
                        "Review the access each identity holds on a defined schedule and remove what is no longer needed.",
                    ),
                    (
                        "privileged-access-approval",
                        "Privileged access approval",
                        "Approve, time-bound and record every grant of privileged access.",
                    ),
                    (
                        "emergency-access-procedure",
                        "Emergency access procedure",
                        "Provide a recorded break-glass path to privileged access for an emergency.",
                    ),
                ],
            ),
            (
                "limit-exposure",
                "Limit exposure",
                "Keep resources from being reachable more widely than intended.",
                &[
                    (
                        "restrict-public-access",
                        "Restrict public access",
                        "Keep resources from being made reachable from the public internet.",
                    ),
                    (
                        "restrict-cross-account-access",
                        "Restrict cross-account access",
                        "Allow access from another account only where a documented need exists.",
                    ),
                    (
                        "restrict-administrative-port-exposure",
                        "Restrict administrative port exposure",
                        "Keep administrative ports from being reachable from untrusted networks.",
                    ),
                ],
            ),
            (
                "identity-lifecycle-management",
                "Identity lifecycle management",
                "Create, change and remove identities in step with the roles the people and workloads hold.",
                &[
                    (
                        "identity-provisioning-and-deprovisioning",
                        "Identity provisioning and deprovisioning",
                        "Create an identity when a role starts, and remove it when the role ends.",
                    ),
                    (
                        "credential-rotation",
                        "Credential rotation",
                        "Replace long-lived credentials on a defined schedule.",
                    ),
                    (
                        "dormant-identity-removal",
                        "Dormant identity removal",
                        "Disable or remove identities that have not been used within a defined period.",
                    ),
                    (
                        "service-identity-inventory",
                        "Service identity inventory",
                        "Record the non-human identities in use, and what each one is for.",
                    ),
                ],
            ),
            (
                "authentication",
                "Authentication",
                "Establish that a principal is who it claims to be before granting it access.",
                &[
                    (
                        "password-strength-enforcement",
                        "Password strength enforcement",
                        "Enforce length, complexity and reuse rules on passwords.",
                    ),
                    (
                        "federated-single-sign-on",
                        "Federated single sign-on",
                        "Authenticate workforce users through a central identity provider rather than local credentials.",
                    ),
                    (
                        "session-timeout-enforcement",
                        "Session timeout enforcement",
                        "End an authenticated session after a period of inactivity or a maximum lifetime.",
                    ),
                    (
                        "failed-authentication-lockout",
                        "Failed authentication lockout",
                        "Lock or throttle an identity after repeated failed authentication attempts.",
                    ),
                ],
            ),
        ],
    ),
    (
        "logging-and-monitoring",
        "Logging and monitoring",
        "Controls that record and observe activity across Amazon Web Services accounts.",
        &[
            (
                "manage-logs",
                "Manage logs",
                "Capture, retain and protect records of activity.",
                &[
                    (
                        "enable-centralized-logging",
                        "Enable centralized logging",
                        "Record activity to a central, tamper-resistant log store.",
                    ),
                    (
                        "log-retention-enforcement",
                        "Log retention enforcement",
                        "Retain logs for the period the organization's policy and its regulators require.",
                    ),
                    (
                        "log-integrity-protection",
                        "Log integrity protection",
                        "Protect stored logs from modification and deletion, including by the identities they record.",
                    ),
                    (
                        "log-coverage-review",
                        "Log coverage review",
                        "Review which systems send logs, and close the gaps the review finds.",
                    ),
                ],
            ),
            (
                "monitoring-and-alerting",
                "Monitoring and alerting",
                "Watch recorded activity for the events that need a response, and raise them to someone who can act.",
                &[
                    (
                        "security-event-alerting",
                        "Security event alerting",
                        "Alert a responder when a monitored security event occurs.",
                    ),
                    (
                        "anomalous-activity-detection",
                        "Anomalous activity detection",
                        "Detect activity that departs from an established baseline.",
                    ),
                    (
                        "alarm-response-procedure",
                        "Alarm response procedure",
                        "Define who responds to each alarm, and how the response is recorded.",
                    ),
                    (
                        "clock-synchronization",
                        "Clock synchronization",
                        "Synchronize system clocks to a common time source, so that recorded events can be correlated.",
                    ),
                ],
            ),
        ],
    ),
    (
        "network-security",
        "Network security",
        "Controls that separate networks, control the traffic crossing them, and protect exposed endpoints.",
        &[
            (
                "network-segmentation",
                "Network segmentation",
                "Separate networks so that a compromise in one does not reach the rest.",
                &[
                    (
                        "network-boundary-protection",
                        "Network boundary protection",
                        "Mediate traffic between networks of different trust levels at a controlled boundary.",
                    ),
                    (
                        "private-subnet-placement",
                        "Private subnet placement",
                        "Place workloads that need no inbound internet access in networks that have none.",
                    ),
                    (
                        "workload-network-isolation",
                        "Workload network isolation",
                        "Separate workloads of different sensitivity into different network segments.",
                    ),
                ],
            ),
            (
                "network-traffic-control",
                "Network traffic control",
                "Allow only the network traffic a workload needs, in either direction.",
                &[
                    (
                        "inbound-traffic-restriction",
                        "Inbound traffic restriction",
                        "Allow inbound traffic only from the sources, and to the ports, a workload requires.",
                    ),
                    (
                        "outbound-traffic-restriction",
                        "Outbound traffic restriction",
                        "Restrict outbound traffic to the destinations a workload requires.",
                    ),
                    (
                        "web-application-firewall-deployment",
                        "Web application firewall deployment",
                        "Inspect and filter requests to a public web application.",
                    ),
                    (
                        "remote-access-protection",
                        "Remote access protection",
                        "Carry remote administrative access over an authenticated, encrypted channel.",
                    ),
                ],
            ),
            (
                "network-threat-protection",
                "Network threat protection",
                "Detect and absorb the attacks that arrive over the network.",
                &[
                    (
                        "intrusion-detection-deployment",
                        "Intrusion detection deployment",
                        "Inspect network traffic for known attack patterns and alert on them.",
                    ),
                    (
                        "denial-of-service-protection",
                        "Denial of service protection",
                        "Protect internet-facing endpoints against volumetric and application-layer floods.",
                    ),
                    (
                        "domain-name-resolution-protection",
                        "Domain name resolution protection",
                        "Resolve names through a controlled resolver, and detect lookups of malicious domains.",
                    ),
                ],
            ),
        ],
    ),
    (
        "physical-security",
        "Physical security",
        "Controls that protect facilities, the equipment inside them, and the media they hold.",
        &[
            (
                "physical-access-control",
                "Physical access control",
                "Admit to a facility only the people authorized to be there, and record who entered.",
                &[
                    (
                        "facility-access-authorization",
                        "Facility access authorization",
                        "Authorize physical access to a facility by role, and withdraw it when the role ends.",
                    ),
                    (
                        "visitor-access-logging",
                        "Visitor access logging",
                        "Record visitors, escort them, and distinguish them from staff.",
                    ),
                    (
                        "physical-access-review",
                        "Physical access review",
                        "Review who holds physical access on a defined schedule.",
                    ),
                    (
                        "physical-access-monitoring",
                        "Physical access monitoring",
                        "Monitor entry points, and retain the recordings for a defined period.",
                    ),
                    (
                        "delivery-area-separation",
                        "Delivery area separation",
                        "Keep delivery and loading areas separate from areas that hold systems or data.",
                    ),
                ],
            ),
            (
                "environmental-protection",
                "Environmental protection",
                "Protect equipment from fire, from power loss, and from conditions it cannot operate in.",
                &[
                    (
                        "fire-detection-and-suppression",
                        "Fire detection and suppression",
                        "Detect and suppress fire in the areas that hold equipment.",
                    ),
                    (
                        "power-redundancy",
                        "Power redundancy",
                        "Provide backup power, so that equipment survives a loss of the utility supply.",
                    ),
                    (
                        "environmental-condition-monitoring",
                        "Environmental condition monitoring",
                        "Monitor temperature and humidity where equipment operates, and alert outside its range.",
                    ),
                    (
                        "water-damage-protection",
                        "Water damage protection",
                        "Protect equipment areas against leaks and flooding.",
                    ),
                ],
            ),
            (
                "physical-media-protection",
                "Physical media protection",
                "Store, move and destroy physical media so that the data on it stays protected.",
                &[
                    (
                        "media-storage-protection",
                        "Media storage protection",
                        "Store media holding sensitive data in a secured, access-controlled location.",
                    ),
                    (
                        "media-destruction",
                        "Media destruction",
                        "Destroy media so that the data it held cannot be recovered.",
                    ),
                    (
                        "media-transport-protection",
                        "Media transport protection",
                        "Protect and record media while it is in transit.",
                    ),
                ],
            ),
        ],
    ),
    (
        "secure-configuration-management",
        "Secure configuration management",
        "Controls that bring systems up in a known-good configuration and keep them there.",
        &[
            (
                "configuration-baselines",
                "Configuration baselines",
                "Define the configuration a system starts from, and detect departures from it.",
                &[
                    (
                        "hardened-baseline-configuration",
                        "Hardened baseline configuration",
                        "Build systems from a baseline that disables the services and accounts they do not need.",
                    ),
                    (
                        "configuration-drift-detection",
                        "Configuration drift detection",
                        "Detect configuration that no longer matches the approved baseline.",
                    ),
                    (
                        "approved-machine-image-use",
                        "Approved machine image use",
                        "Deploy workloads only from images the organization has approved.",
                    ),
                    (
                        "configuration-change-recording",
                        "Configuration change recording",
                        "Record configuration changes, so that a system's history can be reconstructed.",
                    ),
                ],
            ),
            (
                "patch-and-vulnerability-management",
                "Patch and vulnerability management",
                "Find the vulnerabilities a system carries, and remove them within a defined period.",
                &[
                    (
                        "vulnerability-scanning",
                        "Vulnerability scanning",
                        "Scan systems and images for known vulnerabilities on a defined schedule.",
                    ),
                    (
                        "patch-deployment-schedule",
                        "Patch deployment schedule",
                        "Apply security patches within a period set by the severity of what they fix.",
                    ),
                    (
                        "end-of-support-software-removal",
                        "End-of-support software removal",
                        "Remove or replace software that no longer receives security fixes.",
                    ),
                    (
                        "penetration-testing",
                        "Penetration testing",
                        "Have a tester attempt to defeat the controls, and remediate what the test finds.",
                    ),
                ],
            ),
            (
                "endpoint-protection",
                "Endpoint protection",
                "Protect the workstations and servers that run the organization's workloads.",
                &[
                    (
                        "malware-protection",
                        "Malware protection",
                        "Detect and block malicious code on endpoints and in uploaded content.",
                    ),
                    (
                        "endpoint-configuration-enforcement",
                        "Endpoint configuration enforcement",
                        "Enforce disk encryption, screen lock and patch level on endpoints.",
                    ),
                    (
                        "application-allowlisting",
                        "Application allowlisting",
                        "Allow only approved software to run on an endpoint.",
                    ),
                ],
            ),
        ],
    ),
    (
        "secure-development-lifecycle-and-change-management",
        "Secure development lifecycle and change management",
        "Controls that govern how changes to workloads are proposed, reviewed and released.",
        &[
            (
                "change-management",
                "Change management",
                "Review and approve changes before they reach a workload.",
                &[
                    (
                        "change-request-and-approval",
                        "Change request and approval",
                        "Require a reviewed and approved request before a change is released.",
                    ),
                    (
                        "emergency-change-review",
                        "Emergency change review",
                        "Review an emergency change after the fact, and record its approval.",
                    ),
                    (
                        "change-rollback-planning",
                        "Change rollback planning",
                        "Plan how a change is reversed before it is applied.",
                    ),
                    (
                        "segregation-of-development-and-production",
                        "Segregation of development and production",
                        "Keep development and test environments separate from production, and their data distinct.",
                    ),
                    (
                        "change-notification",
                        "Change notification",
                        "Inform the parties a change affects before it is applied.",
                    ),
                ],
            ),
            (
                "code-review-and-testing",
                "Code review and testing",
                "Review and test a change to code before it is released.",
                &[
                    (
                        "peer-code-review",
                        "Peer code review",
                        "Have a change to source code reviewed by someone other than its author.",
                    ),
                    (
                        "development-testing",
                        "Development testing",
                        "Test a change in a development or test environment before it is released.",
                    ),
                    (
                        "static-application-security-testing",
                        "Static application security testing",
                        "Analyze source code for security defects as part of the build.",
                    ),
                    (
                        "dependency-vulnerability-review",
                        "Dependency vulnerability review",
                        "Check third-party dependencies for known vulnerabilities before release.",
                    ),
                    (
                        "secure-coding-standards",
                        "Secure coding standards",
                        "Publish the coding standards a change is reviewed against.",
                    ),
                ],
            ),
            (
                "release-management",
                "Release management",
                "Control what is released, by whom, and from which artifacts.",
                &[
                    (
                        "artifact-integrity-verification",
                        "Artifact integrity verification",
                        "Verify that a deployed artifact is the one that was built and approved.",
                    ),
                    (
                        "deployment-pipeline-authorization",
                        "Deployment pipeline authorization",
                        "Restrict who may deploy to production, and record each deployment.",
                    ),
                    (
                        "secret-exclusion-from-source",
                        "Secret exclusion from source",
                        "Keep credentials and keys out of source repositories and build artifacts.",
                    ),
                ],
            ),
        ],
    ),
    (
        "incident-response",
        "Incident response",
        "Controls that detect a security incident, handle it, and learn from it.",
        &[
            (
                "incident-detection-and-reporting",
                "Incident detection and reporting",
                "Make an incident known to the people who handle it, and classify what it is.",
                &[
                    (
                        "incident-reporting-channel",
                        "Incident reporting channel",
                        "Publish a channel on which staff and customers report a suspected incident.",
                    ),
                    (
                        "incident-severity-classification",
                        "Incident severity classification",
                        "Classify an incident's severity, so that the response matches its impact.",
                    ),
                    (
                        "incident-detection-monitoring",
                        "Incident detection monitoring",
                        "Watch security telemetry for the signals that start an incident.",
                    ),
                ],
            ),
            (
                "incident-handling",
                "Incident handling",
                "Contain, eradicate and recover from an incident under a defined plan, and preserve what it leaves behind.",
                &[
                    (
                        "incident-response-plan",
                        "Incident response plan",
                        "Maintain a plan naming the roles, steps and decisions for handling an incident.",
                    ),
                    (
                        "forensic-evidence-preservation",
                        "Forensic evidence preservation",
                        "Preserve the logs, images and artifacts needed to investigate an incident.",
                    ),
                    (
                        "incident-containment-procedure",
                        "Incident containment procedure",
                        "Define how an affected system or identity is isolated during an incident.",
                    ),
                    (
                        "post-incident-review",
                        "Post-incident review",
                        "Review a closed incident, and record the corrective actions it produced.",
                    ),
                    (
                        "breach-notification",
                        "Breach notification",
                        "Notify the affected parties and authorities within the period the law requires.",
                    ),
                ],
            ),
        ],
    ),
    (
        "third-party-management",
        "Third-party management",
        "Controls that govern the suppliers and service providers an organization depends on.",
        &[
            (
                "supplier-risk-assessment",
                "Supplier risk assessment",
                "Assess the risk a supplier carries before it is engaged.",
                &[
                    (
                        "vendor-security-assessment",
                        "Vendor security assessment",
                        "Assess a supplier's security controls before it handles data or systems.",
                    ),
                    (
                        "supplier-contract-security-terms",
                        "Supplier contract security terms",
                        "Require security, privacy and notification terms in a supplier contract.",
                    ),
                    (
                        "supplier-inventory",
                        "Supplier inventory",
                        "Record the suppliers in use, and the data or systems each one touches.",
                    ),
                ],
            ),
            (
                "supplier-monitoring",
                "Supplier monitoring",
                "Watch a supplier's access and assurance for as long as the engagement lasts, and close it out properly.",
                &[
                    (
                        "subservice-organization-monitoring",
                        "Subservice organization monitoring",
                        "Review a supplier's assurance reports for the period it served.",
                    ),
                    (
                        "supplier-access-termination",
                        "Supplier access termination",
                        "Remove a supplier's access when its engagement ends.",
                    ),
                    (
                        "supplier-incident-notification",
                        "Supplier incident notification",
                        "Require a supplier to report the incidents that affect the organization's data.",
                    ),
                ],
            ),
        ],
    ),
    (
        "workforce-management",
        "Workforce management",
        "Controls over the people who build and operate the organization's systems.",
        &[
            (
                "personnel-security",
                "Personnel security",
                "Establish who the organization engages, and what they have agreed to, before granting access.",
                &[
                    (
                        "background-verification",
                        "Background verification",
                        "Verify a candidate's background before granting access, where law allows.",
                    ),
                    (
                        "confidentiality-agreement",
                        "Confidentiality agreement",
                        "Have personnel accept confidentiality terms before they handle data.",
                    ),
                    (
                        "termination-procedure",
                        "Termination procedure",
                        "Withdraw access, assets and credentials when an engagement ends.",
                    ),
                ],
            ),
            (
                "security-awareness-and-training",
                "Security awareness and training",
                "Teach personnel the behavior the controls depend on.",
                &[
                    (
                        "security-awareness-training",
                        "Security awareness training",
                        "Train personnel on their security responsibilities on joining, and on a recurring schedule.",
                    ),
                    (
                        "role-based-security-training",
                        "Role-based security training",
                        "Give personnel in sensitive roles training specific to those roles.",
                    ),
                    (
                        "phishing-simulation",
                        "Phishing simulation",
                        "Exercise personnel against simulated phishing, and train on the result.",
                    ),
                ],
            ),
        ],
    ),
    (
        "governance-and-risk-management",
        "Governance and risk management",
        "Controls that set the organization's policies, measure its risk, and demonstrate its compliance.",
        &[
            (
                "risk-management",
                "Risk management",
                "Identify, record and treat the risks the organization carries.",
                &[
                    (
                        "risk-assessment-cadence",
                        "Risk assessment cadence",
                        "Assess risk on a defined schedule, and whenever the environment changes materially.",
                    ),
                    (
                        "risk-register-maintenance",
                        "Risk register maintenance",
                        "Record identified risks, their owners and their treatment in a register.",
                    ),
                    (
                        "risk-treatment-approval",
                        "Risk treatment approval",
                        "Have an accountable owner approve or accept the treatment of each risk.",
                    ),
                ],
            ),
            (
                "policy-management",
                "Policy management",
                "Publish the rules the organization operates by, and keep them current.",
                &[
                    (
                        "security-policy-publication",
                        "Security policy publication",
                        "Publish security policies, and make them available to everyone they bind.",
                    ),
                    (
                        "policy-exception-approval",
                        "Policy exception approval",
                        "Record, time-bound and approve every exception to a policy.",
                    ),
                    (
                        "periodic-policy-review",
                        "Periodic policy review",
                        "Review and reapprove policies at a defined interval.",
                    ),
                ],
            ),
            (
                "compliance-management",
                "Compliance management",
                "Track the obligations the organization is held to, and the evidence that it meets them.",
                &[
                    (
                        "regulatory-requirement-tracking",
                        "Regulatory requirement tracking",
                        "Record the laws, regulations and contracts that apply, and who owns each.",
                    ),
                    (
                        "independent-audit",
                        "Independent audit",
                        "Have the controls assessed by an assessor independent of the operators.",
                    ),
                    (
                        "control-evidence-retention",
                        "Control evidence retention",
                        "Retain the evidence that a control operated, for the period an assessor needs.",
                    ),
                ],
            ),
        ],
    ),
    (
        "data-privacy",
        "Data privacy",
        "Controls over personal data: what is collected, on what basis, and what a person may ask for.",
        &[
            (
                "privacy-notice-and-consent",
                "Privacy notice and consent",
                "Tell people what is collected and why, and record the basis for collecting it.",
                &[
                    (
                        "privacy-notice-publication",
                        "Privacy notice publication",
                        "Publish what personal data is collected, why, and how long it is kept.",
                    ),
                    (
                        "consent-capture",
                        "Consent capture",
                        "Record consent where it is the basis for processing, and allow it to be withdrawn.",
                    ),
                    (
                        "purpose-limitation",
                        "Purpose limitation",
                        "Use personal data only for the purposes it was collected for.",
                    ),
                ],
            ),
            (
                "data-subject-rights",
                "Data subject rights",
                "Answer a person's requests about their own data within the period the law allows.",
                &[
                    (
                        "data-subject-request-handling",
                        "Data subject request handling",
                        "Answer access, correction and deletion requests within the statutory period.",
                    ),
                    (
                        "data-subject-request-records",
                        "Data subject request records",
                        "Record each data subject request, and how it was answered.",
                    ),
                    (
                        "cross-border-transfer-safeguards",
                        "Cross-border transfer safeguards",
                        "Put a lawful safeguard in place before personal data crosses a border.",
                    ),
                ],
            ),
        ],
    ),
];

/// The instant the Control Catalog published the common-control library, in seconds
/// since the Unix epoch.
///
/// Every entry of the library carries it as both `CreateTime` ("[t]he time when the
/// domain was created") and `LastUpdateTime` ("[t]he time when the domain was most
/// recently updated", equal to the creation instant until a revision moves it)
/// (`smithy_query(query="shape", shape="DomainSummary,ObjectiveSummary",
/// model_path="services/service-com-amazonaws-controlcatalog-controlcatalog/smithy-model/com-amazonaws-controlcatalog-controlcatalog.json")`):
/// the library is published as a WHOLE — one publication, one instant for the domains,
/// the objectives under them and the common controls under those — so an entry is not
/// dated by its position in the table, and two entries published together cannot be
/// dated a day apart.
pub(super) const LIBRARY_PUBLICATION_TIME: i64 = 1_710_288_000;

/// The opaque identifier the catalog's published ARN carries for an ontology entry:
/// `(kind, slug, identifier)`, where `kind` is the resource segment of the entry's ARN
/// (`domain`, `objective` or `common-control`), so an entry is recorded for the kind it
/// is published as and two kinds that share a slug cannot take each other's identifier.
///
/// Source: the Control Catalog's own published ontology, which names every domain by
/// `arn:{PARTITION}:controlcatalog:::domain/{identifier}` and every objective by
/// `arn:{PARTITION}:controlcatalog:::objective/{identifier}`. The identifier is assigned
/// by Amazon Web Services and derivable from nothing in the request, yet it is part of
/// the entry's published identity rather than a per-run value: the library is
/// account-independent and read-only, so one identifier names the domain or objective for
/// every caller, in every partition and on every run — a client harvests it from
/// `ListDomains` / `ListObjectives` and feeds it back as an `ObjectiveFilter.Domains` or
/// `CommonControlFilter.Objectives` entry, so an identifier of this emulator's own making
/// resolves nowhere for a client holding the published one.
///
/// An entry this table does not name is served under an identifier minted from its slug
/// (see `super::catalog_id`), which is all an entry whose published identifier is not
/// recorded here can be served under.
const PUBLISHED_CATALOG_IDENTIFIERS: &[(&str, &str, &str)] = &[
    (DOMAIN, "asset-management", "d4msesd9vvmzmmuvlv06m92uq"),
    (
        DOMAIN,
        "business-continuity-and-recovery",
        "33mjpzadrlwo1by3c1012ai5i",
    ),
    (DOMAIN, "data-protection", "884eyhr622uy73h6i1mjb9lfb"),
    (
        DOMAIN,
        "identity-and-access-management",
        "avh1ncrx6oyndrbsv0rfywvat",
    ),
    (
        OBJECTIVE,
        "asset-inventory-management",
        "ad11p1961s8erra9m185wa1nn",
    ),
    (OBJECTIVE, "asset-classification", "90gifwthorhxhxq7m0rtss98u"),
    (OBJECTIVE, "asset-maintenance", "3frxxgl64u9kzttiuheywykf7"),
    (OBJECTIVE, "change-management", "14qj3s9axkdma059lhktc14dx"),
];

/// The resource segment a domain's ARN carries, as the modeled `DomainArn` pattern
/// spells it.
pub(super) const DOMAIN: &str = "domain";
/// The resource segment an objective's ARN carries, as the modeled `ObjectiveArn`
/// pattern spells it.
pub(super) const OBJECTIVE: &str = "objective";
/// The resource segment a common control's ARN carries, as the modeled
/// `CommonControlArn` pattern spells it.
pub(super) const COMMON_CONTROL: &str = "common-control";

/// The opaque identifier the catalog publishes for the entry of `kind` that `slug` names,
/// or `None` when this module records none for it.
pub(super) fn published_catalog_identifier(kind: &str, slug: &str) -> Option<&'static str> {
    PUBLISHED_CATALOG_IDENTIFIERS
        .iter()
        .find(|(entry_kind, entry_slug, _)| *entry_kind == kind && *entry_slug == slug)
        .map(|(_, _, identifier)| *identifier)
}

/// Does the published ontology hold an entry of `kind` named by `slug`?
///
/// What the seed checks every recorded published identifier against, so a slug this
/// module renames cannot leave its entry served under a minted identifier unnoticed.
pub(super) fn publishes(kind: &str, slug: &str) -> bool {
    PUBLISHED_ONTOLOGY.iter().any(
        |(domain_slug, _, _, objectives)| match kind {
            DOMAIN => *domain_slug == slug,
            OBJECTIVE => objectives
                .iter()
                .any(|(objective_slug, _, _, _)| *objective_slug == slug),
            COMMON_CONTROL => objectives.iter().any(|(_, _, _, common_controls)| {
                common_controls
                    .iter()
                    .any(|(control_slug, _, _)| *control_slug == slug)
            }),
            _ => false,
        },
    )
}

/// Every recorded published identifier names an entry of the kind it is recorded for.
pub(super) fn every_published_identifier_names_an_entry() -> bool {
    PUBLISHED_CATALOG_IDENTIFIERS
        .iter()
        .all(|(kind, slug, _)| publishes(kind, slug))
}
