//! Times the Git queries behind the history/graph view:
//! `cargo run --release -p gall-core --example git_bench -- <repo>`
use gall_core::fs::{gitgraph, gitrepository};
use std::path::PathBuf;
use std::time::Instant;

fn time<T>(label: &str, f: impl Fn() -> T) {
    f();
    let n = 3;
    let t = Instant::now();
    for _ in 0..n {
        f();
    }
    println!(
        "{label:<36} {:>8.0} ms",
        t.elapsed().as_secs_f64() * 1000.0 / f64::from(n)
    );
}

fn main() {
    let repo = PathBuf::from(std::env::args().nth(1).expect("repo path"));
    time("query_history(200)", || {
        gitgraph::query_history(&repo, false, false, 200, None).unwrap()
    });
    time("snapshot", || gitrepository::snapshot(&repo).unwrap());
    // What the graph view needs: both results, as the UI used to request them (two commands
    // at the same time) and as one combined call.
    time("history + snapshot, two commands", || {
        std::thread::scope(|scope| {
            let history = scope.spawn(|| gitgraph::query_history(&repo, false, false, 200, None));
            let state = gitrepository::snapshot(&repo).unwrap();
            (history.join().unwrap().unwrap(), state)
        })
    });
    time("graph_view, one call", || {
        gitrepository::graph_view(&repo, false, false, 200, None).unwrap()
    });
}
