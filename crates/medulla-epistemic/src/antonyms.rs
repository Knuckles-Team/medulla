//! Negation and antonym table that confirms an opposed claim pair
//! (MEDULLA-DECISION-ENGINE-R700.2).

use std::collections::BTreeSet;

/// Words that flip the polarity of a sentence.
const NEGATIONS: &[&str] = &[
    "no", "not", "never", "none", "neither", "nor", "cannot", "without",
];

/// Antonym pairs, each listed once in lowercase.
const ANTONYMS: &[(&str, &str)] = &[
    ("up", "down"),
    ("increase", "decrease"),
    ("increases", "decreases"),
    ("higher", "lower"),
    ("more", "less"),
    ("faster", "slower"),
    ("true", "false"),
    ("safe", "unsafe"),
    ("enabled", "disabled"),
    ("allowed", "denied"),
    ("success", "failure"),
    ("improves", "degrades"),
    ("positive", "negative"),
];

/// Whether `word` is a negation.
pub fn is_negation(word: &str) -> bool {
    NEGATIONS.contains(&word)
}

/// Whether the two words are listed antonyms.
pub fn are_antonyms(left: &str, right: &str) -> bool {
    ANTONYMS
        .iter()
        .any(|&(a, b)| (a == left && b == right) || (a == right && b == left))
}

/// Whether the two token sets confirm an opposition: their negation parity
/// differs, or one holds an antonym of a word only the other holds.
pub fn confirms_opposition(left: &BTreeSet<String>, right: &BTreeSet<String>) -> bool {
    let negations = |set: &BTreeSet<String>| set.iter().filter(|word| is_negation(word)).count();
    if negations(left) % 2 != negations(right) % 2 {
        return true;
    }
    left.difference(right)
        .any(|a| right.difference(left).any(|b| are_antonyms(a, b)))
}
