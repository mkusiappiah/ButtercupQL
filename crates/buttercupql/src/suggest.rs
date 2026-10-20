//! Suggestions for names that were probably mistyped.

/// The word among `words` closest to `word`, when one is close enough to be a likely typing mistake. Case is
/// ignored, and a swap of two neighbouring letters counts as one mistake.
pub(crate) fn closest<'w>(word: &str, words: impl IntoIterator<Item = &'w str>) -> Option<&'w str> {
    let word = word.to_lowercase();
    let allowed = word.chars().count().max(3) / 3;
    words
        .into_iter()
        .map(|candidate| (distance(&word, &candidate.to_lowercase()), candidate))
        .filter(|&(edits, _)| edits <= allowed)
        .min_by_key(|&(edits, _)| edits)
        .map(|(_, candidate)| candidate)
}

/// How many letters must be added, removed, changed or swapped with a neighbour to make `a` into `b`.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    // Three rows of the table are enough: a swap looks two rows back.
    let mut before: Vec<usize> = vec![0; b.len() + 1];
    let mut last: Vec<usize> = (0..=b.len()).collect();
    let mut row = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        row[0] = i;
        for j in 1..=b.len() {
            let change = usize::from(a[i - 1] != b[j - 1]);
            let mut edits = (last[j] + 1).min(row[j - 1] + 1).min(last[j - 1] + change);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                edits = edits.min(before[j - 2] + 1);
            }
            row[j] = edits;
        }
        std::mem::swap(&mut before, &mut last);
        std::mem::swap(&mut last, &mut row);
    }
    last[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mistakes_are_counted() {
        assert_eq!(distance("where", "where"), 0);
        assert_eq!(distance("wehre", "where"), 1);
        assert_eq!(distance("slect", "select"), 1);
        assert_eq!(distance("stauts", "status"), 1);
        assert_eq!(distance("sumarise", "summarize"), 2);
        assert_eq!(distance("", "take"), 4);
    }

    #[test]
    fn only_a_close_word_is_suggested() {
        let steps = ["where", "select", "sort", "take", "summarize"];
        assert_eq!(closest("wehre", steps), Some("where"));
        assert_eq!(closest("WHERE", steps), Some("where"));
        assert_eq!(closest("tak", steps), Some("take"));
        assert_eq!(closest("sumarize", steps), Some("summarize"));
        assert_eq!(closest("banana", steps), None);
    }
}
