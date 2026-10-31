# Developers' guide

This guide records how fingermouse is built and checked.

## The build standard

Development, test, lint, and typecheck builds use the parallel `rustc` frontend
(`-Zthreads=8`) and, on Linux, the `mold` linker (`-Clink-arg=-fuse-ld=mold`).
These are defaults in `.cargo/config.toml`, which Cargo discovers on its own,
so a bare `cargo build` gets them. `mold` ships for Linux only, so the linker
flag lives in a Linux-only table and macOS and Windows keep their platform
linker. Cargo selects one `rustflags` source rather than merging them, so every
source repeats the same flags apart from the linker.

An assigned `RUSTFLAGS` replaces the configuration's flags, so the Makefile
recipes that set it compose the standard's flags onto any inherited value (CI's
`setup-rust` exports one). Two builds are deliberately excluded: coverage
assigns `RUSTFLAGS` without the fast flags, because a measurement should not
depend on them, and the release recipe and workflow keep the platform linker,
because they assign `RUSTFLAGS` (even an empty value displaces the
configuration). Cargo has no per-profile `rustflags`, so a direct
`cargo build --release` takes the configuration's flags unless `RUSTFLAGS` is
assigned too.

On Linux, install `mold` before building: the configuration names it, so a
build without it fails at link time. CI installs it through `setup-rust`'s
`install-mold` input. `tests/build_standard_contract.rs` holds the standard. It
reads the configuration sources, the commands `make -n` prints for each
development target on a Linux host and a macOS host (each keeping the caller's
own `RUSTFLAGS`) and for each coverage and release target on a Linux host, and
the `setup-rust` steps of the CI workflows (each must pass `install-mold`), so
a flag lost through a recipe or workflow edit fails there. The decision is
recorded in [ADR 001](adr-001-rust-build-standard.md).
`tests/workflow_contracts/test_release_flags_contract.py` holds the release
builds and the lint commands. It reads `release.yml` as YAML and requires each
of its five release builds to assign its own value: a hosted-runner build
assigns `RUSTFLAGS: -D warnings` in its own `env:` block, and a build inside a
BSD VM, whose packaged Rust is stable, assigns an empty `RUSTFLAGS` on the
command line, so the nightly-only flags in `.cargo/config.toml` never reach a
release. It also reads `make -n lint` and requires the Clippy and Whitaker
commands each to keep `-D warnings`.

### Development build footprint

The development profile sets `incremental = false` and `debug = 0` in
`Cargo.toml`, so a dev build keeps no incremental state or debug information.
To debug, set `CARGO_PROFILE_DEV_DEBUG=true`; to rebuild incrementally, set
`CARGO_INCREMENTAL=1` (unset any `sccache` wrapper first, which refuses
incremental builds). `.cargo/config.toml` also sets
`[unstable] embed-metadata = false`, the native Cargo key for nightlies from
2026-07-31, so Cargo coordinates metadata output itself. No raw rustc metadata
flags are used, and the pin is not moved to suit the key.
`tests/build_footprint_contract.rs` holds all three settings and proves them,
with both overrides, at the Cargo boundary by building a one-file crate under
the repository's own `[profile.dev]` table and Cargo configuration.

### Whitaker isolation

`make lint` runs Whitaker with the literal `RUSTFLAGS="-D warnings"`, after
clearing an inherited `CARGO_ENCODED_RUSTFLAGS`, which Cargo ranks above
`RUSTFLAGS`. Whitaker runs under the toolchain its installer manages, so it
takes none of the development standard (no parallel frontend, no `mold` linker
flag and no inherited `RUSTFLAGS`). The build-standard contract reads that
isolated command as exempt from the restatement clauses, and ADR 001 records
the exception. `tests/whitaker_isolation_contract.rs` reads the real
`make -n lint` output and fails unless Whitaker clears the encoded flags and
assigns the literal `RUSTFLAGS="-D warnings"`.

### Cranelift

Exception: Cranelift is not the development-profile backend. The repository pins
`nightly-2026-10-30` (no Cranelift measurement was taken on it, and the
exception has no deferral date: it holds while the release workflow builds on
stable), but the release workflow (`cross +stable build --release`, plus the
macOS, Windows, FreeBSD and OpenBSD legs, which build on stable) builds on a
stable toolchain against `.cargo/config.toml`, and stable Cargo refuses a
`[profile.dev] codegen-backend` key ("config profile `dev` is not valid"), so
selecting the backend would break that build (recorded 2026-09-29). That build
assigns `RUSTFLAGS`, so the nightly-only `-Zthreads` flag never reaches it.
Revisit if that build moves to the pinned nightly.
