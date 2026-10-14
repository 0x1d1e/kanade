use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Component, PathBuf};

// what one drop may name; more than this is not a drag a person made
const MAX_FILES: usize = 256;

// Linux's own limit on a path
const MAX_PATH: usize = 4096;

/*
 * the files in a `text/uri-list`, which another program wrote and may have made up: only absolute
 * `file://` links on this machine count. A list with more than MAX_FILES links, or with one that
 * is not such a link, is refused whole, so a drop never holds only part of what was dragged.
 * The paths are names to be checked again by whoever opens them; nothing here touches the disk
 */
pub fn parse(list: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();

    // one link per line, and lines starting with # are comments
    for line in list.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some(path) = file_path(line) else {
            return Vec::new();
        };

        if files.len() == MAX_FILES {
            return Vec::new();
        }

        files.push(path);
    }

    files
}

fn file_path(link: &str) -> Option<PathBuf> {
    let rest = link.strip_prefix("file://")?;

    // the host is empty or this machine, as in file:///home or file://localhost/home
    let path = rest.strip_prefix("localhost").unwrap_or(rest);

    if !path.starts_with('/') {
        return None;
    }

    // a link's own query and fragment would be part of the name, so they are not guessed at;
    // a name holding them arrives as %3F and %23
    if path.contains(['?', '#']) {
        return None;
    }

    let decoded = decode(path)?;

    if decoded.len() > MAX_PATH || decoded.contains(&0) {
        return None;
    }

    let path = PathBuf::from(OsString::from_vec(decoded));

    // %2e%2e is a parent too, so this looks at the decoded name
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return None;
    }

    Some(path)
}

// spaces and other special letters arrive as %20 and the like; a % without two hex digits is bad
fn decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();

    let mut decoded = Vec::with_capacity(bytes.len());

    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;

            continue;
        }

        let hex = bytes.get(index + 1..index + 3)?;

        let high = char::from(hex[0]).to_digit(16)?;
        let low = char::from(hex[1]).to_digit(16)?;

        decoded.push((high * 16 + low) as u8);
        index += 3;
    }

    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_links_and_decodes_them() {
        let list = "# from a file manager\r\nfile:///home/me/My%20Clip.mov\r\n\r\nfile://localhost/tmp/%C3%A9\r\n";

        assert_eq!(
            parse(list),
            [
                PathBuf::from("/home/me/My Clip.mov"),
                PathBuf::from("/tmp/é")
            ]
        );
    }

    #[test]
    fn a_name_that_is_not_utf8_is_kept_as_bytes() {
        let files = parse("file:///tmp/%FF\n");

        assert_eq!(files[0].as_os_str().as_encoded_bytes(), b"/tmp/\xFF");
    }

    #[test]
    fn only_local_file_links_count() {
        for link in [
            "https://example.org/a",
            "file://host/tmp/a",
            "file:relative",
            "file://",
            "file://tmp/a",
            "/tmp/a",
            "FILE:///tmp/a",
        ] {
            assert!(parse(link).is_empty(), "{link}");
        }
    }

    #[test]
    fn a_name_that_climbs_out_is_refused() {
        for link in [
            "file:///tmp/../etc/passwd",
            "file:///tmp/%2e%2e/etc",
            "file:///tmp/%2E%2E",
            "file:///tmp/..%2Fetc",
            "file:///..",
        ] {
            assert!(parse(link).is_empty(), "{link}");
        }

        // dots inside a name are only a name
        assert_eq!(parse("file:///tmp/a..b").len(), 1);
    }

    #[test]
    fn a_bad_escape_or_a_nul_is_refused() {
        for link in [
            "file:///tmp/a%2",
            "file:///tmp/a%",
            "file:///tmp/a%zz",
            "file:///tmp/a%00b",
            "file:///tmp/a?b",
            "file:///tmp/a#b",
        ] {
            assert!(parse(link).is_empty(), "{link}");
        }
    }

    #[test]
    fn one_bad_link_refuses_the_whole_drop() {
        assert!(parse("file:///tmp/a\nhttps://example.org\nfile:///tmp/b\n").is_empty());
    }

    #[test]
    fn the_count_and_the_length_are_bounded() {
        let at_limit = "file:///tmp/a\n".repeat(MAX_FILES);

        assert_eq!(parse(&at_limit).len(), MAX_FILES);
        assert!(parse(&format!("{at_limit}file:///tmp/a\n")).is_empty());

        let long = format!("file:///{}", "a".repeat(MAX_PATH));

        assert!(parse(&long).is_empty());
        assert_eq!(
            parse(&format!("file:///{}", "a".repeat(MAX_PATH - 1))).len(),
            1
        );
    }
}
