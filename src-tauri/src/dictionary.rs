use std::collections::HashSet;

pub const MAX_TERMS: usize = 100;
pub const MAX_WORDS_PER_TERM: usize = 3;

const MAX_PROMPT_CHARS: usize = 1024;

#[derive(Debug, Clone)]
struct Replacement {
    start: usize,
    end: usize,
    value: String,
}

#[derive(Debug, Clone)]
struct WordSpan {
    start: usize,
    end: usize,
    word: String,
}

pub fn normalize_term(raw: &str) -> Option<String> {
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }

    if word_count(&normalized) > MAX_WORDS_PER_TERM {
        return None;
    }

    Some(normalized)
}

pub fn sanitize_terms(terms: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut cleaned = Vec::new();

    for raw in terms {
        let Some(normalized) = normalize_term(raw) else {
            continue;
        };

        let key = normalized.to_ascii_lowercase();
        if seen.contains(&key) {
            continue;
        }

        seen.insert(key);
        cleaned.push(normalized);

        if cleaned.len() >= MAX_TERMS {
            break;
        }
    }

    cleaned
}

pub fn build_prompt(terms: &[String]) -> Option<String> {
    let cleaned = sanitize_terms(terms);
    if cleaned.is_empty() {
        return None;
    }

    let mut prompt = String::from("Preferred spelling and terminology: ");

    for (index, term) in cleaned.iter().enumerate() {
        let separator = if index == 0 { "" } else { ", " };
        let addition_len = separator.len() + term.len();
        if prompt.len() + addition_len > MAX_PROMPT_CHARS {
            break;
        }

        prompt.push_str(separator);
        prompt.push_str(term);
    }

    if prompt.ends_with(": ") {
        None
    } else {
        Some(prompt)
    }
}

pub fn apply_dictionary_corrections(text: &str, terms: &[String]) -> String {
    if text.is_empty() {
        return String::new();
    }

    let cleaned = sanitize_terms(terms);
    if cleaned.is_empty() {
        return text.to_string();
    }

    let mut sorted_terms = cleaned.clone();
    sorted_terms.sort_by(|a, b| {
        word_count(b)
            .cmp(&word_count(a))
            .then_with(|| b.len().cmp(&a.len()))
    });

    let mut corrected = text.to_string();
    for term in &sorted_terms {
        corrected = replace_exact_occurrences(&corrected, term);
    }

    apply_fuzzy_single_word_replacements(&corrected, &sorted_terms)
}

fn replace_exact_occurrences(text: &str, canonical: &str) -> String {
    if canonical.is_empty() {
        return text.to_string();
    }

    let haystack = text.to_ascii_lowercase();
    let needle = canonical.to_ascii_lowercase();
    if needle.is_empty() {
        return text.to_string();
    }

    let bytes = haystack.as_bytes();
    let mut cursor = 0usize;
    let mut out = String::with_capacity(text.len());

    while cursor < haystack.len() {
        let Some(found) = haystack[cursor..].find(&needle) else {
            break;
        };

        let start = cursor + found;
        let end = start + needle.len();

        if !has_word_boundaries(bytes, start, end) {
            cursor = start + 1;
            continue;
        }

        out.push_str(&text[cursor..start]);
        let matched = &text[start..end];
        out.push_str(&apply_case_style(canonical, matched));
        cursor = end;
    }

    out.push_str(&text[cursor..]);
    out
}

