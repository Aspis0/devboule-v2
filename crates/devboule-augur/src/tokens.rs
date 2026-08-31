//! Test tokens assembled at runtime from pieces.
//!
//! A contiguous `sk-…T3BlbkFJ…` literal in this crate was rejected by GitHub
//! push protection. Do not "clean these up" into a single string: the scanner
//! will fail the push again. The pieces are joined only in memory, then the
//! detector tests assert the real compiled gitleaks rule still matches.

pub fn openai_api_key() -> String {
    let prefix = "sk-";
    let head = "abcdefghijklmnopqrst";
    let infix = "T3BlbkFJ";
    let tail = "abcdefghijklmnopqrst";
    format!("{prefix}{head}{infix}{tail}")
}

pub fn aws_access_token() -> String {
    let prefix = "AKIA";
    let body = "BHCEFGHIJKLMNOPQ";
    format!("{prefix}{body}")
}

pub fn aws_example() -> String {
    let prefix = "AKIA";
    let body = "IOSFODNN7";
    let suffix = "EXAMPLE";
    format!("{prefix}{body}{suffix}")
}

pub fn aws_other() -> String {
    let prefix = "AKIA";
    let body = "ZZZZYYYYXXXXWWWW";
    format!("{prefix}{body}")
}

pub fn aws_changed() -> String {
    let prefix = "AKIA";
    let body = "IOSFODNN7CHANGED";
    format!("{prefix}{body}")
}

pub fn rsa_private_key_pem() -> String {
    let kind = "RSA";
    let label = format!("{kind} PRIVATE KEY");
    let body = "MIIEowIBAAKCAQEA0fake";
    format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----\n")
}
