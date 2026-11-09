mod count;
use std::collections::BTreeMap;

#[test]
fn duplicates_unicode_and_malformed_rows() {
    let expected = BTreeMap::from([("α β".into(), 9), ("".into(), 7), ("a".into(), 3)]);
    assert_eq!(count::count("α β=4\nα β=5\n=7\na=3\r\nmissing\na=bad\na=1=2\n"), expected);
    assert!(count::count("").is_empty());
}

#[test]
fn generated_inputs_match_independent_oracle() {
    let mut input = String::new();
    let mut expected = BTreeMap::new();
    for i in 0..10000_u64 {
        let key = format!("event {}", (i * 7919) % 997);
        let value = (i * 17) % 43;
        input.push_str(&format!("{key}={value}\n"));
        *expected.entry(key).or_insert(0) += value;
    }
    assert_eq!(count::count(&input), expected);
}
