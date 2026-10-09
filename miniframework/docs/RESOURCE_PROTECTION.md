# Optional resource protection

Enable independently in the app's dependency:

```toml
miniframework = { path = "../../crates/miniframework", features = ["rate-limit", "queue"] }
```

Neither module is a default feature and neither introduces dependencies or
background tasks. Existing transport limits remain enabled. Cargo features
are additive: another dependency on this crate can enable a feature in the
same build. `default-features = false` also disables the existing default
TLS/gzip features; it is not needed just to omit these new modules.

## Admission and rate limiting

`rate_limit::Limiter` provides thread-safe token buckets. Configure the
global and caller burst sizes/refill rates, a fixed caller-table capacity,
and a concurrent-handler limit. Work costs are integer units: an export
can cost more than a status poll. Costs must be nonzero and fit both
burst budgets. Rejected work consumes neither bucket's tokens.

`RateLimited::new(app, limits, classify)` implements `Service`. The classifier
returns `Some((identity, cost))` for protected requests or `None` for an
explicit bypass. Identity is a fixed 16-byte app-supplied value, derived
from a validated account/API key or a trusted gateway's identity. Do not
trust arbitrary client-supplied identity headers or forwarded IP headers.
Authentication still belongs to the app. If authentication runs only in
the inner handler, classify unauthenticated attempts into a shared bounded
anonymous bucket, then authenticate in the handler; never grant a fresh
allowance for every unverified token.

Caller exhaustion returns 429; global budget, table capacity and concurrency
exhaustion return 503. Both responses carry `Retry-After` and `Cache-Control:
no-store`. Misconfigured work cost returns 503. `Service` policies
(CORS, streaming, HTTPS, schema) delegate to the inner app. Metrics retain
the app's fields and append `admission_accepted`, `admission_rejected` and
`admission_active`. `stats()` exposes the same counters; acceptance/rejection
counters saturate at `u32::MAX` and reset on reboot.

Caller records are preallocated and bounded. A record can be recycled only
once its bucket is fully replenished; cycling identities cannot reset an
active caller's allowance. Table exhaustion sheds new identities. Pick
capacity for legitimate users and constrain anonymous identities upstream.

The wrapper covers app handler execution, not pending/running jobs or
response transmission. The current transport invokes handlers serially;
its concurrent-handler limit does not introduce parallelism. Queue worker
limits independently constrain background execution. Expensive work must
run outside the serving loop, and app-controlled queries/batches must bound
their own work. For distinct route budgets, use separate `Limiter` instances
in app admission logic; stacked wrappers consume each outer allowance before
an inner wrapper can refuse work.

`Site` built-ins, static assets and preflights handled by `Site` bypass the
wrapper. TLS/connection admission and request-body receipt also happen before
it. Public deployments should use a gateway with coarse limits and restrict
origin access; app middleware cannot protect a saturated link or undo a TLS
handshake. Keep admin/debug endpoints private at that boundary.

## Bounded jobs

`Queue::<BYTES>::new(queue::Limits { slots, workers, per_owner, lifetime })`
preallocates `slots * BYTES` payload/result bytes plus metadata. Each claimed
worker owns at most another `BYTES` bytes of copied input. The slot reuses
its payload storage for its result. App-created temporary buffers and HTTP
response buffers are additional memory. For example, eight 1024-byte slots
and one worker require 8 KiB of slot data plus metadata and up to 1 KiB of
claimed input. Measure actual heap headroom before deploying to a board.

The app controls its HTTP contract:

1. Authenticate and validate an eligible operation; serialize owned job data.
2. Call `submit(owner, payload)`. Only after success, return 202 with a job
   token and status URL. IDs are 128-bit random values from `getrandom`; failure
   to obtain randomness refuses submission. Tokens do not replace authorization.
3. An app worker calls `claim()` outside `Service::handle`. Claims are FIFO
   and bounded by `workers`. Execute without holding the queue lock, then
   `job.finish(result)` or `job.fail()`. Dropping a claim marks it failed.
4. A status route authenticates the owner and calls `poll(owner, id, output)`.
   It returns pending/running/done/failed; unknown, expired and another owner's
   jobs all appear absent. Results copy into caller-provided storage.
5. Completed/failed results occupy slots until expiry or `forget(owner, id)`.
   Polling does not consume a result, so a lost HTTP response can be retried.

Oversized payloads/results are refused; a result-size failure marks the job
failed and releases its worker allowance. Per-owner quotas include pending,
running and retained jobs. `stats()` reports pending/running/retained counts
for the app's metrics. Full queues should return 503; an owner's job quota
can return 429. Apply a polling rate budget too, and use client retry backoff.

Lifetime starts at submission and covers queue waiting, execution and result
retention. Pending and finished jobs expire automatically when the queue is
accessed. An expired running job becomes invisible to polling but retains
its slot and worker allowance until the claim is released, preventing a
stalled worker from causing unlimited replacement work. `job.expired()` is
a cooperative deadline; it cannot interrupt side effects. Workers need their
own downstream timeouts. Do not leak/forget worker claims.

This queue is volatile: jobs/results disappear on reboot. Durable recovery,
automatic retries, deduplication/idempotency keys and cancellation are app
policies, not provided by this module. Retryable writes need app-level
idempotency before they are exposed publicly. There is no implicit conversion
of arbitrary HTTP requests into jobs: an app chooses immediate execution,
queueing, or rejection, and documents both responses for its clients.

## Runnable desktop example

From `crates/miniframework`, in Git Bash:

```sh
RESOURCE_DEMO_TOKEN=local-demo cargo run --example resource_protection --features rate-limit,queue
```

The example binds only `127.0.0.1:18090`. In another shell:

```sh
curl -i -H 'Authorization: Bearer local-demo' --data-binary 'hello' http://127.0.0.1:18090/api/v1/jobs
curl -i -H 'Authorization: Bearer local-demo' 'http://127.0.0.1:18090/api/v1/jobs?id=TOKEN_FROM_POST'
```

POST returns 202 with `job_id`, `status_url` and `Location`. GET returns 202
while pending/running, 200 with uppercase text when complete, or 200 with a
failed status. Jobs live for 60 seconds; polling does not renew that lifetime.
The example uses a single authenticated demo principal, separate anonymous
rate budget, one desktop worker, bounded ASCII conversion, and queue metrics.
It is an integration example, not a public deployment configuration.

`make check` exercises each feature independently without defaults and both
features together with HTTP/2, alongside the existing checks.
