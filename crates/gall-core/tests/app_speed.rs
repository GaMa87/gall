//! Speed test of what the app does outside Git: listing folders (a folder of 46 500
//! entries, a profile folder like `AppData/Local`), finding names and content in a tree
//! of 60 000 files, the file index behind Quick Open, comparing files and folders, and
//! colouring syntax.
//!
//! Every row is the median of five runs, checked against its limit for the platform in
//! `tests/app_speed_limits.tsv`: 150 % of the slowest run recorded there, kept as a
//! multiple of a reference (reading and stat-ing a folder of 40 000 files on one thread
//! with the standard library, measured right before the row), so a busy or slower machine
//! moves the limit with it. A platform without recorded limits is measured and reported
//! only.
//!
//! Not run by default; part of the release checks:
//!
//! ```text
//! cargo test -p gall-core --release --test app_speed -- --ignored --nocapture
//! ```
//!
//! Environment: `GALL_SPEED_RECORD=1` raises the limits of this platform to 150 % of the
//! slowest run of this pass and writes the file (run it about three times on a new
//! platform); `GALL_SPEED_BIG`, `GALL_SPEED_TREE` set the size of the fixture;
//! `GALL_SPEED_DIR` where it is built; `GALL_SPEED_REPORT` a file the table is appended to.
//! `GALL_SPEED_ONLY` selects comma-separated parts of row names for focused measurements;
//! omit it for the complete speed check. Use it when recording a new row so existing
//! limits stay unchanged.
use gall_core::fs::{
    archive, archive_view, diff, dircmp, dirsize, grep, index, listing, search, syntax,
};
use gall_core::storage::Db;
use gall_core::view::sort;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const RUNS: usize = 5;

