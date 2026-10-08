# Coverage ownership

The trunk owns both persistent coverage outputs. On a push to `main`,
`.github/workflows/coverage-main.yml` measures coverage, writes the ratchet
baseline, and uploads the report to CodeScene. Pull-request CI measures the
same selection only to compare it with that baseline: it archives no report,
never calls CodeScene, and names `CS_ACCESS_TOKEN` nowhere. The call is what
moves to the trunk, not the archive: the uploader pins the `cs-coverage`
archive by digest, but the client refuses to run whenever CodeScene's API
changes shape, and on the trunk such a change no longer fails every pull
request.

## The publisher

The publisher never binds the token in an `env` block, because the uploader is
a composite action that would pass a step's environment on to its nested steps.
A check step writes whether the secret is set, the upload runs only when it is
and only for `refs/heads/main`, and the token reaches the uploader solely as its
`access-token` input. Runs share one concurrency group per ref and are never
cancelled, so triggered runs (a push or a dispatch) upload in commit order. A
manual re-run of an older run is an operator action: it republishes that
commit's coverage and baseline until the next push supersedes it.

## Known gaps

Three gaps are known and accepted.

- Merges made by the Dependabot automerge workflow with `GITHUB_TOKEN` fire no
  push, so they reach the publisher only through a dispatch or the next
  ordinary push.
- A dispatch that replaces a pending push uploads the same or a newer commit,
  but the shared action writes the baseline only on a push, so the baseline
  stays one push behind until the next one.
- The contract judges the workflows as committed. A same-repository pull
  request runs its own copy of `ci.yml`, and a dispatch runs the dispatched
  branch's copy of `coverage-main.yml`, so an edit on a branch could still read
  the repository secret. Only a deployment environment whose branches are
  restricted to `main` would confine the token by platform policy; that is a
  repository-settings decision outside the workflows.

## The contract

`make workflow-contracts`, which pull-request CI runs, holds this shape by
running `cv005-contracts check`, the shared contract library in
`leynos/shared-actions` (`packages/cv005-contracts`), from a full commit named
by `CV005_CONTRACTS_REF` in the Makefile. A fix to the rules is therefore a pin
bump. The target needs `uv`, which fetches the Python 3.13 the library runs
under. The repository's parameters are in `.github/cv005.toml`: its
`repository` name and the `[selection]` inputs the baseline measures, which the
publisher's generator must carry and every pull-request lane must match. The
library's own suite proves each rule refuses the shape it exists to refuse, so
this repository keeps no copy of the readers or the refusal cases. It reads
every workflow strictly (a repeated key is an error) and follows local
reusable-workflow calls transitively, so a called workflow cannot reach
CodeScene on a pull request's behalf.
