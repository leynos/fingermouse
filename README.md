# Fingermouse

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
- `--allowed-hosts` / `FINGERMOUSE_ALLOWED_HOSTS`: comma separated list of
  hostnames Fingermouse serves (defaults to the value of `default-host`).
- `--store-root` / `FINGERMOUSE_STORE_ROOT`: filesystem root used by the
  object store (default `./data`). The path is created when missing.
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
- `cargo clippy --all-targets --all-features -- -D warnings` for linting.
- `cargo test` for unit tests built with `rstest` and `tokio`.

The rate limiter depends on the `mockable` clock abstraction, enabling
deterministic control of timestamps in the test suite.