fn setting(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

/// A source-like text of `lines` lines; `seed` varies it, and files where `needle` is
/// set carry a word to search for.
fn source(seed: usize, lines: usize, needle: bool) -> String {
    let mut text = String::with_capacity(lines * 40);
    for line in 0..lines {
        let n = seed.wrapping_mul(31).wrapping_add(line * 7);
        match line % 6 {
            0 => writeln!(text, "fn handler_{}(input: &str) -> usize {{", n % 97).unwrap(),
            1 => writeln!(
                text,
                "    let value_{} = input.len() + {};",
                n % 13,
                n % 1000
            )
            .unwrap(),
            2 => writeln!(text, "    // TODO: revisit case {} of the parser", n % 50).unwrap(),
            3 => writeln!(
                text,
                "    if value_{} > {} {{ return {}; }}",
                n % 13,
                n % 77,
                n % 9
            )
            .unwrap(),
            4 => writeln!(text, "    value_{} * {}", n % 13, 1 + n % 5).unwrap(),
            _ => text.push_str("}\n"),
        }
    }
    if needle {
        text.push_str("const NEEDLE_TOKEN: &str = \"needle_token\";\n");
    }
    text
}

struct Fixture {
    big: PathBuf,
    appdata: PathBuf,
    tree: PathBuf,
    left: PathBuf,
    right: PathBuf,
    file_a: PathBuf,
    file_b: PathBuf,
    /// A settings database with 100 000 colour tags in 1 000 folders.
    db: PathBuf,
    /// A zip archive of 50 000 small files in 500 folders.
    zip: PathBuf,
    /// A compressed tar archive of 20 000 source files in 200 folders.
    tar_gz: PathBuf,
    /// A 7z archive of 5 000 source files in ten solid blocks.
    seven_z: PathBuf,
    /// A zip archive of the 5 000 source files of `left`, made by `archive::compress`.
    small_zip: PathBuf,
    /// Where the compress and extract rows write (emptied after every run).
    scratch: PathBuf,
}

fn build(base: &Path) -> Fixture {
    let started = Instant::now();
    let big_files = setting("GALL_SPEED_BIG", 40_000);
    let tree_files = setting("GALL_SPEED_TREE", 60_000);
    let exts = ["rs", "txt", "png", "pdf", "json", "md", "log", "dll"];
    // One folder with very many entries: files, folders, dot names, links.
    let big = base.join("big");
    std::fs::create_dir_all(&big).unwrap();
    for i in 0..big_files {
        write(
            &big.join(format!("file{i:06}.{}", exts[i % exts.len()])),
            "x",
        );
    }
    for i in 0..big_files / 20 {
        std::fs::create_dir(big.join(format!("folder{i:05}"))).unwrap();
    }
    for i in 0..big_files / 10 {
        write(&big.join(format!(".hidden{i:05}")), "h");
    }
    #[cfg(unix)]
    for i in 0..500 {
        std::os::unix::fs::symlink(
            big.join(format!("file{i:06}.{}", exts[i % exts.len()])),
            big.join(format!("link{i:04}")),
        )
        .unwrap();
    }
    // A profile folder: thousands of application folders, each a small tree.
    let appdata = base.join("AppData").join("Local");
    for i in 0..3_000 {
        let app = appdata.join(format!("Vendor{:03}.App{i:04}", i % 300));
        for sub in ["Cache", "Settings", ".state"] {
            let dir = app.join(sub);
            std::fs::create_dir_all(&dir).unwrap();
            for f in 0..3 {
                write(&dir.join(format!("data{f}.bin")), "d");
            }
        }
    }
    // A source tree to search.
    let tree = base.join("tree");
    for i in 0..tree_files {
        let dir = tree
            .join(format!("crate{:03}", i % 150))
            .join(format!("mod{:02}", i % 20));
        if i < 3_000 {
            std::fs::create_dir_all(&dir).unwrap();
        }
        let name = format!(
            "file{i:06}.{}",
            ["rs", "rs", "rs", "md", "json", "txt"][i % 6]
        );
        write(&dir.join(name), &source(i, 30, i % 200 == 0));
    }
    // Two versions of one long file.
    let file_a = base.join("a.rs");
    let file_b = base.join("b.rs");
    let old = source(1, 40_000, false);
    let new: String = old
        .lines()
        .enumerate()
        .map(|(n, line)| match n % 40 {
            7 => format!("{line} // changed\n"),
            19 => format!("{line}\n    let inserted_{n} = {n};\n"),
            31 => String::new(),
            _ => format!("{line}\n"),
        })
        .collect();
    write(&file_a, &old);
    write(&file_b, &new);
    // Two folder trees that differ in a few files.
    let (left, right) = (base.join("left"), base.join("right"));
    for i in 0..5_000 {
        for (side, changed) in [(&left, false), (&right, i % 50 == 0)] {
            let dir = side.join(format!("d{:02}", i % 50));
            if i < 50 {
                std::fs::create_dir_all(&dir).unwrap();
            }
            write(
                &dir.join(format!("f{i:05}.txt")),
                &source(i + changed as usize, 20, false),
            );
        }
    }
    let db = base.join("gall.db");
    {
        let store = Db::open(&db).unwrap();
        let paths: Vec<String> = (0..100_000)
            .map(|i| tagged_path(i % 1_000, i / 1_000))
            .collect();
        store.set_tags(&paths, 3).unwrap();
    }
    let zip = base.join("many.zip");
    {
        use std::io::Write as _;
        let file = std::io::BufWriter::new(std::fs::File::create(&zip).unwrap());
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for i in 0..50_000 {
            writer
                .start_file(format!("dir{:03}/file{i:05}.rs", i % 500), options)
                .unwrap();
            writer.write_all(source(i, 10, false).as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }
    let tar_gz = base.join("many.tar.gz");
    {
        let file = std::io::BufWriter::new(std::fs::File::create(&tar_gz).unwrap());
        let packed = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut writer = tar::Builder::new(packed);
        for i in 0..20_000 {
            let text = source(i, 30, false);
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(1_600_000_000);
            header.set_cksum();
            let name = format!("dir{:03}/file{i:05}.rs", i % 200);
            writer
                .append_data(&mut header, name, text.as_bytes())
                .unwrap();
        }
        writer.into_inner().unwrap().finish().unwrap();
    }
    // Ten blocks of 500 files each, as 7-Zip packs with a limited solid block size.
    let seven_z = base.join("many.7z");
    {
        use sevenz_rust2::{ArchiveEntry, ArchiveWriter, SourceReader};
        let mut writer = ArchiveWriter::create(&seven_z).unwrap();
        for block in 0..10 {
            let files: Vec<(String, String)> = (block * 500..(block + 1) * 500)
                .map(|i| (format!("d{block:02}/f{i:05}.rs"), source(i, 30, false)))
                .collect();
            writer
                .push_archive_entries(
                    files
                        .iter()
                        .map(|(name, _)| ArchiveEntry::new_file(name))
                        .collect(),
                    files
                        .iter()
                        .map(|(_, text)| SourceReader::from(text.as_bytes()))
                        .collect(),
                )
                .unwrap();
        }
        writer.finish().unwrap();
    }
    let scratch = base.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let small_zip = {
        let mut tick = |_| {};
        archive::compress(
            std::slice::from_ref(&left),
            &base.join("small.zip"),
            archive::ArchiveFormat::Zip,
            &AtomicBool::new(false),
            &mut tick,
        )
        .unwrap()
    };
    eprintln!("fixture built in {:.1} s", started.elapsed().as_secs_f32());
    Fixture {
        big,
        appdata,
        tree,
        left,
        right,
        file_a,
        file_b,
        db,
        zip,
        tar_gz,
        seven_z,
        small_zip,
        scratch,
    }
}

/// The path of tagged file `file` in folder `dir` of the tags fixture.
fn tagged_path(dir: usize, file: usize) -> String {
    let sep = std::path::MAIN_SEPARATOR;
    format!("{sep}tags{sep}folder{dir:04}{sep}file{file:04}.txt")
}

type Row<'a> = (&'static str, Box<dyn Fn() -> bool + 'a>);

/// 100 000 files of 40 types, as a listing returns them.
fn many_entries() -> Vec<gall_core::fs::FileEntry> {
    (0..100_000usize)
        .map(|i| {
            let ext = format!("t{:02}", i * 7919 % 40);
            let name = format!("file {}.{ext}", i * 7919 % 100_000);
            gall_core::fs::FileEntry {
                path: format!("/many/{name}"),
                name,
                kind: gall_core::fs::EntryKind::File,
                is_dir: false,
                is_hidden: false,
                is_symlink: false,
                cloud_only: false,
                mode: None,
                owner: None,
                link_target: None,
                attributes: None,
                size: (i * 31 % 5_000) as u64,
                modified: Some((i * 104_729 % 1_000_003) as i64),
                created: None,
                ext,
                readonly: false,
            }
        })
        .collect()
}

fn rows(f: &Fixture) -> Vec<Row<'_>> {
    let options = |show_hidden, dirs_only| listing::ListOptions {
        show_hidden,
        hide_temp: false,
        dirs_only,
    };
    let list = move |dir: &'_ Path, show_hidden, dirs_only, at_least: usize| {
        let dir = dir.to_path_buf();
        Box::new(move || {
            listing::list_dir(&dir, options(show_hidden, dirs_only))
                .is_ok_and(|l| l.len() >= at_least)
        }) as Box<dyn Fn() -> bool>
    };
    let never = AtomicBool::new(false);
    let never: &'static AtomicBool = Box::leak(Box::new(never));
    // An archive's list of entries is cached until the file changes: a new date before
    // every run makes each one read the archive again (and extract into a fresh folder).
    fn changed(archive: &Path) -> bool {
        static TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let date = std::time::UNIX_EPOCH + Duration::from_secs(1_600_000_000 + n);
        std::fs::File::options()
            .write(true)
            .open(archive)
            .and_then(|file| file.set_modified(date))
            .is_ok()
    }
    let list_archive = |archive: &Path, entries: usize| {
        let archive = archive.to_path_buf();
        Box::new(move || {
            changed(&archive)
                && archive_view::index(&archive).is_ok_and(|items| items.len() == entries)
        }) as Box<dyn Fn() -> bool>
    };
    let open_inside = |archive: &Path, inner: &'static str| {
        let archive = archive.to_path_buf();
        Box::new(move || {
            let inner = archive_view::virtual_path(&archive, inner);
            let Ok(out) = archive_view::materialize(&[PathBuf::from(inner)]) else {
                return false;
            };
            let found = out[0].is_file();
            // <cache>/<archive version>/entries/<folder>/<file>
            if let Some(root) = out[0].ancestors().nth(3) {
                let _ = std::fs::remove_dir_all(root);
            }
            changed(&archive) && found
        }) as Box<dyn Fn() -> bool>
    };
    // Packs the 5 000 files of `left` and removes the archive again.
    let pack = |format: archive::ArchiveFormat, name: &'static str| {
        let (source, scratch) = (f.left.clone(), f.scratch.clone());
        Box::new(move || {
            let mut tick = |_| {};
            let Ok(made) = archive::compress(
                std::slice::from_ref(&source),
                &scratch.join(name),
                format,
                never,
                &mut tick,
            ) else {
                return false;
            };
            let ok = std::fs::metadata(&made).is_ok_and(|m| m.len() > 10_000);
            let _ = std::fs::remove_file(&made);
            ok
        }) as Box<dyn Fn() -> bool>
    };
    // Unpacks `small_zip` (a folder `left` of 5 000 files) into a fresh folder and removes it again.
    let unpack = {
        let (zip, scratch) = (f.small_zip.clone(), f.scratch.clone());
        Box::new(move || {
            let mut tick = |_| {};
            let Ok(made) = archive::extract(
                &zip,
                &scratch,
                archive::ExtractMode::Folder,
                never,
                &mut tick,
            ) else {
                return false;
            };
            let files = made
                .first()
                .map(|root| {
                    std::fs::read_dir(root.join("left"))
                        .map(|d| d.count())
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            for path in made {
                let _ = std::fs::remove_dir_all(path);
            }
            files > 0
        }) as Box<dyn Fn() -> bool>
    };
    let find = |pattern: &'static str, at_least: usize| {
        let tree = f.tree.clone();
        Box::new(move || {
            let mut n = 0;
            search::search(
                &tree,
                pattern,
                search::SearchOptions::default(),
                never,
                |_| n += 1,
            );
            n >= at_least
        }) as Box<dyn Fn() -> bool>
    };
    let content = |pattern: &'static str, regex: bool, at_least: usize| {
        let tree = f.tree.clone();
        Box::new(move || {
            let opts = grep::GrepOptions {
                regex,
                ..Default::default()
            };
            grep::grep(&tree, pattern, &opts, never, &|_| {}).is_ok_and(|s| s.matches >= at_least)
        }) as Box<dyn Fn() -> bool>
    };
    let idx: &'static index::FileIndex = Box::leak(Box::new(index::FileIndex::new()));
    let sorted = |mode| {
        let entries = many_entries();
        Box::new(move || {
            let mut v = entries.clone();
            let sort = sort::SortSpec {
                key: sort::SortKey::Modified,
                descending: true,
                ..Default::default()
            };
            let group = sort::GroupSpec {
                mode,
                ..Default::default()
            };
            let groups = sort::group_entries(&mut v, &sort, &group);
            v.len() == 100_000 && groups.len() == if mode.by_type() { 40 } else { 0 }
        }) as Box<dyn Fn() -> bool>
    };
    let roots = vec![f.tree.clone(), f.big.clone(), f.appdata.clone()];
    let build_index = {
        let roots = roots.clone();
        Box::new(move || idx.rebuild(&roots, &index::IndexOptions::default(), never))
            as Box<dyn Fn() -> bool>
    };
    let ask = |query: &'static str| {
        Box::new(move || idx.query(query, None, 50).total > 0) as Box<dyn Fn() -> bool>
    };
    let file = |p: &Path| diff::DiffSource::File {
        path: p.to_path_buf(),
    };
    let (a, b) = (file(&f.file_a), file(&f.file_b));
    let (a2, b2) = (a.clone(), b.clone());
    let structural = diff::DiffOptions {
        structural: true,
        ..diff::DiffOptions::default()
    };
    let text_a = std::fs::read_to_string(&f.file_a).unwrap();
    let lines: Vec<String> = text_a.lines().map(str::to_owned).collect();
    let (left, right) = (f.left.clone(), f.right.clone());
    let (left2, right2) = (f.left.clone(), f.right.clone());
    let (left3, right3) = (f.left.clone(), f.right.clone());
    let (left4, right4) = (f.left.clone(), f.right.clone());
    let (tree_left, tree_right) = (f.tree.clone(), f.appdata.clone());
    let tree = f.tree.clone();
    let cmp = |content| dircmp::DirCmpOptions {
        content,
        hidden: true,
        respect_gitignore: false,
        ..Default::default()
    };
    // Filters that leave out whole folders and single files, and a size range, as the filter form sets them.
    let filtered = move || dircmp::DirCmpOptions {
        exclude: vec!["d0*/".to_owned(), "f*7.txt".to_owned()],
        include: vec!["*.txt".to_owned()],
        min_size: Some(1),
        ..cmp(false)
    };
    // Hidden files are not shown, so every folder is read once more to find what is not listed.
    let unlisted = move || dircmp::DirCmpOptions {
        hidden: false,
        ..cmp(false)
    };
    let store: &'static Db = Box::leak(Box::new(Db::open(&f.db).unwrap()));
    let sep = std::path::MAIN_SEPARATOR;
    let fresh: Vec<String> = (0..1_000)
        .map(|i| format!("{sep}fresh{sep}file{i:04}.txt"))
        .collect();
    let folder = move |dir: usize| format!("{sep}tags{sep}folder{dir:04}");
    vec![
        (
            "sort 100 000 entries by date",
            sorted(sort::GroupMode::Folders),
        ),
        (
            "sort 100 000 entries by date, grouped by type",
            sorted(sort::GroupMode::FoldersType),
        ),
        (
            "list a folder of 46 500 entries, hidden shown",
            list(&f.big, true, false, 40_000),
        ),
        (
            "list the same, hidden not shown",
            list(&f.big, false, false, 40_000),
        ),
        (
            "list the same, folders only (tree column)",
            list(&f.big, false, true, 2_000),
        ),
        (
            "list AppData/Local (3 000 folders)",
            list(&f.appdata, true, false, 3_000),
        ),
        (
            "list one application folder",
            list(&f.appdata.join("Vendor001.App0001"), true, false, 3),
        ),
        ("find names: *.md in 60 000 files", find("*.md", 10_000)),
        ("find names: file01234*", find("file01234*", 1)),
        (
            "search content: a rare word",
            content("needle_token", false, 300),
        ),
        (
            "search content: a common word",
            content("TODO", false, 20_000),
        ),
        (
            "search content: a regex",
            content(r"fn handler_9\d\(", true, 100),
        ),
        (
            "search content: no match",
            content("nothing_like_this_anywhere", false, 0),
        ),
        ("build the file index (115 000 entries)", build_index),
        ("Quick Open: file01234", ask("file01234")),
        ("Quick Open: one letter", ask("a")),
        ("Quick Open: two words", ask("crate mod17 rs")),
        (
            "Quick Open: no match",
            Box::new(move || idx.query("zzqqxx", None, 50).total == 0),
        ),
        (
            "diff two files of 40 000 lines",
            Box::new(move || {
                diff::compare(&a, &b, diff::DiffOptions::default()).is_ok_and(|d| d.text.is_some())
            }),
        ),
        (
            "diff two files of 40 000 lines by tokens (structural)",
            Box::new(move || {
                diff::compare(&a2, &b2, structural.clone()).is_ok_and(|d| d.text.is_some())
            }),
        ),
        (
            "highlight 40 000 lines",
            Box::new(move || syntax::highlight("a.rs", &lines).is_some()),
        ),
        (
            "compare two folders of 5 000 files (sizes and times)",
            Box::new(move || dircmp::compare_dirs(&left, &right, cmp(false), never).is_ok()),
        ),
        (
            "compare two folders of 5 000 files (content)",
            Box::new(move || dircmp::compare_dirs(&left2, &right2, cmp(true), never).is_ok()),
        ),
        (
            "compare two folders of 5 000 files (filtered)",
            Box::new(move || dircmp::compare_dirs(&left3, &right3, filtered(), never).is_ok()),
        ),
        (
            "compare two folders of 5 000 files (content, read again)",
            Box::new(move || {
                let fresh = dircmp::DirCmpOptions {
                    fresh: true,
                    ..cmp(true)
                };
                dircmp::compare_dirs(&left4, &right4, fresh, never).is_ok()
            }),
        ),
        (
            "compare a tree of 60 000 files with a profile folder, many folders (hidden files not shown)",
            Box::new(move || {
                dircmp::compare_dirs(&tree_left, &tree_right, unlisted(), never).is_ok()
            }),
        ),
        (
            "size of a folder of 60 000 files",
            Box::new(move || dirsize::dir_size(&tree, never, &|_| {}).is_some()),
        ),
        (
            "list a zip archive of 50 000 entries",
            list_archive(&f.zip, 50_500),
        ),
        (
            "open one file inside a zip archive of 50 000 entries",
            open_inside(&f.zip, "dir250/file49750.rs"),
        ),
        (
            "compress a folder of 5 000 files to a zip",
            pack(archive::ArchiveFormat::Zip, "pack.zip"),
        ),
        (
            "compress a folder of 5 000 files to a tar.gz",
            pack(archive::ArchiveFormat::TarGz, "pack.tar.gz"),
        ),
        ("extract a zip of 5 000 files", unpack),
        (
            "list a tar.gz archive of 20 000 files",
            list_archive(&f.tar_gz, 20_200),
        ),
        (
            "open an early file inside a tar.gz archive of 20 000 files",
            open_inside(&f.tar_gz, "dir050/file00050.rs"),
        ),
        (
            "list a 7z archive of 5 000 files",
            list_archive(&f.seven_z, 5_010),
        ),
        (
            "open one file inside a 7z archive of 5 000 files",
            open_inside(&f.seven_z, "d05/f02600.rs"),
        ),
        (
            "tag 1 000 files",
            Box::new(move || {
                store.set_tags(&fresh, 2).is_ok() && store.set_tags(&fresh, 0).is_ok()
            }),
        ),
        (
            "tags of a folder among 100 000 tags",
            Box::new(move || {
                (0..20).all(|dir| store.tags_in(&folder(dir)).is_ok_and(|t| t.len() == 100))
            }),
        ),
        (
            "rename 100 tagged files among 100 000 tags",
            Box::new(move || {
                (0..100).all(|i| {
                    let (from, to) = (tagged_path(999, i), tagged_path(999, i + 5_000));
                    store.move_tag(&from, &to).is_ok() && store.move_tag(&to, &from).is_ok()
                })
            }),
        ),
    ]
}

