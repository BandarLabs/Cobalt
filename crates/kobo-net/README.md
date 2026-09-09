# Runtime HTTPS transport

Cobalt applications submit tasks to the runtime. This crate performs authorized
network requests with TLS verification, bounded response sizes, cancellation and
connection deadlines. Applications must not call it directly or receive resolved
credential values.

## Requests with a body

`post_controlled` preserves the existing POST interface, including the explicit
retained-request mode used for long-lived seeks. `write_controlled` accepts a
closed `WriteMethod` (`Post`, `Put` or `Patch`) and otherwise uses the same
credential, header, size, deadline and cancellation checks.

PUT and PATCH send one request on a fresh connection. They do not follow
redirects or automatically resend a body after a connection failure. A lost
response does not establish whether the server applied a change. Callers must
reconcile server state or use an API's documented idempotency mechanism before
retrying a mutation. PUT/PATCH refuse retained-request mode before network I/O.

The runtime must authorize the exact method, URL, body and credential before
calling this API. A POST credential grant does not authorize PUT or PATCH.
The SDK exposes `Task::Update` with `UpdateMethod::Put` or `Patch`. Both the
simulator and native runtime dispatch it through a separate update backend.
Existing provider credential policies refuse updates until an app-specific
method/body/destination grant is reviewed. The SDK retry helper never silently
replays an update after an uncertain failure.

## Validation

Run `cargo test -p kobo-net`. Local TLS fixtures verify the HTTP verb, UTF-8 body
length, credential header, success response, authentication failures, rate-limit
delay, response ceiling, lost response and redirect refusal. Fixture journeys
run serially to avoid saturating the process-wide bounded resolver queue with
unrelated tests. Clients and servers within each journey still run concurrently;
unit tests separately exercise resolver overload, cancellation and cleanup.
