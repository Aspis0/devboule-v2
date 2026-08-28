# This is a plain configuration file with no function or class definitions.
# It contains only assignments, comments, and data structures.
# The semantic chunker will find no definitions here, so it falls back
# to the sliding window chunking strategy.

PIPELINE_CONFIG = {
    "name": "oracle-ingestion",
    "version": "2.5.0",
    "max_chunk_chars": 2500,
    "overlap_chars": 400,
    "embedding_model": "Qwen/Qwen3-Embedding-0.6B",
    "embedding_dims": 1024,
}

EXTENSION_MAP = {
    ".rs": "rust",
    ".py": "python",
    ".ts": "typescript",
    ".tsx": "typescript",
    ".js": "javascript",
    ".jsx": "javascript",
    ".java": "java",
    ".kt": "kotlin",
    ".sh": "bash",
}

CHUNK_PROFILES = {
    "code": {"max_chars": 2500, "overlap": 400},
    "doc": {"max_chars": 12000, "overlap": 1200},
    "structured": {"max_chars": 8000, "overlap": 900},
    "default": {"max_chars": 2200, "overlap": 280},
}

EXCLUDED_DIRS = {
    ".git", "node_modules", "target", "__pycache__",
    "venv", ".venv", "dist", "build",
}

SENSITIVE_PATTERNS = [
    ".env", ".env.*", "*.key", "*.pem",
    "secrets.yaml", "token.txt", "credentials",
]

DOMAIN_KEYWORDS = {
    "oracle": ["chunk", "embedding", "query", "context", "answer"],
    "compute": ["instance", "gpu", "cpu", "vm", "lifecycle", "cleanup"],
}

SOURCE_QUALITY_RULES = [
    "prefer_src_over_docs",
    "penalize_tests_unless_asked",
    "penalize_generated_artifacts",
]

EMBED_BATCH_SIZE = 32
CHUNK_BATCH_FILES = 16
CHUNK_BATCH_CHUNKS = 8
CHUNK_BATCH_CHARS = 50000
CHUNK_MIN_FREE_GB = 5.0
CHUNK_MAX_GPU_TEMP_C = 85
CHUNK_GPU_COOLDOWN_SECONDS = 45
CHUNK_LOW_MEMORY_RETRY_SECONDS = 5
CHUNK_LOW_MEMORY_RETRY_CYCLES = 6
LLM_MODEL = "voxtral-small-24b-2507"
LLM_TEMPERATURE = 0.1
ORACLE_PORT = 8765

PRIORITY_RANKS = {
    "src": 0,
    "src-tauri": 0,
    "tests": 1,
    "docs": 2,
    "default": 3,
}