/// The time limits (see the module docs): `platform <tab> row <tab> factor`.
struct Limits {
    file: PathBuf,
    values: std::collections::BTreeMap<(String, String), f64>,
}

/// No limit below this many milliseconds.
const SMALLEST_LIMIT: f64 = 25.0;

impl Limits {
    fn load() -> Self {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/app_speed_limits.tsv");
        let values = std::fs::read_to_string(&file)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| {
                let mut fields = line.split('\t');
                let key = (fields.next()?.to_owned(), fields.next()?.to_owned());
                Some((key, fields.next()?.trim().parse().ok()?))
            })
            .collect();
        Limits { file, values }
    }

    fn get(&self, platform: &str, row: &str, reference: f64) -> Option<u64> {
        self.values
            .get(&(platform.to_owned(), row.to_owned()))
            .map(|factor| (factor * reference).max(SMALLEST_LIMIT).ceil() as u64)
    }

    fn record(&mut self, platform: &str, row: &str, slowest: Duration, reference: f64) {
        let factor = slowest.as_secs_f64() * 1000.0 * 1.5 / reference;
        let entry = self
            .values
            .entry((platform.to_owned(), row.to_owned()))
            .or_insert(0.0);
        *entry = entry.max((factor * 1000.0).ceil() / 1000.0);
    }

    fn save(&self) {
        let mut text = String::from(
            "# Time limits of the app speed test (crates/gall-core/tests/app_speed.rs), as multiples\n# of the reference time (reading and stat-ing a folder of 40 000 files, measured before\n# each row; on all threads of the machine for a row that uses threads, else on one):\n# 150 % of the slowest recorded run. Written by\n# GALL_SPEED_RECORD=1; delete a line to record that row again from scratch.\n",
        );
        for ((platform, row), factor) in &self.values {
            writeln!(text, "{platform}\t{row}\t{factor:.3}").unwrap();
        }
        std::fs::write(&self.file, text).expect("write the limits");
    }
}

