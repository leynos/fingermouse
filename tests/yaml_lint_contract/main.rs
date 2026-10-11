//! Keeps the YAML lint gate wired into `make lint` and CI.
//!
//! `make lint` must run `yamllint --strict` over the Git-tracked YAML and
//! actionlint, and CI must install both through the pinned shared installer
//! before it runs the gate. The judgement is driven against fixtures first,
//! because a rule exercised only over this repository's own compliant files
//! would pass whether or not it detects anything, and then applied to the real
//! files. Everything is read as text, so the contract needs no parser
//! dependency; the Concordat `yaml-lint-baseline` rule audits the same wiring
//! with a full Makefile parse, and the `boundary` file runs the Make targets.

mod boundary;
mod judgement;

use judgement::{Makefile, Workflow, code_lines};

const MAKEFILE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Makefile"));
const YAMLLINT_CONFIG: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/.yamllint.yml"));
const ESTATE_BASE: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/.yamllint-estate.yml"));
const ACTIONLINT_CONFIG: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/.github/actionlint.yaml"
));
const WORKFLOWS: [&str; 4] = [
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.github/workflows/ci.yml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.github/workflows/coverage-main.yml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.github/workflows/dependabot-automerge.yml"
    )),
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.github/workflows/release.yml"
    )),
];

const INSTALLER: &str = "leynos/shared-actions/.github/actions/install-yaml-lint@";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const GOOD_MAKEFILE: &str = "lint: yamllint actionlint\nyamllint:\n\tgit ls-files -z '*.yml' \
                             '*.yaml' | xargs -0 -r $(YAMLLINT) --strict \
                             --\nactionlint:\n\t$(ACTIONLINT)\n";

