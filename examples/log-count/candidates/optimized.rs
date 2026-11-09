use std::collections::{BTreeMap, HashMap};

// Borrow keys while aggregating; allocate each distinct key once for the result.
pub fn count(input: &str) -> BTreeMap<String, u64> {
    let mut totals: HashMap<&str, u64> = HashMap::new();
    for line in input.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        let Ok(value) = value.parse::<u64>() else { continue };
        *totals.entry(key).or_default() += value;
    }
    totals.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}
