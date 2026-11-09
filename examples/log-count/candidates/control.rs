use std::collections::BTreeMap;

// Deliberately incorrect negative control: overwrites duplicate keys.
pub fn count(input: &str) -> BTreeMap<String, u64> {
    input.lines().filter_map(|line| {
        let (key, value) = line.split_once('=')?;
        Some((key.to_owned(), value.parse().ok()?))
    }).collect()
}
