//! Round trips with the standard tools: what Gall writes is applied by GNU `patch` (`-u`, `-c`, `-n`), and what GNU `diff`
//! writes (`-u`, `-c`, plain) is read and applied by Gall, on disposable files, including a missing final newline, CRLF,
//! Unicode names and names with spaces, and GNU `patch` finds the files by the names in Gall's headers. On Windows the
//! tools come from the `PATH` or from Git for Windows (`usr\bin` next to its `git`). The tests are skipped, and say so,
//! where `diff` or `patch` is not found.

use gall_core::fs::patch::{apply_plan, make_patch, parse, plan, sniff, write, Format};
use std::path::{Path, PathBuf};
use std::process::Command;

fn works(program: &Path) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|o| {
            if !o.status.success() {
                return false;
            }
            let out = String::from_utf8_lossy(&o.stdout).to_ascii_lowercase();
            // Reject Strawberry Perl's ancient Windows GNU patch 2.5.x (asserts on LF hunks
            // and lacks quoted filename support); require modern GNU diffutils / patch 2.7+.
            out.contains("gnu") && !out.contains("patch 2.5.")
        })
}

/// The program of a GNU tool: on Windows prefer Git for Windows' `usr\bin` copy (Git's
/// `--exec-path` is `<Git>\mingw64\libexec\git-core`) over legacy ports on `PATH` such as
/// Strawberry Perl's `patch 2.5.9`; elsewhere use `PATH` (including Homebrew's `gdiff`/`gpatch`
/// on macOS, where `/usr/bin/patch` is BSD `patch 2.0-12u11-Apple`).
fn tool(name: &str) -> Option<PathBuf> {
    if cfg!(windows) {
        if let Ok(exec) = Command::new("git").arg("--exec-path").output() {
            let exec = PathBuf::from(String::from_utf8_lossy(&exec.stdout).trim());
            if let Some(found) = exec
                .ancestors()
                .map(|dir| dir.join("usr").join("bin").join(format!("{name}.exe")))
                .find(|candidate| candidate.is_file() && works(candidate))
            {
                return Some(found);
            }
        }
    }
    let plain = PathBuf::from(name);
    if works(&plain) {
        return Some(plain);
    }
    if cfg!(target_os = "macos") {
        let gnu_prefixed = PathBuf::from(format!("g{name}"));
        if works(&gnu_prefixed) {
            return Some(gnu_prefixed);
        }
    }
    None
}

fn gnu() -> bool {
    let ok = tool("diff").is_some() && tool("patch").is_some();
    if !ok {
        // The output of a passing test is not shown: a missing tool is a failure where it is required (`GALL_REQUIRE_GNU=1`,
        // set on the machines that are meant to have the tools), and a skip with a message elsewhere.
        assert!(
            std::env::var_os("GALL_REQUIRE_GNU").is_none(),
            "GNU diff and patch are required here (GALL_REQUIRE_GNU) and were not found"
        );
        eprintln!("skipped: GNU diff and patch are not installed (on Windows: not in Git for Windows either)");
    }
    ok
}

const FORMATS: [(Format, &str, &str); 3] = [
    (Format::Unified, "-u", "-u"),
    (Format::Context, "-c", "-c"),
    (Format::Normal, "-n", ""),
];

/// (name of the file, old text, new text): the cases every round trip is run on.
fn cases() -> Vec<(&'static str, String, String)> {
    vec![
        (
            "plain.txt",
            "a\nb\nc\nd\ne\n".into(),
            "a\nB\nc\nd\ne\nf\n".into(),
        ),
        (
            "my file.txt",
            "one\ntwo\nthree\n".into(),
            "one\n2\nthree\n".into(),
        ),
        (
            "fäile ü.txt",
            "žvižgač\nšola\n".into(),
            "žvižgač\nšola\nčaj\n".into(),
        ),
        ("no-final-newline.txt", "a\nb".into(), "a\nb\nc".into()),
        ("gains-newline.txt", "a\nb".into(), "a\nb\n".into()),
        ("loses-newline.txt", "a\nb\n".into(), "a\nb".into()),
        (
            "crlf.txt",
            "a\r\nb\r\nc\r\n".into(),
            "a\r\nB\r\nc\r\nd\r\n".into(),
        ),
        (
            "many.txt",
            (0..60).map(|i| format!("line {i}\n")).collect(),
            (0..60)
                .map(|i| {
                    if i % 20 == 3 {
                        format!("changed {i}\n")
                    } else {
                        format!("line {i}\n")
                    }
                })
                .collect(),
        ),
        ("empties.txt", "a\n\n\nb\n".into(), "a\n\nb\n\n".into()),
    ]
}

fn run(dir: &Path, name: &str, args: &[&str]) -> std::process::Output {
    let program = tool(name).unwrap_or_else(|| panic!("{name} was not found"));
    Command::new(&program)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("{}: {e}", program.display()))
}

