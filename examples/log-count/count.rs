use std::collections::BTreeMap;

// Aggregate event counts by the entire key before '='. Ignore malformed rows.
pub fn count(input: &str) -> BTreeMap<String, u64> {
    let mut totals: Vec<(String, u64)> = Vec::new();
    for line in input.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        let Ok(value) = value.parse::<u64>() else { continue };
        if let Some((_, total)) = totals.iter_mut().find(|(k, _)| k == key) {
            *total += value;
        } else {
            totals.push((key.to_owned(), value));
        }
    }
    totals.into_iter().collect()
}
