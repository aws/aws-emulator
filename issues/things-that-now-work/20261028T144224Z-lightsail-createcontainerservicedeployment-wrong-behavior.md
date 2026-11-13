Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a13c4bd7833d49550a5f70ddd7b2f501243c6c8c

# A deployment created on a DISABLED container service drives it back to RUNNING while isDisabled stays true

_Filed: 2026-10-28T14:42:24Z_
_Operation: CreateContainerServiceDeployment | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a13c4bd7833d49550a5f70ddd7b2f501243c6c8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateContainerServiceDeployment` sets the container service's state to `DEPLOYING` and enqueues one owed phase that settles it into `RUNNING` and activates the new deployment, unconditionally — it never looks at the service's own `is_disabled` flag.

So a service the caller disabled with `UpdateContainerService {"isDisabled": true}` (state `DISABLED`) answers a deployment create with `state: "DEPLOYING"`, and once the clock settles the owed phase `GetContainerServices` reports `state: "RUNNING"` and an `ACTIVE` `currentDeployment` while `isDisabled` is still `true`. The record then states two contradictory things at once: the service is disabled, and it is running an active container deployment.

Its sibling `UpdateContainerService` implements exactly the rule that is missing here: it computes where the service settles from `service.is_disabled` first, so `DISABLED` wins over `RUNNING`/`READY`. The state module carries the same rule in prose. `CreateContainerServiceDeployment` is the one writer of this field that skips it — nothing in `StoredContainerService::advance` compensates, because the owed phase it applies already names `RUNNING`.

## Evidence

The modelled meaning of the state this call overwrites — `smithy_query(query="doc", shape="com.amazonaws.lightsail#ContainerService", member="state")`:
```
DISABLED - The container service is disabled, and its active deployment and containers, if any, are shut down.
```

The handler, which never consults `is_disabled` — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_container_service_deployment.rs:217-230:
```
            // Launching a deployment is what puts a container service in
            // `DEPLOYING`, and `RUNNING` is the phase it settles into once that
            // deployment is active. ...
            service.state = crate::state::CONTAINER_SERVICE_DEPLOYING;
            service.owed.clear();
            service.owed.push_back(crate::state::OwedPhase {
                at_ms: now.saturating_add(crate::state::STATE_TRANSITION_MILLIS),
                state: crate::state::CONTAINER_SERVICE_RUNNING,
                activates_next_deployment: true,
            });
```

The sibling in the SAME crate that does apply the rule — src/ops/update_container_service.rs:300-336:
```
                // Where the service settles once the change is applied: a
                // disabled one is `DISABLED` ("the container service is
                // disabled, and its active deployment and containers, if any,
                // are shut down"), and an enabled one is `RUNNING` when it has
                // a deployment to serve ... and `READY` when it has none.
                let settles_into = if service.is_disabled {
                    crate::state::CONTAINER_SERVICE_DISABLED
                } else if service.current_deployment().is_some()
                    || service.next_deployment().is_some()
                {
                    crate::state::CONTAINER_SERVICE_RUNNING
```

The same rule stated in the state module — src/state.rs:8282-8299:
```
/// `ContainerServiceState` values this slice uses, from the modelled enum and the
/// `ContainerService$state` documentation: ... and
/// `DISABLED` for one that "is disabled, and its active deployment and
/// containers, if any, are shut down".
```

Nothing downstream repairs it: the clock applies the owed phase verbatim and activates the deployment — src/state.rs:9617-9647 (`fn advance`):
```
            self.owed.pop_front();
            self.state = phase.state;
            if !phase.activates_next_deployment {
                continue;
            }
...
            self.deployments[activating].state = DEPLOYMENT_STATE_ACTIVE;
```
and `is_disabled` (src/state.rs:9443) is left `true` and reported as such (src/state.rs:9557 `is_disabled: Some(self.is_disabled)`).

Test coverage: tests/all/container_service_update.rs:706-774 pins the sibling's rule ("RULE: an update neither launches nor retires a deployment — the one the service serves survives being disabled", asserting `wait_for_state(..., "DISABLED")` keeps `currentDeployment` and leaves `nextDeployment` null). No test in tests/all/container_service_deployment.rs or container_service_deployment_history.rs creates a deployment on a disabled service, so this path is unpinned.

## Steps to Reproduce

1. CreateContainerService in us-east-2 with {"serviceName": "svc", "power": "nano", "scale": 1} and poll GetContainerServices until state is READY.
2. UpdateContainerService with {"serviceName": "svc", "isDisabled": true} and poll until state is DISABLED (isDisabled true).
3. CreateContainerServiceDeployment with {"serviceName": "svc", "containers": {"web": {"image": "nginx:1.27", "ports": {"80": "HTTP"}}}, "publicEndpoint": {"containerName": "web", "containerPort": 80}}.
4. Observe the answered containerService: state "DEPLOYING" with isDisabled still true.
5. Wait past STATE_TRANSITION_MILLIS and call GetContainerServices: state is "RUNNING" and currentDeployment is ACTIVE, while isDisabled is still true — a service the account disabled is reported as running an active deployment, which the modelled DISABLED description says is shut down.
6. Compare with step 2's path through src/ops/update_container_service.rs:312-320, where the same service's settling state is computed from is_disabled.