#[test]
fn gnu_patch_applies_what_gall_writes_in_every_format() {
    if !gnu() {
        return;
    }
    for (format, flag, _) in FORMATS {
        for (name, old, new) in cases() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(name), &old).unwrap();
            // Written with the file's own name, as `patch` expects it in a header.
            let text = write(format, &[make_patch(name, name, &old, &new, format)]);
            std::fs::write(dir.path().join("p.patch"), &text).unwrap();
            let out = run(
                dir.path(),
                "patch",
                &[flag, "--batch", "-i", "p.patch", name],
            );
            assert!(
                out.status.success(),
                "{format:?} {name}: patch said {}{}\n--- patch ---\n{text}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                std::fs::read(dir.path().join(name)).unwrap(),
                new.as_bytes(),
                "{format:?} {name}\n{text}"
            );
        }
    }
}

#[test]
fn gall_applies_what_gnu_diff_writes_in_every_format() {
    if !gnu() {
        return;
    }
    for (format, _, diff_flag) in FORMATS {
        for (name, old, new) in cases() {
            let dir = tempfile::tempdir().unwrap();
            let (a, b) = (dir.path().join("a"), dir.path().join("b"));
            std::fs::create_dir_all(&a).unwrap();
            std::fs::create_dir_all(&b).unwrap();
            std::fs::write(a.join(name), &old).unwrap();
            std::fs::write(b.join(name), &new).unwrap();
            let relative_a = format!("a/{name}");
            let relative_b = format!("b/{name}");
            let mut args = vec![];
            if !diff_flag.is_empty() {
                args.push(diff_flag);
            }
            args.extend([relative_a.as_str(), relative_b.as_str()]);
            let out = run(dir.path(), "diff", &args);
            // `diff` exits with 1 when the files differ.
            assert!(
                out.status.code() == Some(1),
                "{format:?} {name}: diff said {:?}",
                out.status
            );
            let text = String::from_utf8(out.stdout).unwrap();
            assert_eq!(
                sniff(&text),
                Some(format),
                "{format:?} {name}: not recognised\n{text}"
            );
            let mut patches =
                parse(format, &text).unwrap_or_else(|e| panic!("{format:?} {name}: {e}\n{text}"));
            // A plain diff carries no names: the target is chosen, as the dialog lets the user do.
            let strip = if format == Format::Normal {
                for patch in &mut patches {
                    patch.old_name = Some(name.to_owned());
                }
                None
            } else {
                Some(1)
            };
            // Applied to a copy of the old tree.
            let work = dir.path().join("work");
            std::fs::create_dir_all(&work).unwrap();
            std::fs::write(work.join(name), &old).unwrap();
            let planned = plan(&work, &patches, strip)
                .unwrap_or_else(|p| panic!("{format:?} {name}: {p:?}\n{text}"));
            apply_plan(&planned).unwrap_or_else(|e| panic!("{format:?} {name}: {e:?}"));
            assert_eq!(
                std::fs::read(work.join(name)).unwrap(),
                new.as_bytes(),
                "{format:?} {name}\n{text}"
            );
        }
    }
}

#[test]
fn gall_refuses_what_gnu_patch_would_also_reject() {
    if !gnu() {
        return;
    }
    // A patch made for other content: both tools refuse it and leave the file as it was.
    let (old, new) = ("a\nb\nc\n", "a\nB\nc\n");
    let text = write(
        Format::Unified,
        &[make_patch("f.txt", "f.txt", old, new, Format::Unified)],
    );
    let other = "x\ny\nz\n";
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), other).unwrap();
    std::fs::write(dir.path().join("p.patch"), &text).unwrap();
    let out = run(
        dir.path(),
        "patch",
        &["-u", "--batch", "-i", "p.patch", "f.txt"],
    );
    assert!(!out.status.success());
    let patches = parse(Format::Unified, &text).unwrap();
    assert!(plan(dir.path(), &patches, None).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("f.txt")).unwrap(),
        other
    );
}

#[test]
fn gnu_patch_finds_the_files_by_the_names_gall_writes() {
    if !gnu() {
        return;
    }
    // Without a file on the command line `patch` reads the name from the header: a bare name ends at a space or a tab,
    // so Gall quotes such names as GNU `diff` and Git do.
    let mut names = vec!["plain.txt", "my file.txt", "fäile ü.txt", "trailing .txt"];
    if !cfg!(windows) {
        names.extend(["tab\there.txt", "say \"hi\".txt", "back\\slash x.txt"]);
    }
    for (format, flag, _) in FORMATS.into_iter().filter(|(f, ..)| *f != Format::Normal) {
        for name in &names {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join("sub")).unwrap();
            let file = dir.path().join("sub").join(name);
            std::fs::write(&file, "one\ntwo\n").unwrap();
            let header = |side: &str| format!("{side}/sub/{name}");
            let text = write(
                format,
                &[make_patch(
                    &header("a"),
                    &header("b"),
                    "one\ntwo\n",
                    "one\n2\n",
                    format,
                )],
            );
            std::fs::write(dir.path().join("p.patch"), &text).unwrap();
            let out = run(
                dir.path(),
                "patch",
                &[flag, "-p1", "--batch", "-i", "p.patch"],
            );
            assert!(
                out.status.success(),
                "{format:?} {name:?}: patch said {}{}\n--- patch ---\n{text}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                std::fs::read_to_string(&file).unwrap(),
                "one\n2\n",
                "{format:?} {name:?}\n{text}"
            );
        }
    }
}
