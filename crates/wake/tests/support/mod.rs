//! Shared helpers for the integration tests: golden files and a tiny deterministic generator.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

pub fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(name)
}

pub fn read(name: &str) -> Vec<u8> {
    std::fs::read(golden(name)).unwrap_or_else(|e| panic!("cannot read golden file {name}: {e}"))
}

pub fn model(name: &str) -> Vec<u8> {
    read(&format!("models/{name}.tflite"))
}

pub fn pcm(name: &str) -> Vec<i16> {
    read(name)
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

pub fn u16s(name: &str) -> Vec<u16> {
    read(name)
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect()
}

/// Lines of a text file, each split into numbers.
pub fn numbers(name: &str) -> Vec<Vec<f64>> {
    String::from_utf8(read(name))
        .expect("golden text is UTF-8")
        .lines()
        .map(|l| {
            l.split_whitespace()
                .map(|v| v.parse().expect("a number"))
                .collect()
        })
        .collect()
}

/// xorshift64*: enough randomness for mutation tests, reproducible everywhere.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}