fn apply_fuzzy_single_word_replacements(text: &str, terms: &[String]) -> String {
    let single_word_terms = terms
        .iter()
        .filter(|term| word_count(term) == 1)
        .map(|term| (term.to_ascii_lowercase(), term.as_str()))
        .collect::<Vec<_>>();

    if single_word_terms.is_empty() {
        return text.to_string();
    }

    let exact_terms = single_word_terms
        .iter()
        .map(|(lower, _)| lower.clone())
        .collect::<HashSet<_>>();

    let spans = collect_word_spans(text);
    if spans.is_empty() {
        return text.to_string();
    }

    let mut replacements = Vec::new();

    for span in spans {
        if !is_fuzzy_eligible_word(&span.word) {
            continue;
        }

        let source_lower = span.word.to_ascii_lowercase();
        if exact_terms.contains(&source_lower) {
            continue;
        }

        let mut best_distance = usize::MAX;
        let mut best_term: Option<&str> = None;
        let mut ambiguous = false;

        for (candidate_lower, candidate_term) in &single_word_terms {
            if !is_fuzzy_pair_candidate(&source_lower, candidate_lower) {
                continue;
            }

            let distance = levenshtein_ascii(&source_lower, candidate_lower);
            let max_distance = allowed_distance(source_lower.len(), candidate_lower.len());
            if distance == 0 || distance > max_distance {
                continue;
            }

            if distance < best_distance {
                best_distance = distance;
                best_term = Some(*candidate_term);
                ambiguous = false;
                continue;
            }

            if distance == best_distance {
                ambiguous = true;
            }
        }

        if ambiguous {
            continue;
        }

        if let Some(term) = best_term {
            replacements.push(Replacement {
                start: span.start,
                end: span.end,
                value: apply_case_style(term, &span.word),
            });
        }
    }

    if replacements.is_empty() {
        return text.to_string();
    }

    replacements.sort_by_key(|replacement| replacement.start);

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for replacement in replacements {
        if replacement.start < cursor {
            continue;
        }
        out.push_str(&text[cursor..replacement.start]);
        out.push_str(&replacement.value);
        cursor = replacement.end;
    }
    out.push_str(&text[cursor..]);
    out
}

fn collect_word_spans(text: &str) -> Vec<WordSpan> {
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let mut cursor = 0usize;

    while cursor < bytes.len() {
        if !is_word_byte(bytes[cursor]) {
            cursor += 1;
            continue;
        }

        let start = cursor;
        cursor += 1;
        while cursor < bytes.len() && is_word_byte(bytes[cursor]) {
            cursor += 1;
        }

        spans.push(WordSpan {
            start,
            end: cursor,
            word: text[start..cursor].to_string(),
        });
    }

    spans
}

fn apply_case_style(canonical: &str, matched: &str) -> String {
    if is_all_caps(matched) {
        return canonical.to_ascii_uppercase();
    }

    if is_title_case_phrase(matched) {
        return canonical
            .split_whitespace()
            .map(title_case_token)
            .collect::<Vec<_>>()
            .join(" ");
    }

    canonical.to_string()
}

fn is_all_caps(value: &str) -> bool {
    let mut has_alpha = false;
    for ch in value.chars() {
        if ch.is_ascii_alphabetic() {
            has_alpha = true;
            if !ch.is_ascii_uppercase() {
                return false;
            }
        }
    }
    has_alpha
}

fn is_title_case_phrase(value: &str) -> bool {
    let words = value.split_whitespace().collect::<Vec<_>>();
    if words.is_empty() {
        return false;
    }

    words.into_iter().all(is_title_case_token)
}

fn is_title_case_token(token: &str) -> bool {
    let mut chars = token.chars().filter(|ch| ch.is_ascii_alphabetic());
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.all(|ch| ch.is_ascii_lowercase())
}

fn title_case_token(token: &str) -> String {
    let mut out = String::with_capacity(token.len());
    let mut seen_alpha = false;

    for ch in token.chars() {
        if ch.is_ascii_alphabetic() {
            if !seen_alpha {
                out.push(ch.to_ascii_uppercase());
                seen_alpha = true;
            } else {
                out.push(ch.to_ascii_lowercase());
            }
        } else {
            out.push(ch);
        }
    }

    out
}

fn has_word_boundaries(bytes: &[u8], start: usize, end: usize) -> bool {
    let before_ok = if start == 0 {
        true
    } else {
        !is_word_byte(bytes[start - 1])
    };
    let after_ok = if end >= bytes.len() {
        true
    } else {
        !is_word_byte(bytes[end])
    };
    before_ok && after_ok
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'\''
}

fn word_count(value: &str) -> usize {
    value.split_whitespace().count()
}

fn is_fuzzy_eligible_word(word: &str) -> bool {
    word.len() >= 3 && word.chars().all(|ch| ch.is_ascii_alphabetic())
}

