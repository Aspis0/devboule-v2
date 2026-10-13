//! The embedded design-craft corpus: the frontend's own files, not a copy.
//!
//! One responsibility: name the exact files the daemon bakes in at build
//! time. Each `include_str!` reads `src/features/design/craft/` itself, so
//! one edit reaches the app and the daemon together; a removed file fails
//! the build here, and the corpus tests fail on an added file or a changed
//! byte instead of letting the two sides diverge.

pub(crate) const MANIFEST_JSON: &str =
    include_str!("../../../src/features/design/craft/manifest.json");

pub(crate) struct EmbeddedSource {
    pub(crate) path: &'static str,
    pub(crate) text: &'static str,
}

pub(crate) const EMBEDDED_SOURCES: &[EmbeddedSource] = &[
    EmbeddedSource {
        path: "craft/accessibility.md",
        text: include_str!("../../../src/features/design/craft/accessibility.md"),
    },
    EmbeddedSource {
        path: "craft/anti-ai-slop.md",
        text: include_str!("../../../src/features/design/craft/anti-ai-slop.md"),
    },
    EmbeddedSource {
        path: "craft/cognition.md",
        text: include_str!("../../../src/features/design/craft/cognition.md"),
    },
    EmbeddedSource {
        path: "craft/color.md",
        text: include_str!("../../../src/features/design/craft/color.md"),
    },
    EmbeddedSource {
        path: "craft/form-validation.md",
        text: include_str!("../../../src/features/design/craft/form-validation.md"),
    },
    EmbeddedSource {
        path: "craft/icons.md",
        text: include_str!("../../../src/features/design/craft/icons.md"),
    },
    EmbeddedSource {
        path: "craft/layout.md",
        text: include_str!("../../../src/features/design/craft/layout.md"),
    },
    EmbeddedSource {
        path: "craft/microcopy.md",
        text: include_str!("../../../src/features/design/craft/microcopy.md"),
    },
    EmbeddedSource {
        path: "craft/motion.md",
        text: include_str!("../../../src/features/design/craft/motion.md"),
    },
    EmbeddedSource {
        path: "craft/reference-research.md",
        text: include_str!("../../../src/features/design/craft/reference-research.md"),
    },
    EmbeddedSource {
        path: "craft/rtl.md",
        text: include_str!("../../../src/features/design/craft/rtl.md"),
    },
    EmbeddedSource {
        path: "craft/slides.md",
        text: include_str!("../../../src/features/design/craft/slides.md"),
    },
    EmbeddedSource {
        path: "craft/spacing.md",
        text: include_str!("../../../src/features/design/craft/spacing.md"),
    },
    EmbeddedSource {
        path: "craft/state-coverage.md",
        text: include_str!("../../../src/features/design/craft/state-coverage.md"),
    },
    EmbeddedSource {
        path: "craft/typography.md",
        text: include_str!("../../../src/features/design/craft/typography.md"),
    },
];
