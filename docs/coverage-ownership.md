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

`make workflow-contracts`, which pull-request CI runs, holds this shape in
`tests/workflow_contracts/`. It reads every workflow strictly (a repeated key
is an error) and follows local reusable-workflow calls transitively, so a
called workflow cannot reach CodeScene on a pull request's behalf.
