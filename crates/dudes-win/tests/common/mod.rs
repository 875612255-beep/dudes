#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

pub(crate) fn rust_sources() -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect(&src_dir(), &mut found);
    found.sort();
    found
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("src/ is where it was") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

pub(crate) fn code_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect()
}

pub(crate) fn found_in_code<'token>(text: &str, tokens: &[&'token str]) -> Vec<&'token str> {
    let code = code_lines(text);
    tokens
        .iter()
        .copied()
        .filter(|token| code.iter().any(|line| line.contains(token)))
        .collect()
}

pub(crate) fn count_in_code(text: &str, token: &str) -> usize {
    code_lines(text)
        .iter()
        .map(|line| line.matches(token).count())
        .sum()
}
