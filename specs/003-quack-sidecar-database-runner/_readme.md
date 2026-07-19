# Feature 003 implementation notes

## Current status

The official runner is available only behind a non-production compatibility
selection. Desktop production routing remains on the existing CLI path until
the controller bootstrap, packaging, parity and CutoverEvidence gates pass.

## Pool and profile

`WorkerPoolControl` owns every allocation. It atomically leases a ready warm
worker, or creates a single-use on-demand worker when no warm worker is ready.
Warm capacity is evaluated with `max(base_capacity, ceil(peak_5m * 1.20))` at
startup, after a profile save, and every five seconds. Only ready workers can
be selected for scale-in.

`RunnerResourcesProfile` is a complete, versioned workspace setting. Saves
are atomic: ready workers apply the newest version, starting workers do not
publish an obsolete version, and leased sessions drain before the next query
uses the latest effective profile. The permitted per-run query parallelism is
automatic or 1 through 8; it is not a sidecar pool size.

## Diagnostics

Autoscale events contain opaque IDs and safe counters only: evaluation reason,
outcome, active demand, five-minute peak, capacity counts, target, provisioned
and terminated workers. They never contain a sidecar endpoint, port, PID, path,
token, secret, SQL or capability. Settings diagnostics expose requested and
effective profile versions plus safe clamp reasons.

## Not a cutover record

This file is implementation documentation, not CutoverEvidence. Package
verification, lifecycle/parity coverage, benchmark evidence and named approval
remain required before the official runner can serve production traffic.
