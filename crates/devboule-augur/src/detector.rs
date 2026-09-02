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
        registry.register(Box::new(crate::detectors::untested::Untested));
        registry.register(Box::new(crate::detectors::clippy::Clippy::default()));
        registry
    }

    /// Run every detector at or under `budget`. Failures from one detector
    /// do not silence the others: a clippy that cannot start should not hide
    /// a secret that is already on disk. The failure is reported, not swallowed.
    pub fn review(&self, ctx: &Context<'_>, budget: Cost) -> Review {
        let registered: Vec<&'static str> = self.detectors.iter().map(|d| d.id()).collect();
        let mut review = Review {
            findings: Vec::new(),
            completed: Vec::new(),
            failed: Vec::new(),
            registered,
        };
        for detector in &self.detectors {
            if detector.cost() > budget {
                continue;
            }
            match detector.scan(ctx) {
                Ok(mut produced) => {
                    for finding in &mut produced {
                        finding.stamp_source(detector.id());
                    }
                    review.findings.append(&mut produced);
                    review.completed.push(detector.id());
                }
                Err(error) => review.failed.push(FailedDetector {
                    detector: detector.id(),
                    message: error.to_string(),
                }),
            }
        }
        review.findings = crate::finding::coalesce(review.findings);
        review
    }
}

/// Outcome of one [`Registry::review`]. `completed` is what actually ran
/// (including empty-clean detectors) so the ledger can replace only those.
/// `registered` is every detector this registry knows, so a removed
/// detector's rows can be dropped on a later scan.
#[derive(Debug)]
pub struct Review {
    pub findings: Vec<Finding>,
    pub completed: Vec<&'static str>,
    pub failed: Vec<FailedDetector>,
    pub registered: Vec<&'static str>,
}

#[derive(Debug)]
pub struct FailedDetector {
    pub detector: &'static str,
    pub message: String,
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
        let review = registry.review(&Context::new(temp.path(), &files), Cost::Tool);
        assert!(
            review
                .findings
                .iter()
                .any(|finding| finding.source() == "secrets"),
            "in-process detector missing: {:?}",
            review.findings
        );
        assert!(
            review
                .findings
                .iter()
                .any(|finding| finding.source() == "clippy"),
            "tool detector missing: {:?}",
            review.findings
        );
        assert!(review.failed.is_empty());
        assert!(review.completed.contains(&"secrets"));
        assert!(review.completed.contains(&"clippy"));
    }

    #[test]
    fn a_broken_tool_is_reported_not_swallowed() {
        fn boom(_root: &Path) -> Result<String, Error> {
            Err(Error::tool("clippy failed: rustc exploded"))
        }
        let mut registry = Registry::new();
        registry.register(Box::new(Secrets::default()));
        registry.register(Box::new(Clippy::from_output(boom)));
        let files: [PathBuf; 0] = [];
        let review = registry.review(&Context::new(Path::new("."), &files), Cost::Tool);
        assert!(
            review
                .failed
                .iter()
                .any(|failed| failed.detector == "clippy"),
            "clippy failure was swallowed: {:?}",
            review.failed
        );
        assert!(review.completed.contains(&"secrets"));
        assert!(!review.completed.contains(&"clippy"));
    }

    #[test]
    fn builtin_cheap_review_runs_the_third_detector_and_skips_clippy() {
        let files: [PathBuf; 0] = [];
        let review = Registry::builtin().review(&Context::new(Path::new("."), &files), Cost::Cheap);
        assert!(
            review.completed.contains(&"untested"),
            "one register() line must put untested on the cheap path: completed={:?} registered={:?}",
            review.completed,
            review.registered
        );
        assert!(review.completed.contains(&"secrets"));
        assert!(
            !review.completed.contains(&"clippy"),
            "clippy is Cost::Tool and must stay off the request path: {:?}",
            review.completed
        );
        assert!(
            review
                .failed
                .iter()
                .all(|failed| failed.detector != "clippy"),
            "clippy must not even start: {:?}",
            review.failed
        );
        assert!(review.registered.contains(&"untested"));
        assert!(review.registered.contains(&"clippy"));
    }
}