fn is_fuzzy_pair_candidate(source_lower: &str, candidate_lower: &str) -> bool {
    let source_bytes = source_lower.as_bytes();
    let candidate_bytes = candidate_lower.as_bytes();

    if source_bytes.is_empty() || candidate_bytes.is_empty() {
        return false;
    }
    if source_bytes[0] != candidate_bytes[0] {
        return false;
    }

    let len_diff = source_lower.len().abs_diff(candidate_lower.len());
    len_diff <= 1
}

fn allowed_distance(source_len: usize, candidate_len: usize) -> usize {
    let max_len = source_len.max(candidate_len);
    if max_len <= 5 {
        1
    } else {
        2
    }
}

fn levenshtein_ascii(left: &str, right: &str) -> usize {
    if left == right {
        return 0;
    }
    if left.is_empty() {
        return right.len();
    }
    if right.is_empty() {
        return left.len();
    }

    let left_bytes = left.as_bytes();
    let right_bytes = right.as_bytes();

    let mut previous = (0..=right_bytes.len()).collect::<Vec<_>>();
    let mut current = vec![0usize; right_bytes.len() + 1];

    for (i, left_byte) in left_bytes.iter().enumerate() {
        current[0] = i + 1;
        for (j, right_byte) in right_bytes.iter().enumerate() {
            let cost = usize::from(left_byte != right_byte);
            let deletion = previous[j + 1] + 1;
            let insertion = current[j] + 1;
            let substitution = previous[j] + cost;
            current[j + 1] = deletion.min(insertion).min(substitution);
        }
        previous.clone_from_slice(&current);
    }

    previous[right_bytes.len()]
}

#[cfg(test)]
mod tests {
    use super::{
        apply_dictionary_corrections, build_prompt, normalize_term, sanitize_terms, MAX_TERMS,
        MAX_WORDS_PER_TERM,
    };

    #[test]
    fn normalize_term_collapses_whitespace() {
        assert_eq!(
            normalize_term("   next   js   "),
            Some("next js".to_string())
        );
    }

    #[test]
    fn normalize_term_rejects_over_word_limit() {
        let value = (0..(MAX_WORDS_PER_TERM + 1))
            .map(|_| "a")
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(normalize_term(&value), None);
    }

    #[test]
    fn sanitize_terms_dedupes_and_caps_size() {
        let mut input = vec!["Postgres".to_string(), "postgres".to_string()];
        input.extend((0..MAX_TERMS).map(|idx| format!("term{idx}")));

        let cleaned = sanitize_terms(&input);
        assert_eq!(cleaned.first(), Some(&"Postgres".to_string()));
        assert_eq!(cleaned.len(), MAX_TERMS);
    }

    #[test]
    fn build_prompt_includes_terms() {
        let prompt = build_prompt(&["Postgres".to_string(), "Kubernetes".to_string()])
            .expect("prompt should exist");
        assert!(prompt.contains("Postgres"));
        assert!(prompt.contains("Kubernetes"));
    }

    #[test]
    fn exact_phrase_replacement_preserves_case_style() {
        let corrected = apply_dictionary_corrections(
            "we deploy with next js app",
            &["Next JS App".to_string()],
        );
        assert_eq!(corrected, "we deploy with Next JS App");
    }

    #[test]
    fn exact_word_replacement_handles_all_caps() {
        let corrected = apply_dictionary_corrections("POSTGRES is up", &["Postgres".to_string()]);
        assert_eq!(corrected, "POSTGRES is up");
    }

    #[test]
    fn fuzzy_word_replacement_replaces_close_match() {
        let corrected =
            apply_dictionary_corrections("Kubernets rollout done", &["Kubernetes".to_string()]);
        assert_eq!(corrected, "Kubernetes rollout done");
    }

    #[test]
    fn fuzzy_word_replacement_skips_ambiguous_matches() {
        let corrected =
            apply_dictionary_corrections("cart", &["cast".to_string(), "card".to_string()]);
        assert_eq!(corrected, "cart");
    }
}
