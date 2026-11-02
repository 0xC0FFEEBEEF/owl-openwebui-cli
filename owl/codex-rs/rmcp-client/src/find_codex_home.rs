use dirs::home_dir;
use std::path::PathBuf;

/// This was copied from codex-core but codex-core depends on this crate.
/// TODO: move this to a shared crate lower in the dependency tree.
///
///
/// Returns the path to owl's settings home, which owl keeps to itself: `OWL_HOME`,
/// then `CODEX_HOME` for compatibility with existing scripts and test harnesses,
/// then `~/.owl`.
///
/// - If `OWL_HOME` or `CODEX_HOME` is set, the value will be canonicalized and this
///   function will Err if the path does not exist.
/// - Otherwise this function does not verify that the directory exists.
pub(crate) fn find_codex_home() -> std::io::Result<PathBuf> {
    // Honor an explicit home when one is set, `OWL_HOME` first.
    for var in ["OWL_HOME", "CODEX_HOME"] {
        if let Ok(val) = std::env::var(var)
            && !val.is_empty()
        {
            return PathBuf::from(val).canonicalize();
        }
    }

    let mut p = home_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not find home directory",
        )
    })?;
    p.push(".owl");
    Ok(p)
}
