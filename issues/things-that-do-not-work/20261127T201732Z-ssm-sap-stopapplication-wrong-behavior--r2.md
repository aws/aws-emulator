Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 45e339952ace655163e1a6b7e05141367a36929a

# IncludeEc2InstanceShutdown skips a declared component's EC2 instance while the registration is still REGISTERING

_Filed: 2026-11-27T20:17:32Z_
_Operation: StopApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `45e339952ace655163e1a6b7e05141367a36929a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StopApplication` reads the instances to shut down from `StoredApplication::ec2_instance_ids()`, whose own doc states that BOTH the registration's `Instances` and the `Ec2InstanceId` of each DECLARED component are instances of the application and are what a call acting on "the associated Amazon EC2 instance" acts on. The body, however, chains `self.components()` — the DISCOVERED set, which `components()` deliberately reports as empty until `discovery_completed` is true.

This crate explicitly serves a stop against an application whose registration is still running (`application_control_available` has a dedicated exception for exactly that), so there is a reachable case in which `IncludeEc2InstanceShutdown: true` shuts down only the `Instances` entry and leaves every declared component's instance running — and `ListOperationEvents` then reports no milestone for those instances either. The identical call made after the registration settles shuts down both, so the same flag on the same registration means two different things depending on when it is called.

The fix is to read the instances the REGISTRATION declared (`described_components`), which is what the helper's doc says it reads, rather than the discovery-gated `components()`.

## Evidence

Crate doc that states the rule — services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:1105-1110:
```
    /// A registration associates an application with an EC2 instance in EITHER of two
    /// places, exactly as it describes a component ([`registered_components`]): the
    /// `Instances` the SAP system itself runs on, and the `Ec2InstanceId` each declared
    /// component runs on. Both are instances of this application, so both are what a
    /// call that acts on "the associated Amazon EC2 instance" acts on; an id named twice
    /// is one instance, so it is answered once.
```
The code under it reads the DISCOVERED set, not the declared one — state.rs:1113-1116:
```
        let declared = self
            .instances
            .iter()
            .chain(self.components().iter().map(|c| &c.ec2_instance_id));
```
and `components()` is empty until discovery completes — state.rs:1287-1293:
```
    pub fn components(&self) -> &[StoredComponent] {
        if self.discovery_completed {
            &self.described_components
        } else {
            &[]
        }
    }
```
The stop is nevertheless served while the registration is still running — state.rs:3876-3880:
```
    if control == ApplicationControl::Stop
        && application.status == APPLICATION_STATUS_REGISTERING
    {
        return Ok(());
    }
```
and that is the list the handler hands to EC2 — src/ops/stop_application.rs:356-360:
```
    let instances_to_shut_down = if input.include_ec2_instance_shutdown == Some(true) {
        application.ec2_instance_ids()
    } else {
        Vec::new()
    };
```
The handler's own module doc makes the same claim the code breaks — src/ops/stop_application.rs:12-14:
```
//! * `IncludeEc2InstanceShutdown` set to true stops the Amazon EC2 instances the
//!   registration associated with the application, through the service that owns them
```
Test coverage: `tests/all/ec2_instance_shutdown.rs:11-13` states the same rule ("The instances associated with an application are the ones its registration named: the `Instances` the SAP system runs on and the `Ec2InstanceId` of each declared component."), but every row goes through `register_activated(&emulator, row.id, &system_instance, &components)` (ec2_instance_shutdown.rs:356), i.e. a registration that has already settled — so the REGISTERING-time stop this crate deliberately permits is not covered by any case.

## Steps to Reproduce

1. Mount ssm-sap beside EC2 on one engine (as `tests/all/ec2_instance_shutdown.rs` does) and launch two running instances, A and B.
2. Call RegisterApplication with ApplicationType HANA, Instances=[A], Sid/SapInstanceNumber/Credentials set, and ComponentsInfo=[{ComponentType, Sid, Ec2InstanceId: B}].
3. Without waiting for TRANSITION_SPAN (the application is still REGISTERING), call StopApplication with that ApplicationId and IncludeEc2InstanceShutdown=true — it is accepted, per `application_control_available`'s REGISTERING exception.
4. Call EC2 DescribeInstances for A and B: A is stopping/stopped, B is still running.
5. Repeat the same registration, wait for the registration to settle to ACTIVATED first, then issue the identical stop: now both A and B are stopped — the same flag on the same registration reaches a different set of instances.