/// One fixture: a name for the failure message, the text, and the verdict.
type Case = (&'static str, String, bool);

/// Makefiles the judgement must accept.
fn gate_accepted() -> Vec<Case> {
    let delegated = format!("lint:\n\t$(MAKE) gate\ngate: yamllint actionlint\n{}", {
        GOOD_MAKEFILE.split_once('\n').map_or("", |(_, rest)| rest)
    });
    vec![
        ("wired", GOOD_MAKEFILE.to_owned(), true),
        ("delegated", delegated, true),
    ]
}

/// Makefiles the judgement must reject, each with one defect.
fn gate_rejected() -> Vec<Case> {
    let swap = |from: &str, to: &str| GOOD_MAKEFILE.replace(from, to);
    let gate = "git ls-files -z '*.yml' '*.yaml' | xargs -0 -r $(YAMLLINT) --strict --";
    let bypassed = format!(
        "lint:\n\t$(MAKE) gate\ngate: yamllint actionlint\n{}",
        swap("\t$(ACTIONLINT)", "\t-$(ACTIONLINT)")
            .split_once('\n')
            .map_or("", |(_, rest)| rest)
    );
    vec![
        ("not strict", swap(" --strict", ""), false),
        ("one pathspec", swap(" '*.yaml'", ""), false),
        ("whole tree", swap(gate, "$(YAMLLINT) --strict ."), false),
        (
            "workflows filter",
            swap("'*.yaml' |", "'*.yaml' .github/workflows |"),
            false,
        ),
        (
            "yamllint ignored",
            swap("--strict --", "--strict -- || true"),
            false,
        ),
        (
            "yamllint dash",
            swap("\tgit ls-files", "\t-git ls-files"),
            false,
        ),
        (
            "yamllint negated",
            swap("\tgit ls-files", "\t! git ls-files"),
            false,
        ),
        (
            "actionlint ignored",
            swap("\t$(ACTIONLINT)", "\t$(ACTIONLINT) || true"),
            false,
        ),
        (
            "actionlint dash",
            swap("\t$(ACTIONLINT)", "\t-$(ACTIONLINT)"),
            false,
        ),
        (
            "actionlint negated",
            swap("\t$(ACTIONLINT)", "\t! $(ACTIONLINT)"),
            false,
        ),
        (
            "outside lint",
            swap(
                "lint: yamllint actionlint",
                "lint:\n\ttrue\nother: yamllint actionlint",
            ),
            false,
        ),
        ("no actionlint", swap("\t$(ACTIONLINT)", "\ttrue"), false),
        ("bypassed through delegation", bypassed, false),
    ]
}

fn workflow(extra_job: &str, steps: &[String]) -> String {
    let body: String = steps
        .iter()
        .map(|step| ["      ", step.as_str(), "\n"].concat())
        .collect();
    ["jobs:\n", extra_job, "  gate:\n    steps:\n", body.as_str()].concat()
}

/// Workflows the judgement must accept.
fn ci_accepted() -> Vec<Case> {
    let pinned = format!("- uses: {INSTALLER}{SHA}");
    let run = |command: &str| format!("- run: {command}");
    vec![
        (
            "pinned",
            workflow("", &[pinned.clone(), run("make lint")]),
            true,
        ),
        (
            "options",
            workflow("", &[pinned.clone(), run("make lint MDLINT=true")]),
            true,
        ),
        (
            "absolute make",
            workflow("", &[pinned.clone(), run("/usr/bin/make lint")]),
            true,
        ),
    ]
}

/// Workflows the judgement must reject, each with one defect.
fn ci_rejected() -> Vec<Case> {
    let pinned = format!("- uses: {INSTALLER}{SHA}");
    let run = |command: &str| format!("- run: {command}");
    let unpinned = format!("- uses: {INSTALLER}v1");
    let other_job = format!("  tools:\n    steps:\n      {pinned}\n");
    vec![
        (
            "unpinned",
            workflow("", &[unpinned, run("make lint")]),
            false,
        ),
        ("missing", workflow("", &[run("make lint")]), false),
        (
            "installed after",
            workflow("", &[run("make lint"), pinned.clone()]),
            false,
        ),
        (
            "no lint",
            workflow("", &[pinned.clone(), run("make test")]),
            false,
        ),
        (
            "ignored failure",
            workflow("", &[pinned.clone(), run("make lint || true")]),
            false,
        ),
        (
            "continue on error",
            workflow(
                "",
                &[
                    pinned,
                    run("make lint"),
                    "  continue-on-error: true".to_owned(),
                ],
            ),
            false,
        ),
        (
            "installer in another job",
            workflow(&other_job, &[run("make lint")]),
            false,
        ),
    ]
}

#[test]
fn the_gate_judgement_detects_each_defect() {
    for (name, makefile, wired) in gate_accepted().into_iter().chain(gate_rejected()) {
        assert_eq!(
            Makefile(&makefile).gate_is_wired(),
            wired,
            "{name}: {makefile}"
        );
    }
}

#[test]
fn the_ci_judgement_detects_each_defect() {
    for (name, text, installs) in ci_accepted().into_iter().chain(ci_rejected()) {
        assert_eq!(
            Workflow(&text).installs_before_lint(),
            installs,
            "{name}: {text}"
        );
    }
}

#[test]
fn make_lint_runs_the_yaml_lint_gate() {
    assert!(
        Makefile(MAKEFILE).gate_is_wired(),
        "the lint closure lacks the gate"
    );
}

#[test]
fn ci_installs_the_tools_before_it_runs_the_gate() {
    let gated: Vec<&str> = WORKFLOWS
        .iter()
        .copied()
        .filter(|text| Workflow(text).runs_the_gate())
        .collect();
    assert!(!gated.is_empty(), "no workflow runs the lint gate");
    for text in gated {
        assert!(Workflow(text).installs_before_lint());
    }
    for text in WORKFLOWS {
        assert!(!Workflow(text).provisions_by_hand());
    }
}

#[test]
fn the_local_yamllint_configuration_extends_the_estate_base() {
    let keys: Vec<&str> = code_lines(YAMLLINT_CONFIG)
        .filter(|line| *line != "---")
        .collect();
    assert_eq!(keys.first(), Some(&"extends: .yamllint-estate.yml"));
}

#[test]
fn actionlint_accepts_the_estate_runner_labels() {
    assert!(code_lines(ACTIONLINT_CONFIG).any(|line| line == "- ubicloud-*"));
}

/// The estate base, as its code lines: the canonical configuration with
/// comments and blank lines dropped.
const CANONICAL_ESTATE_BASE: [&str; 28] = [
    "---",
    "extends: default",
    "ignore: |",
    "  **/charts/**/templates/",
    "rules:",
    "  document-start: disable",
    "  comments:",
    "    min-spaces-from-content: 1",
    "  indentation:",
    "    spaces: consistent",
    "    indent-sequences: consistent",
    "    check-multi-line-strings: false",
    "  braces:",
    "    min-spaces-inside: 0",
    "    max-spaces-inside: 1",
    "  brackets:",
    "    min-spaces-inside: 0",
    "    max-spaces-inside: 1",
    "  line-length:",
    "    max: 120",
    "    allow-non-breakable-words: true",
    "    allow-non-breakable-inline-mappings: true",
    "    ignore: |",
    "      .github/workflows/",
    "      .github/actions/",
    "  truthy:",
    "    allowed-values: ['true', 'false']",
    "    check-keys: false",
];

#[test]
fn the_estate_base_is_the_canonical_configuration() {
    // Every rule, heading and value, in order, with indentation: a dropped or
    // changed setting fails even when its text survives under a sibling rule.
    // The Concordat `yaml-lint-baseline` rule audits the same file by digest.
    let code: Vec<&str> = ESTATE_BASE
        .lines()
        .map(|line| line.split(" #").next().unwrap_or_default().trim_end())
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .collect();
    assert_eq!(code, CANONICAL_ESTATE_BASE);
}