/// The reference time in milliseconds: the faster of two passes that read `dir` and stat
/// every entry, with nothing of Gall's code. On one thread for a row that works on one;
/// for a row that shares its work between threads the entries are stat-ed on as many
/// threads as the machine gives, so the limit of such a row follows the number of cores
/// (a limit recorded on 16 cores would otherwise fail every run on 4).
fn reference(dir: &Path, threaded: bool) -> f64 {
    let threads = if threaded {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    } else {
        1
    };
    let pass = || {
        let start = Instant::now();
        let entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read the folder")
            .flatten()
            .collect();
        let part = entries.len().div_ceil(threads).max(1);
        let n: usize = std::thread::scope(|scope| {
            let handles: Vec<_> = entries
                .chunks(part)
                .map(|part| {
                    scope.spawn(move || part.iter().filter(|e| e.metadata().is_ok()).count())
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).sum()
        });
        assert!(n > 0);
        start.elapsed().as_secs_f64() * 1000.0
    };
    pass().min(pass())
}

/// Rows whose work is shared between threads; the others (diff, folder compare) run on
/// one. Keep in step with the code: a row moves here when its operation gets threads.
fn threaded(row: &str) -> bool {
    [
        "list ",
        "find names",
        "search content",
        "build the file index",
        "Quick Open",
        "highlight",
        "size of a folder",
    ]
    .iter()
    .any(|start| row.starts_with(start))
}

#[test]
#[ignore = "builds a large fixture; part of the release checks"]
fn the_app_stays_fast_outside_git() {
    let base = std::env::var_os("GALL_SPEED_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let temp = tempfile::Builder::new()
        .prefix("gall-app-speed-")
        .tempdir_in(base)
        .expect("temp folder");
    let fixture = build(temp.path());
    let platform = std::env::consts::OS;
    let record = std::env::var_os("GALL_SPEED_RECORD").is_some();
    let mut limits = Limits::load();
    let mut failures = Vec::new();
    let mut unlimited = 0;
    let only = std::env::var("GALL_SPEED_ONLY").unwrap_or_default();
    let mut table = format!(
        "\n### App speed ({platform})\n\n| Operation | first ms | median ms | worst ms | limit ms | reference ms | |\n|---|---:|---:|---:|---:|---:|---|\n"
    );
    for (name, run) in rows(&fixture) {
        if !only.is_empty() && !only.split(',').any(|part| name.contains(part)) {
            continue;
        }
        let reference = reference(&fixture.big, threaded(name));
        let time = |run: &dyn Fn() -> bool| -> (Duration, bool) {
            let start = Instant::now();
            let ok = run();
            (start.elapsed(), ok)
        };
        let (first, mut ok) = time(&run);
        let mut times = Vec::new();
        for _ in 0..RUNS {
            let (t, good) = time(&run);
            ok &= good;
            times.push(t);
        }
        times.sort();
        let (median, worst) = (times[RUNS / 2].as_millis() as u64, times[RUNS - 1]);
        if record && ok {
            limits.record(platform, name, worst, reference);
        }
        let limit = limits.get(platform, name, reference);
        let mut notes = Vec::new();
        if !ok {
            notes.push("the call failed".to_owned());
        }
        match limit {
            Some(limit) if median > limit => notes.push(format!("{median} ms, limit {limit} ms")),
            Some(_) => {}
            None => unlimited += 1,
        }
        writeln!(
            table,
            "| {name} | {} | {median} | {} | {} | {reference:.0} | {} |",
            first.as_millis(),
            worst.as_millis(),
            limit.map_or("-".to_owned(), |l| l.to_string()),
            if !notes.is_empty() {
                "**over**"
            } else if median > 200 {
                "above 200 ms"
            } else {
                ""
            }
        )
        .unwrap();
        if !notes.is_empty() {
            failures.push(format!("{name}: {}", notes.join("; ")));
        }
    }
    if unlimited > 0 {
        writeln!(
            table,
            "\n{unlimited} rows have no time limit for `{platform}` yet. Record them on this machine with `GALL_SPEED_RECORD=1`."
        )
        .unwrap();
    }
    eprintln!("{table}");
    if record {
        limits.save();
        eprintln!("limits written to {}", limits.file.display());
    }
    if let Some(file) = std::env::var_os("GALL_SPEED_REPORT") {
        use std::io::Write as _;
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .and_then(|mut f| f.write_all(table.as_bytes()));
    }
    assert!(
        failures.is_empty(),
        "the app got slower:\n  {}",
        failures.join("\n  ")
    );
}
