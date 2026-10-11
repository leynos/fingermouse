//! The judgement functions of the YAML lint contract.

const INSTALLER: &str = "leynos/shared-actions/.github/actions/install-yaml-lint@";

/// Returns the lines of `text` with comments removed and blanks dropped.
pub fn code_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(|line| line.split(" #").next().unwrap_or_default().trim())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// One line of a recipe or a workflow step, trimmed.
#[derive(Clone, Copy)]
pub struct Line<'a>(pub &'a str);

impl Line<'_> {
    /// The target a literal `$(MAKE) target` recipe line delegates to.
    fn delegated_target(self) -> Option<String> {
        self.0
            .strip_prefix("$(MAKE) ")
            .map(|next| next.trim().to_owned())
    }

    /// Whether the line lints the tracked YAML strictly, from Git's own file
    /// list, with its failure left to fail the target.
    fn is_sound_yamllint(self) -> bool {
        let line = self.0;
        let from_git = line.starts_with("git ls-files")
            && line.contains("| xargs")
            && !line.contains(".github");
        let every_pathspec = line.contains("'*.yml'") && line.contains("'*.yaml'");
        from_git
            && every_pathspec
            && line.contains("$(YAMLLINT)")
            && line.contains("--strict")
            && !line.contains("|| true")
    }

    /// Whether the line runs actionlint with its failure left to fail the target.
    fn is_sound_actionlint(self) -> bool {
        self.0.starts_with("$(ACTIONLINT)") && !self.0.contains("|| true")
    }

    /// Whether the line is a job key: indented by exactly two spaces.
    fn is_job_header(self) -> bool {
        self.0.starts_with("  ") && !self.0.starts_with("   ")
    }

    /// Whether the step line pins the shared installer to a full commit SHA.
    fn pins_the_installer(self) -> bool {
        self.0
            .trim_start_matches("- ")
            .strip_prefix("uses: ")
            .and_then(|uses| uses.strip_prefix(INSTALLER))
            .is_some_and(|reference| {
                reference.len() == 40 && reference.bytes().all(|b| b.is_ascii_hexdigit())
            })
    }

    /// Whether the step line runs the lint gate.
    fn runs_the_gate(self) -> bool {
        let run = self.0.trim_start_matches("- ").strip_prefix("run: ");
        run.is_some_and(|text| {
            let words: Vec<&str> = text.split_whitespace().collect();
            words
                .first()
                .is_some_and(|word| matches!(*word, "make" | "/usr/bin/make"))
                && words.contains(&"lint")
                && !text.contains("||")
        })
    }

    /// Whether the line mentions both `make` and the `lint` target.
    fn mentions_the_gate(self) -> bool {
        self.0.contains("make") && self.0.split_whitespace().any(|word| word == "lint")
    }

    /// Whether the line lets a failing step pass.
    fn ignores_failure(self) -> bool {
        self.0.contains("continue-on-error: true") || self.0.contains("|| true")
    }

    /// Whether the line provisions a lint tool by hand.
    fn provisions_by_hand(self) -> bool {
        let line = self.0;
        line.contains("uv tool install \"yamllint")
            || line.contains("download-actionlint")
            || line.contains("releases/download") && line.contains("actionlint")
    }
}

/// One Makefile rule: its targets' prerequisites and its recipe lines.
struct Rule {
    targets: Vec<String>,
    prerequisites: Vec<String>,
    recipe: Vec<String>,
}

/// A Makefile, read as text.
pub struct Makefile<'a>(pub &'a str);

