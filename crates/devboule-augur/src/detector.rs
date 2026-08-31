//! Detectors as a registry, not a pipeline.
//!
//! The trait does not assume in-process or cheap. Each detector says what it
//! costs so a caller can run the cheap ones now and the tools when asked.
//! Nothing here starts a timer.

use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::finding::Finding;

/// What running this detector costs, in order of magnitude.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cost {
    /// In-process, deterministic. Fine to run whenever asked.
    Cheap,
    /// A subprocess. The tool is the expert; we read its output.
    Tool,
    /// Slow or paid (an AI reviewer). Declared so a caller can skip it.
    /// No detector of this cost ships yet.
    Expensive,
}

/// Where to look, and which files. The caller asked; we do not invent a walk
/// of the whole disk if they named the files.
pub struct Context<'a> {
    pub root: &'a Path,
    pub files: &'a [PathBuf],
}

impl<'a> Context<'a> {
    pub fn new(root: &'a Path, files: &'a [PathBuf]) -> Self {
        Self { root, files }
    }
}

pub trait Detector: Send + Sync {
    fn id(&self) -> &'static str;
    fn cost(&self) -> Cost;
    fn scan(&self, ctx: &Context<'_>) -> Result<Vec<Finding>, Error>;
}

/// Detectors the caller chose. Adding a kind of review is constructing one
/// more and calling [`Registry::register`].
pub struct Registry {
    detectors: Vec<Box<dyn Detector>>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            detectors: Vec::new(),
        }
    }

    pub fn register(&mut self, detector: Box<dyn Detector>) -> &mut Self {
        self.detectors.push(detector);
        self
    }

    pub fn builtin() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(crate::detectors::secrets::Secrets::default()));
        registry.register(Box::new(crate::detectors::clippy::Clippy::default()));
        registry
    }

    /// Run every detector at or under `budget`. Failures from one detector
    /// do not silence the others: a clippy that cannot start should not hide
    /// a secret that is already on disk.
    pub fn review(&self, ctx: &Context<'_>, budget: Cost) -> Vec<Finding> {
        let mut findings = Vec::new();
        for detector in &self.detectors {
            if detector.cost() > budget {
                continue;
            }
            match detector.scan(ctx) {
                Ok(mut produced) => findings.append(&mut produced),
                Err(_) => continue,
            }
        }
        findings
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detectors::clippy::Clippy;
    use crate::detectors::secrets::Secrets;
    use crate::error::Error;

    #[test]
    fn a_cheap_review_does_not_run_a_tool_detector() {
        fn boom(_root: &Path) -> Result<String, Error> {
            panic!("clippy ran on a cheap review");
        }
        let mut registry = Registry::new();
        registry.register(Box::new(Secrets::default()));
        registry.register(Box::new(Clippy::from_output(boom)));
        let files: [PathBuf; 0] = [];
        let ctx = Context::new(Path::new("."), &files);
        let _ = registry.review(&ctx, Cost::Cheap);
    }

    #[test]
    fn two_shapes_of_detector_both_show_up() {
        const FIXTURE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/clippy-unused-variable.jsonl"
        ));
        fn fixture(_root: &Path) -> Result<String, Error> {
            Ok(FIXTURE.to_string())
        }
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("src")).expect("mkdir");
        std::fs::write(
            temp.path().join("src/lib.rs"),
            format!(
                "pub fn unused() {{ let x = 1; }}\nconst KEY: &str = \"{}\";\n",
                crate::tokens::aws_access_token()
            ),
        )
        .expect("write");
        let files = [PathBuf::from("src/lib.rs")];
        let mut registry = Registry::new();
        registry.register(Box::new(Secrets::default()));
        registry.register(Box::new(Clippy::from_output(fixture)));
        let findings = registry.review(&Context::new(temp.path(), &files), Cost::Tool);
        assert!(
            findings.iter().any(|finding| finding.source() == "secrets"),
            "in-process detector missing: {findings:?}"
        );
        assert!(
            findings.iter().any(|finding| finding.source() == "clippy"),
            "tool detector missing: {findings:?}"
        );
    }
}
