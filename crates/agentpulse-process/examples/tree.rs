//! Usage: tree <pid>. Prints the process tree rooted at <pid> and sampling cost.

use std::time::{Duration, Instant};

use agentpulse_process::{Sampler, attribute};

fn main() {
    let pid: u32 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .expect("usage: tree <pid>");
    let mut sampler = Sampler::new();
    sampler.sample();
    std::thread::sleep(Duration::from_secs(1));
    let table = sampler.sample();
    let root = table.procs.get(&pid).expect("pid not found");
    let usage = attribute(
        &table,
        &[agentpulse_core::ProcessIdentity {
            pid,
            start_time_s: root.start_time_s,
        }],
    );
    let u = usage[&pid];
    println!(
        "pid {pid}: {} processes, cpu {:?} %, rss {:.1} MB",
        u.process_count,
        u.cpu_percent,
        u.rss_bytes as f64 / 1_000_000.0
    );
    let start = Instant::now();
    for _ in 0..5 {
        sampler.sample();
    }
    println!(
        "sample(): {:.2} ms avg, {} processes in table",
        start.elapsed().as_secs_f64() * 1000.0 / 5.0,
        table.procs.len()
    );
}
