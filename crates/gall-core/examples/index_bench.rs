//! Indexes a folder and times a few queries:
//! `cargo run --release -p gall-core --example index_bench -- [root] [query...]`
use gall_core::fs::grep::{grep, GrepOptions};
use gall_core::fs::index::{FileIndex, IndexOptions};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = args
        .next()
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .expect("root");
    let queries: Vec<String> = args.collect();
    let queries = if queries.is_empty() {
        vec![
            "readme".into(),
            "fpane".into(),
            "ext:pdf".into(),
            "src/ main".into(),
        ]
    } else {
        queries
    };

    let idx = FileIndex::new();
    idx.set_preferred(&dirs::home_dir().into_iter().collect::<Vec<_>>());
    idx.rebuild(
        std::slice::from_ref(&root),
        &IndexOptions::default(),
        &AtomicBool::new(false),
    );
    let st = idx.status();
    println!("indexed {} entries in {} ms", st.entries, st.build_ms);
    for q in &queries {
        let r = idx.query(q, None, 50);
        println!(
            "{q:>12}: {:>7} matches in {:>3} ms  top: {}",
            r.total,
            r.elapsed_ms,
            r.hits.first().map(|h| h.path.as_str()).unwrap_or("-")
        );
    }
    let t = std::time::Instant::now();
    let st = grep(
        &root,
        "TODO",
        &GrepOptions::default(),
        &AtomicBool::new(false),
        &|_| {},
    )
    .unwrap();
    println!(
        "grep TODO: {} matches in {} files, {} searched, {} ms (wall {} ms)",
        st.matches,
        st.files,
        st.searched,
        st.elapsed_ms,
        t.elapsed().as_millis()
    );
}
