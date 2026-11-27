Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.exec-interactive-stdout-round-trip
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.exec-interactive-stdout-round-trip|run#21|AssertionError|assert f"META42-{nonce}=0" in out, (

# ecs: a container has no task metadata endpoint, so `$ECS_CONTAINER_METADATA_URI_V4` is unset and a program that reads its own task metadata fetches nothing

> _Filed while fixing `20261119T173202Z-ecs-5-net-new-causes-executecommand-never-opens.md`. That change serves the execute-command session's data channel, so a client can now type into a container and read what it wrote — which is what makes this gap reachable: the recorded check gets its shell, and the one command of it that fails is the one reading the task metadata endpoint. Serving that endpoint is a capability of its own rather than a part of the session, so it is tracked here._

## Operation
RunTask, ExecuteCommand

## Description
AWS: every container of a task Amazon ECS launches is given the environment variable
`ECS_CONTAINER_METADATA_URI_V4`, "the task metadata endpoint version 4" — an HTTP endpoint inside
the task that answers `GET <uri>` with that container's metadata and `GET <uri>/task` with the
task's (its `Cluster`, `TaskARN`, `Family`, `Revision`, `DesiredStatus`, `KnownStatus`, `Limits`,
`AvailabilityZone` and one entry per container). A program in the container reads its own identity
from it, which is what the recorded check does over an exec session:
`wget -qO- $ECS_CONTAINER_METADATA_URI_V4/task; echo META42-<nonce>=$?` → `META42-<nonce>=0`.

Emulator: nothing sets that variable (no `ECS_CONTAINER_METADATA_URI` anywhere in
`services/`), and nothing serves such an endpoint, so the word expands to nothing, the fetch is a
request to no address, and the probe reports no status at all — `META42-<nonce>=` with nothing
after the `=`. The container's own session already HAS the client the fetch is written with
(`src/state/container_http_client.rs`) and the records a server is published in
(`services/common/src/served_http_addresses.rs`), so what is missing is the endpoint itself and the
variable naming it.

## Why it matters
Reading its own task metadata is how a container learns which task it is — the documented way a
program gets its task ARN, its family and revision, its limits and its availability zone without
calling the ECS API. Any recorded behaviour that turns on a container knowing its own identity (a
task that writes its own ARN, a sidecar that reports the task it belongs to, a probe that reads the
task's limits) is unobservable, and a client cannot tell "the endpoint answered nothing" from "the
program did not run".

## What a fix needs
1. A SERVER for the endpoint, published in the shared records a container's own HTTP client reads
   (`::service_common::served_http_addresses::ServedAddresses::publish`), implementing `ServesHttp`
   over this store's own task record: `GET /v4/<id>` for the container's metadata and
   `GET /v4/<id>/task` for the task's, every member computed from the stored task, revision and
   container rather than from the request.
2. The VARIABLE, set in the environment of every container of a started task
   (`src/state/container_runtime.rs`, beside `HOSTNAME`), and in the environment of an
   execute-command session of that container (`src/state/exec_sessions/sessions.rs`), so a program
   and an exec session read the same endpoint.
3. The endpoint's lifetime: published as the container starts and withdrawn with the run, so a task
   that is gone leaves nothing answering.

## Steps to Reproduce
1. `RegisterTaskDefinition` with one container and a task role; `RunTask` with
   `enableExecuteCommand=true`; wait for `RUNNING` with the `ExecuteCommandAgent` agent running.
2. `ExecuteCommand(interactive=true, command="/bin/sh")` and open the session's `streamUrl`.
3. Type `wget -qO- $ECS_CONTAINER_METADATA_URI_V4/task; echo META=$?` → the stream carries `META=`
   with no status, where AWS carries the task metadata document and `META=0`.
