//! All matching passages, keeping distant matches and different field matches separate.

use std::collections::BTreeSet;

use super::{MATCH_END, MATCH_START};

pub(super) fn highlight(table: &str, columns: &[i32], start: &str, end: &str) -> String {
    columns
        .iter()
        .map(|column| format!("highlight({table}, {column}, {start}, {end})"))
        .collect::<Vec<_>>()
        .join(" || char(0) || ")
}

pub(super) fn collect(sources: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut found = BTreeSet::new();
    let mut snippets = Vec::new();
    for source in sources {
        for column in source.split('\0') {
            for snippet in passages(column) {
                if found.insert(snippet.clone()) {
                    snippets.push(snippet);
                }
            }
        }
    }
    snippets
}

fn passages(text: &str) -> Vec<String> {
    let words: Vec<_> = text.split_whitespace().collect();
    let mut windows: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for (index, word) in words.iter().enumerate() {
        if word.contains(MATCH_START) {
            start = Some(index);
        }
        if word.contains(MATCH_END)
            && let Some(start) = start.take()
        {
            let range = (start.saturating_sub(6), (index + 7).min(words.len()));
            if let Some(last) = windows.last_mut().filter(|last| last.1 >= range.0) {
                last.1 = last.1.max(range.1);
            } else {
                windows.push(range);
            }
        }
    }
    windows
        .into_iter()
        .map(|(start, end)| {
            let before = if start == 0 { "" } else { "…\u{a0}" };
            let after = if end == words.len() { "" } else { "\u{a0}…" };
            format!("{before}{}{after}", words[start..end].join(" "))
        })
        .collect()
}
