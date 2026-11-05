//! Benchmark for ZIP optimization: 32 stored text entries (~6.4 MB) are
//! re-optimized 15 times in-process and the median wall time is printed.
//!
//!     cargo run --release --example zip_bench

use std::io::{Cursor, Write};
use std::time::Instant;

fn text(seed: u32) -> String {
    let words = [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
        "lambda", "mu", "nu", "xi", "omicron", "pi",
    ];
    let mut x = 0x9E37_79B9u32 ^ seed.wrapping_mul(0x85EB_CA6B);
    let mut t = String::new();
    while t.len() < 200_000 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        t.push_str(words[(x % 16) as usize]);
        t.push(if x.is_multiple_of(11) { '\n' } else { ' ' });
        if x.is_multiple_of(7) {
            t.push_str(&(x % 1000).to_string());
        }
    }
    t
}

fn main() {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for i in 0..32 {
        w.start_file(format!("file{i:02}.txt"), o).unwrap();
        w.write_all(text(i).as_bytes()).unwrap();
    }
    let src = w.finish().unwrap().into_inner();
    let dir = std::env::temp_dir().join("ect-zipbench");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bench.zip");
    let cfg = ect::Config {
        quiet: true,
        workers: 1,
        ..ect::Config::default()
    };
    let mut times = Vec::new();
    for _ in 0..15 {
        std::fs::write(&path, &src).unwrap();
        let t = Instant::now();
        let r = ect::process(cfg.clone(), &[&path]);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(!r.has_errors());
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "median {:.2} ms  min {:.2} ms  in {} -> out {} bytes",
        times[7],
        times[0],
        src.len(),
        std::fs::metadata(&path).unwrap().len()
    );
}
