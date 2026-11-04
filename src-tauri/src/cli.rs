//! Command-line startup options (X-01).
//!
//! ```text
//! gall [PATH]                 open PATH in browser 1
//! gall -p PATH | --path PATH  browser 1
//! gall -p2 PATH               browser 2
//! gall -o PATH [--newtab]     open in running instance (same or new tab)
//! gall --nowindow             start hidden (reserved)
//! ```

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliArgs {
    pub path1: Option<String>,
    pub path2: Option<String>,
    pub new_tab: bool,
    pub no_window: bool,
    /// Debug builds: run the UI self-test (GALL_SELFTEST=1).
    pub selftest: bool,
    /// "demo" = README screenshots instead of the test suite.
    pub selftest_mode: String,
}

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> CliArgs {
    let mut out = CliArgs::default();
    let mut it = args.into_iter().skip(1).peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-p" | "-path" | "--path" | "-o" | "--open" => out.path1 = it.next(),
            "-p2" | "--path2" => out.path2 = it.next(),
            "-newtab" | "--newtab" => out.new_tab = true,
            "-nowindow" | "--nowindow" => out.no_window = true,
            s if s.starts_with('-')
                && s.len() > 1
                && std::path::Path::new(&s[1..]).is_absolute() =>
            {
                out.path1 = Some(s[1..].to_string())
            }
            s if !s.starts_with('-') => out.path1 = Some(s.to_string()),
            other => log::warn!("unknown argument {other}"),
        }
    }
    let norm = |p: String| {
        let e = gall_core::nav::path::expand(&p);
        gall_core::nav::path::canonicalize(std::path::Path::new(&e))
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or(e)
    };
    out.selftest = cfg!(debug_assertions) && std::env::var("GALL_SELFTEST").is_ok();
    out.selftest_mode = if out.selftest {
        std::env::var("GALL_SELFTEST").unwrap_or_default()
    } else {
        String::new()
    };
    out.path1 = out.path1.map(norm);
    out.path2 = out.path2.map(norm);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: &[&str]) -> CliArgs {
        parse(
            std::iter::once("gall")
                .chain(v.iter().copied())
                .map(String::from),
        )
    }

    #[cfg(unix)]
    #[test]
    fn variants() {
        assert_eq!(
            p(&["/tmp"]).path1.as_deref(),
            Some(std::fs::canonicalize("/tmp").unwrap().to_str().unwrap())
        );
        assert!(p(&["-o", "/", "-newtab"]).new_tab);
        assert_eq!(p(&["-p2", "/"]).path2.as_deref(), Some("/"));
        assert_eq!(p(&["-/"]).path1.as_deref(), Some("/"));
    }

    #[cfg(windows)]
    #[test]
    fn variants_windows() {
        let tmp = std::env::temp_dir();
        let got = p(&[tmp.to_str().unwrap()]).path1.unwrap();
        assert!(!got.starts_with(r"\\?\"), "verbatim prefix leaked: {got}");
        assert!(std::path::Path::new(&got).is_dir());
        assert!(p(&["-o", r"C:\", "-newtab"]).new_tab);
        assert_eq!(p(&["-p2", r"C:\"]).path2.as_deref(), Some(r"C:\"));
    }
}