impl Makefile<'_> {
    /// The rule a Makefile line opens, if it opens one.
    fn rule_header(line: &str) -> Option<Rule> {
        let assignment = [":=", "?="].iter().any(|operator| line.contains(operator));
        if line.starts_with('#') || assignment {
            return None;
        }
        let (targets, rest) = line.split_once(':')?;
        let prerequisites = rest.split("##").next().unwrap_or_default();
        Some(Rule {
            targets: targets.split_whitespace().map(str::to_owned).collect(),
            prerequisites: prerequisites
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            recipe: Vec::new(),
        })
    }

    /// The rules, joining nothing and expanding nothing.
    fn rules(&self) -> Vec<Rule> {
        let mut found: Vec<Rule> = Vec::new();
        for line in self.0.lines() {
            match (line.strip_prefix('\t'), found.last_mut()) {
                (Some(recipe), Some(rule)) => rule.recipe.push(recipe.trim().to_owned()),
                (Some(_), None) => {}
                (None, _) => found.extend(Self::rule_header(line)),
            }
        }
        found
    }

    /// Every recipe line reachable from `lint` through prerequisites and
    /// literal `$(MAKE) target` calls.
    fn lint_closure(&self) -> Vec<String> {
        let rules = self.rules();
        let mut pending = vec!["lint".to_owned()];
        let mut seen: Vec<String> = Vec::new();
        let mut lines = Vec::new();
        while let Some(target) = pending.pop() {
            if seen.contains(&target) {
                continue;
            }
            seen.push(target.clone());
            for rule in rules.iter().filter(|rule| rule.targets.contains(&target)) {
                pending.extend(rule.prerequisites.iter().cloned());
                pending.extend(
                    rule.recipe
                        .iter()
                        .filter_map(|line| Line(line).delegated_target()),
                );
                lines.extend(rule.recipe.iter().cloned());
            }
        }
        lines
    }

    /// Whether the closure runs a strict yamllint over `git ls-files` and
    /// actionlint.
    pub fn gate_is_wired(&self) -> bool {
        let lines = self.lint_closure();
        lines.iter().any(|line| Line(line).is_sound_yamllint())
            && lines.iter().any(|line| Line(line).is_sound_actionlint())
    }
}

/// A workflow, read as text.
pub struct Workflow<'a>(pub &'a str);

impl Workflow<'_> {
    /// The jobs, each as its code lines. A job starts at a two-space-indented
    /// key under `jobs:` and runs to the next such key.
    fn jobs(&self) -> Vec<Vec<&str>> {
        let mut found: Vec<Vec<&str>> = Vec::new();
        let mut in_jobs = false;
        for line in self.0.lines() {
            let code = line.split(" #").next().unwrap_or_default();
            if code.trim().is_empty() || code.trim_start().starts_with('#') {
                continue;
            }
            if !code.starts_with(' ') {
                in_jobs = code.trim_end() == "jobs:";
            } else if in_jobs && Line(code).is_job_header() {
                found.push(Vec::new());
            } else if let Some(job) = found.last_mut().filter(|_| in_jobs) {
                job.push(code.trim());
            }
        }
        found
    }

    /// Whether some job runs the lint gate.
    pub fn runs_the_gate(&self) -> bool {
        self.jobs()
            .iter()
            .any(|job| job.iter().any(|line| Line(line).runs_the_gate()))
    }

    /// Whether some job runs the gate and every such job installs the tools,
    /// pinned, in the same job and before it, with failure not ignored.
    pub fn installs_before_lint(&self) -> bool {
        let gated: Vec<Vec<&str>> = self
            .jobs()
            .into_iter()
            .filter(|job| job.iter().any(|line| Line(line).mentions_the_gate()))
            .collect();
        !gated.is_empty()
            && gated.iter().all(|job| {
                let install = job.iter().position(|line| Line(line).pins_the_installer());
                let lint = job.iter().position(|line| Line(line).runs_the_gate());
                let ignored = job.iter().any(|line| Line(line).ignores_failure());
                matches!((install, lint), (Some(first), Some(second)) if first < second) && !ignored
            })
    }

    /// Whether the workflow still provisions a lint tool by hand.
    pub fn provisions_by_hand(&self) -> bool {
        code_lines(self.0).any(|line| Line(line).provisions_by_hand())
    }
}
