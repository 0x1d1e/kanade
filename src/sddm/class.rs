//! What an installed SDDM theme takes a picture from, told by its files: the class decides what
//! `kanade sddm sync` can do for it. Everything here reads a theme somebody else wrote, so the
//! reads are bounded and what is shown of it has its control characters removed.

use std::fs;
use std::path::{Path, PathBuf};

// what a theme does with a picture
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    // reads the picture from a `theme.conf` key (`config.<key>`): Kanade replaces that key
    A(String),

    // draws a fixed file of its own (the path, relative to the theme), which Kanade leaves
    B(String),

    // has no picture setting and no picture file
    C,

    // plays a video (the file or the setting), which Kanade does not replace
    D(String),
}

impl Class {
    // in a few words: the class and what it is told by
    pub fn brief(&self) -> String {
        match self {
            Class::A(key) => format!("A ({key})"),
            Class::B(file) => format!("B ({file})"),
            Class::C => String::from("C"),
            Class::D(video) => format!("D ({video})"),
        }
    }

    // what `kanade sddm sync` does for a theme of this class
    pub fn does(&self) -> String {
        match self {
            Class::A(key) => format!(
                "sync writes the picture and sets `{key}` in the overlay's theme.conf.user to it"
            ),
            Class::B(file) => format!(
                "the theme draws its own {file}, which sync does not replace: it links the theme and writes no picture"
            ),
            Class::C => String::from(
                "the theme has no picture setting: sync links the theme and writes no picture",
            ),
            Class::D(video) => format!(
                "the theme plays {video}, which sync does not replace: it links the theme and writes no picture"
            ),
        }
    }

    // whether sync writes a picture for the theme
    pub fn takes_picture(&self) -> bool {
        matches!(self, Class::A(_))
    }
}

// the most files of a theme looked at, and how deep, a theme being a few dozen
const MOST_FILES: usize = 600;
const DEEPEST: usize = 4;

// the most bytes read of one file
const MOST_BYTES: u64 = 256 << 10;

const VIDEOS: &[&str] = &["mp4", "webm", "mkv", "mov", "avi", "ogv", "m4v"];
const IMAGES: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "svg", "bmp"];

// a file name stem that says the file is the theme's background
const BACKGROUNDS: &[&str] = &["background", "bg", "wallpaper", "backdrop"];

// `text` without a byte order mark, which an editor on another system may put first
pub fn unbom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

// `text` shown to a terminal: no control character, which could carry an escape sequence
pub fn plain(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

// the `(section, key, value)` of an ini file's lines, BOM and comments and quotes off
pub fn pairs(text: &str) -> Vec<(String, String, String)> {
    let mut section = String::new();
    let mut out = Vec::new();

    for line in unbom(text).lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }

        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_owned();
        } else if let Some((key, value)) = line.split_once('=') {
            let value = value.trim().trim_matches(['"', '\'']);

            out.push((section.clone(), key.trim().to_owned(), value.to_owned()));
        }
    }

    out
}

// a regular file's text, and whether that was all of it
#[derive(Debug)]
pub struct Read {
    pub text: String,

    // the file is longer than `MOST_BYTES`, so `text` is its start
    pub whole: bool,
}

/*
 * a regular file's text, no more than `MOST_BYTES` of it, `whole` saying whether that was all of it.
 * The error is why there is no text: not a regular file (a pipe, a device, a directory, reached by
 * name or through a link), which could block or never end, or the open or the read failing, as it
 * failed. The open does not wait either, in case the file is swapped for a pipe after the check
 */
