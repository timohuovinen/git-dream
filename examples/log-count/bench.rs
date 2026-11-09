mod count;
use std::hint::black_box;
use std::time::Instant;

fn main() {
    let mut input = String::new();
    for i in 0..40000 { input.push_str(&format!("event-{}={}\n", (i * 7919) % 2048, i % 43)); }
    black_box(count::count(black_box(&input)));
    let mut samples = vec![];
    for _ in 0..7 {
        let start = Instant::now();
        let result = count::count(black_box(&input));
        assert_eq!(result.len(), 2048);
        black_box(result);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!("{{\"score\":{},\"samples_ms\":{:?},\"rows\":40000,\"keys\":2048}}", samples[3], samples);
}
