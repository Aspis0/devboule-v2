/// The collector's text-extension allow-list. Keep this separate from the
/// chunk builder so a city-only consumer can reuse the exact filter without
/// compiling AST chunking or any embedding code.
pub const COLLECTOR_MAX_FILE_BYTES: u64 = 1_200_000;

pub fn is_text_extension(ext: &str) -> bool {
    matches!(
        ext,
        ".adoc"
            | ".bib"
            | ".css"
            | ".gradle"
            | ".html"
            | ".java"
            | ".js"
            | ".jsx"
            | ".json"
            | ".jsonc"
            | ".kt"
            | ".kts"
            | ".md"
            | ".mjs"
            | ".cjs"
            | ".mts"
            | ".cts"
            | ".org"
            | ".properties"
            | ".ps1"
            | ".py"
            | ".r"
            | ".rmd"
            | ".rs"
            | ".rst"
            | ".sh"
            | ".sql"
            | ".tex"
            | ".toml"
            | ".ts"
            | ".tsx"
            | ".xml"
            | ".txt"
            | ".yaml"
            | ".yml"
    )
}