pub fn read(path: &Path) -> Result<Read, String> {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt;

    let named = || path.display().to_string();
    let meta = fs::metadata(path).map_err(|error| format!("{}: {error}", named()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", named()));
    }

    // one byte past the limit says whether there was more, which the file's length alone cannot
    let mut bytes = Vec::new();
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)
        .and_then(|file| file.take(MOST_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("{}: {error}", named()))?;

    let whole = bytes.len() as u64 <= MOST_BYTES;
    bytes.truncate(MOST_BYTES as usize);

    Ok(Read {
        whole,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

// its text, for a caller that only wants that
pub fn text(path: &Path) -> Option<String> {
    read(path).ok().map(|read| read.text)
}

fn extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

// the files of `dir` as paths relative to it, bounded; directories are walked, links are not followed
fn files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, at: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > DEEPEST || out.len() > MOST_FILES {
            return;
        }

        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(fs::DirEntry::file_name);

        for entry in entries {
            let rel = at.join(entry.file_name());
            // a link to a directory is not entered; a link to a file counts as the file
            let Ok(kind) = entry.file_type() else {
                continue;
            };

            if kind.is_dir() {
                walk(&entry.path(), &rel, depth + 1, out);
            } else {
                out.push(rel);
            }
        }
    }

    let mut out = Vec::new();
    walk(dir, Path::new(""), 0, &mut out);

    out
}

/*
 * the key a QML file reads of `config`: every `config.<name>` in `source`, the name as written.
 * A theme that reads `config.background` takes a picture from the setting even when its own
 * theme.conf does not list it
 */
fn config_keys(source: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut rest = source;

    while let Some(at) = rest.find("config.") {
        let before = rest[..at].chars().next_back();
        rest = &rest[at + "config.".len()..];

        // `myconfig.x` and `a.config.x` are some other object's
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
            continue;
        }

        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();

        if !name.is_empty() {
            keys.push(name);
        }
    }

    keys
}

/*
 * the class of the theme in `dir`. `own` is Kanade's theme, which reads `backdrop`. The order: a
 * video is D (nothing Kanade writes would show over it), a `background` key or a QML file reading
 * `config.background` is A, a file named like a background is B, else C. A color in the key is no
 * picture, so a theme whose `background` is `#rrggbb` is C
 */
pub fn classify(dir: &Path, own: bool) -> Class {
    if own {
        return Class::A(String::from("backdrop"));
    }

    let listed = files(dir);
    let name = |rel: &Path| rel.to_string_lossy().into_owned();

    // theme.conf and the user's file over it, which decide what the setting holds
    let mut settings: Vec<(String, String, String)> = Vec::new();
    for conf in ["theme.conf", "theme.conf.user"] {
        if let Ok(read) = read(&dir.join(conf)) {
            settings.extend(pairs(&read.text));
        }
    }

    // what a theme says of itself goes to the terminal, so it is stripped of control characters
    // here as the setting below is: the file name is as much the theme's text as a value is
    let video = listed
        .iter()
        .find(|rel| VIDEOS.contains(&extension(&name(rel)).as_str()))
        .map(|rel| plain(&name(rel)))
        .or_else(|| {
            settings
                .iter()
                .find(|(_, _, value)| VIDEOS.contains(&extension(value).as_str()))
                .map(|(_, _, value)| plain(value))
        });
    if let Some(video) = video {
        return Class::D(video);
    }

    let color = |value: &str| value.starts_with('#');
    let key = settings
        .iter()
        .rev()
        .find(|(_, key, value)| key.eq_ignore_ascii_case("background") && !color(value))
        .map(|(_, key, _)| key.clone());

    let key = key.or_else(|| {
        listed
            .iter()
            .filter(|rel| extension(&name(rel)) == "qml")
            .filter_map(|rel| read(&dir.join(rel)).ok().map(|read| read.text))
            .flat_map(|source| config_keys(&source))
            .find(|key| key.eq_ignore_ascii_case("background"))
    });
    if let Some(key) = key {
        return Class::A(plain(&key));
    }

    // a color the theme's own key holds is a setting, but not of a picture
    if settings
        .iter()
        .any(|(_, key, value)| key.eq_ignore_ascii_case("background") && color(value))
    {
        return Class::C;
    }

    let fixed = listed.iter().find(|rel| {
        let file = name(rel);
        let stem = Path::new(&file)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        IMAGES.contains(&extension(&file).as_str()) && BACKGROUNDS.contains(&stem.as_str())
    });

    match fixed {
        Some(file) => Class::B(plain(&name(file))),
        None => Class::C,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kanade-sddm-class-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        for (rel, text) in files {
            let path = dir.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        dir
    }

    // a theme's files decide how Kanade treats it; a wrong class writes a picture nobody reads or
    // leaves one a theme would have shown
    #[test]
    fn a_theme_is_told_by_its_files() {
        type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], Class);
        let cases: Vec<Case> = vec![
            (
                "key",
                &[("theme.conf", "[General]\nbackground=assets/bg.jpg\n")],
                Class::A(String::from("background")),
            ),
            (
                "bom",
                &[("theme.conf", "\u{feff}[General]\nbackground=a.png\n")],
                Class::A(String::from("background")),
            ),
            (
                "qml",
                &[
                    ("theme.conf", "[General]\nfont=x\n"),
                    ("Main.qml", "Image { source: config.background }"),
                ],
                Class::A(String::from("background")),
            ),
            (
                "other",
                &[(
                    "Main.qml",
                    "Image { source: myconfig.background; x: a.config.background }",
                )],
                Class::C,
            ),
            (
                "fixed",
                &[("Main.qml", ""), ("images/Background.PNG", "")],
                Class::B(String::from("images/Background.PNG")),
            ),
            (
                "none",
                &[("Main.qml", ""), ("theme.conf", "[General]\nfont=x\n")],
                Class::C,
            ),
            (
                "color",
                &[("theme.conf", "[General]\nBackground=#112233\n")],
                Class::C,
            ),
            (
                "video-file",
                &[("theme.conf", "background=a.png\n"), ("loop.MP4", "")],
                Class::D(String::from("loop.MP4")),
            ),
            (
                "video-key",
                &[("theme.conf", "[General]\nbackground=\"bg/loop.webm\"\n")],
                Class::D(String::from("bg/loop.webm")),
            ),
        ];

        for (name, files, want) in cases {
            let dir = theme(name, files);
            assert_eq!(classify(&dir, false), want, "{name}");
            fs::remove_dir_all(dir).unwrap();
        }

        assert_eq!(
            classify(Path::new("/nonexistent-kanade-theme"), true),
            Class::A(String::from("backdrop"))
        );
    }

    // a theme's name for a file goes to the terminal: nothing in it moves the cursor, whether the
    // name is a setting's value or a file of the theme's own
    #[test]
    fn what_a_theme_says_is_shown_without_control_characters() {
        assert_eq!(plain("a\u{1b}[2Jb\n"), "a[2Jb");

        let dir = theme(
            "escape",
            &[("theme.conf", "background=\"x\u{1b}[31m.mp4\"\n")],
        );
        let Class::D(video) = classify(&dir, false) else {
            panic!("a video")
        };
        assert!(!video.contains('\u{1b}'));
        fs::remove_dir_all(dir).unwrap();

        let dir = theme("escape-file", &[("x\u{1b}[31m.MP4", "")]);
        let Class::D(video) = classify(&dir, false) else {
            panic!("a video")
        };
        assert!(!video.contains('\u{1b}'), "{video:?}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_ini_file_with_a_byte_order_mark_reads_its_first_section() {
        let pairs = pairs("\u{feff}[General]\nbackground = \"a.png\"\n; c\n#c\nx=1");

        assert_eq!(
            pairs[0],
            ("General".into(), "background".into(), "a.png".into())
        );
        assert_eq!(pairs.len(), 2);
    }

    // a seed over the limit is said to be cut, and why there is no text at all is the real error
    #[test]
    fn a_read_says_what_went_wrong_instead_of_nothing() {
        let dir = std::env::temp_dir().join(format!("kanade-sddm-read-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let big = dir.join("big");
        fs::write(&big, vec![b'a'; MOST_BYTES as usize + 1]).unwrap();
        let cut = read(&big).unwrap();
        assert!(!cut.whole, "cut at the limit");
        assert_eq!(cut.text.len() as u64, MOST_BYTES);

        let exact = dir.join("exact");
        fs::write(&exact, vec![b'a'; MOST_BYTES as usize]).unwrap();
        assert!(read(&exact).unwrap().whole, "the limit itself is whole");

        let missing = read(&dir.join("absent")).unwrap_err();
        assert!(missing.contains("absent"), "{missing}");
        assert!(
            missing.contains("No such file") || missing.contains("os error"),
            "the open's own error: {missing}"
        );
        assert!(read(&dir).unwrap_err().contains("not a file"));

        fs::remove_dir_all(dir).unwrap();
    }
}
