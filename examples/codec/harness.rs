//! Runs one function of codec.rs on stdin and writes the result to stdout.
//! `harness compress < file > blob`, `harness decompress < blob > file`.
mod codec;
use std::io::{Read, Write};

fn main() {
    let mut input = Vec::new();
    std::io::stdin().lock().read_to_end(&mut input).unwrap();
    let output = match std::env::args().nth(1).as_deref() {
        Some("compress") => codec::compress(&input),
        Some("decompress") => codec::decompress(&input),
        _ => panic!("usage: harness compress|decompress"),
    };
    std::io::stdout().lock().write_all(&output).unwrap();
}
