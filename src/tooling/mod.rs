use std::ffi::OsStr;

pub(crate) fn windows_reserved_stem(text: &str) -> bool {
    let stem = text.split('.').next().unwrap_or(text).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || matches!(
            stem.as_str(),
            "COM¹" | "COM²" | "COM³" | "LPT¹" | "LPT²" | "LPT³"
        )
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

pub(crate) fn safe_archive_component(name: &OsStr) -> bool {
    let text = name.to_string_lossy();
    if text.is_empty()
        || text == "."
        || text == ".."
        || text.ends_with('.')
        || text.ends_with(' ')
        || text.contains(':')
        || text.chars().any(char::is_control)
    {
        return false;
    }
    !windows_reserved_stem(&text)
}

pub mod dap;
pub mod doc_blocks;
pub mod doc_comments;
pub mod installer;
pub mod lockfile;
pub mod lsp;
pub mod manifest;
pub mod native;
pub mod pods;
pub mod publish;
pub mod registry;
pub mod repl;
pub mod solver;
pub mod target;
pub mod upgrade;
