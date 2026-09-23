# Fingermouse

[![Ask DeepWiki](https://deepwiki.com/badge.svg)](
https://deepwiki.com/leynos/fingermouse)

Fingermouse is a Tokio-based finger server that answers classic finger queries
over TCP. User details are sourced from a TOML document hosted in an
`object_store` backend, and optional `.plan` files are delivered when the
client requests verbose output.

## Features

- Fully asynchronous request handling on top of `tokio`.
- Object store integration via `object_store::local::LocalFileSystem`, ready
  to swap for cloud backends in the future.
- Strict username and hostname validation to prevent traversal or wildcard
  enumeration.
- Per-IP sliding window rate limiting backed by injectable clocks for
  deterministic tests.
- Structured logging through `tracing` with environment-controlled log levels.

## Configuration

The executable reads configuration from command-line options or matching
environment variables:

- `--listen` / `FINGERMOUSE_LISTEN`: TCP socket address to bind to (default
  `0.0.0.0:7979`).
- `--default-host` / `FINGERMOUSE_DEFAULT_HOST`: hostname returned when the
  client omits one (default `localhost`).
- `--allowed-hosts` / `FINGERMOUSE_ALLOWED_HOSTS`: comma-separated list of
  hostnames Fingermouse serves (defaults to the value of `default-host`).
- `--store-root` / `FINGERMOUSE_STORE_ROOT`: filesystem root used by the
  object store (default `./data`). The path is created if missing.
- `--profile-prefix` / `FINGERMOUSE_PROFILE_PREFIX`: directory containing
  `<username>.toml` profile files (default `profiles`).
- `--plan-prefix` / `FINGERMOUSE_PLAN_PREFIX`: directory containing
  `<username>.plan` files (default `plans`).
- `--rate-limit` / `FINGERMOUSE_RATE_LIMIT`: permitted requests per
  window for each IP address (default `30`).
- `--rate-window-secs` / `FINGERMOUSE_RATE_WINDOW_SECS`: length of the rate-
  limiting window (default `60`).
- `--rate-capacity` / `FINGERMOUSE_RATE_CAPACITY`: maximum distinct client IPs
  retained before eviction (default `8192`).
- `--metrics-listen` / `FINGERMOUSE_METRICS_LISTEN`: optional socket address
  that exposes Prometheus metrics (disabled by default).
- `--request-timeout-ms` / `FINGERMOUSE_REQUEST_TIMEOUT_MS`: read timeout for
  client queries (default `3000`).
- `--max-request-bytes` / `FINGERMOUSE_MAX_REQUEST_BYTES`: maximum accepted
  query size (default `512`).

Profiles must expose a `username` key matching the requested account. All other
string keys are returned verbatim as `Key: Value` pairs in the finger response.
When `/W` is present, Fingermouse appends the user's plan, reports
`(empty plan)` for blank files, or `(no plan)` when the plan is missing.

## Storage Layout

```plaintext
<store-root>/
  profiles/
    alice.toml
  plans/
    alice.plan
```

Each profile TOML file must be UTF-8 and contain string values only. Plans are
treated as UTF-8 text and sanitized to printable ASCII to avoid terminal
control sequences.

## Building

Fingermouse uses crates that support static linking. To produce a `musl` binary
for a scratch container, use `cargo zigbuild`:

```bash
cargo install cargo-zigbuild
cargo zigbuild --target x86_64-unknown-linux-musl --release
```

The resulting binary in `target/x86_64-unknown-linux-musl/release` can be
copied into a `FROM scratch` image together with the `profiles/` and `plans/`
directories.

## Testing

Fast feedback is available through:

- `cargo fmt` for formatting checks.
- `make lint` for linting, which runs Clippy
  (`cargo clippy --all-targets --all-features -- -D warnings`) followed by the
  [Whitaker](https://github.com/leynos/whitaker) Dylint suite with warnings
  denied.
- `cargo test` for unit tests built with `rstest` and `tokio`.
- `make spelling` for en-GB-oxendict prose spelling. The generated
  `typos.toml` starts from the shared estate dictionary, refreshes its
  untracked local cache only when the authority is newer, and then applies the
  narrow repository policy in `typos.local.toml`.
- `make workflow-contracts` for the coverage workflow contract described
  below.

Linting requires the Whitaker suite. Install it with
[`whitaker-installer`](https://github.com/leynos/whitaker):

```bash
cargo binstall --no-confirm --locked whitaker-installer  # or: cargo install --locked whitaker-installer
whitaker-installer
```

This provisions the pinned toolchain, `cargo-dylint`, and the `whitaker`
wrapper used by `make lint`.

The rate limiter depends on the `mockable` clock abstraction, enabling
deterministic control of timestamps in the test suite.

## Coverage ownership

The trunk owns both persistent coverage outputs. On a push to `main`,
`.github/workflows/coverage-main.yml` measures coverage, writes the ratchet
baseline, and uploads the report to CodeScene. Pull-request CI measures the
same selection only to compare it with that baseline: it archives no report,
never calls CodeScene, and never receives `CS_ACCESS_TOKEN`. The call is what
moves to the trunk, not the archive: the uploader pins the `cs-coverage`
archive by digest, but the client refuses to run whenever CodeScene's API
changes shape, and on the trunk such a change no longer fails every pull
request.

The publisher never binds the token in an `env` block, because the uploader is
a composite action that would pass a step's environment on to its nested steps.
A check step writes whether the secret is set, the upload runs only when it is
and only for `refs/heads/main`, and the token reaches the uploader solely as its
`access-token` input. Runs share one concurrency group per ref and are never
cancelled, so triggered runs (a push or a dispatch) upload in commit order. A
manual re-run of an older run is an operator action: it republishes that
commit's coverage and baseline until the next push supersedes it.

Two gaps are known and accepted. Merges made by the Dependabot automerge
workflow with `GITHUB_TOKEN` fire no push, so they reach the publisher only
through a dispatch or the next ordinary push. A dispatch that replaces a
pending push uploads the same or a newer commit, but the shared action writes
the baseline only on a push, so the baseline stays one push behind until the
next one.

`make workflow-contracts`, which pull-request CI runs, holds this shape in
`tests/workflow_contracts/`. It reads every workflow strictly (a repeated key
is an error) and follows local reusable-workflow calls transitively, so a
called workflow cannot reach CodeScene on a pull request's behalf.
