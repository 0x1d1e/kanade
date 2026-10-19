//! The first empty wallpaper folder is filled from the Favorites list of
//! <https://github.com/ItsTerm1n4l/Wallpapers>. A state file gates it, so a folder the user later
//! empties is not filled again. A failed download leaves what arrived and does not mark the seed
//! done. Nothing here sets a wallpaper.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ureq::Agent;

use crate::atomic::{self, Mode};
use crate::config;
use crate::sources::body;

const LIST: &str =
    "https://raw.githubusercontent.com/ItsTerm1n4l/Wallpapers/refs/heads/main/pages/Favorites.md";
const IMAGES: &str = "https://raw.githubusercontent.com/ItsTerm1n4l/Wallpapers/main/images/";

const MARKER: &str = "kanade/wallpaper-seed";

// the favorites page, and one picture
const PAGE: u64 = 256 * 1024;
const PICTURE: u64 = 25 * 1024 * 1024;

const PATIENCE: Duration = Duration::from_secs(30);

pub fn run() {
    let Some(dir) = super::directory() else {
        return;
    };
    let Some(marker) = marker() else {
        return;
    };
    if marker.is_file() {
        return;
    }

    let page = match fetch_text(LIST, PAGE) {
        Ok(page) => page,
        Err(error) => {
            eprintln!("kanade: wallpaper: the favorites list was not downloaded: {error}");
            return;
        }
    };
    let wanted = files(&page);
    if wanted.is_empty() {
        eprintln!("kanade: wallpaper: the favorites list named no images");
        return;
    }

    // a missing marker is an unfinished seed, including a folder that already holds pictures;
    // files already present are skipped, and the marker is written only when every download works
    if let Err(error) = fs::create_dir_all(&dir) {
        eprintln!("kanade: wallpaper: {error}");
        return;
    }

    let agent = agent();
    let mut failed = false;
    for file in &wanted {
        let dest = dir.join(file);
        if dest.is_file() && fs::metadata(&dest).is_ok_and(|meta| meta.len() > 0) {
            continue;
        }
        let url = image_url(file);
        let part = dir.join(format!(".{file}.part"));
        match fetch_file(&agent, &url, &part, PICTURE) {
            Ok(()) => {
                if fs::rename(&part, &dest).is_err() {
                    let _ = fs::remove_file(&part);
                    failed = true;
                    eprintln!("kanade: wallpaper: could not save {file}");
                }
            }
            Err(error) => {
                let _ = fs::remove_file(&part);
                failed = true;
                eprintln!("kanade: wallpaper: could not download {file}: {error}");
            }
        }
    }

    if !failed && let Err(error) = mark(&marker) {
        eprintln!("kanade: wallpaper: {error}");
    }
}

// image file names from markdown image links, `![name](/images/<file>)` or a full URL
pub fn files(markdown: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = markdown;

    while let Some(start) = rest.find("](") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find(')') else {
            break;
        };
        let link = &rest[..end];
        rest = &rest[end + 1..];

        let Some(file) = image_name(link) else {
            continue;
        };
        if !found.iter().any(|have| have == &file) {
            found.push(file);
        }
    }

    found
}

fn image_name(link: &str) -> Option<String> {
    let path = link.split(['?', '#']).next()?.trim();
    let at = path.find("/images/")?;
    let file = &path[at + "/images/".len()..];
    if file.is_empty()
        || file.contains('/')
        || file.contains('\\')
        || file.contains("..")
        || file.starts_with('.')
    {
        return None;
    }
    super::image(Path::new(file)).then(|| file.to_owned())
}

fn image_url(file: &str) -> String {
    let mut url = String::from(IMAGES);
    for byte in file.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                url.push(byte as char);
            }
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

fn marker() -> Option<PathBuf> {
    config::base(
        "XDG_STATE_HOME",
        ".local/state",
        std::env::var("HOME").ok().as_deref(),
    )
    .map(|dir| dir.join(MARKER))
}

fn mark(path: &Path) -> Result<(), String> {
    atomic::write(path, b"seeded\n", Mode::Private).map_err(|error| error.to_string())
}

fn agent() -> Agent {
    Agent::config_builder()
        .https_only(true)
        .max_redirects(3)
        .timeout_global(Some(PATIENCE))
        .http_status_as_error(false)
        .user_agent(concat!("Kanade/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn fetch_text(url: &str, limit: u64) -> Result<String, String> {
    let agent = agent();
    let mut response = agent.get(url).call().map_err(|error| error.to_string())?;
    if response.status() != 200 {
        return Err(format!("HTTP {}", response.status()));
    }
    body::text(response.body_mut(), limit).map_err(|error| error.to_string())
}

fn fetch_file(agent: &Agent, url: &str, dest: &Path, limit: u64) -> Result<(), String> {
    let mut response = agent.get(url).call().map_err(|error| error.to_string())?;
    if response.status() != 200 {
        return Err(format!("HTTP {}", response.status()));
    }

    let mut bytes = Vec::new();
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .reader()
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;

    if bytes.len() as u64 > limit || bytes.len() < 32 {
        return Err("short".to_owned());
    }

    atomic::write(dest, &bytes, Mode::Shared).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorites_markdown_names_its_image_files() {
        let page = "\
# Favorites

![Abstract stripe](/images/Abstract-stripe-corner.jpg)
![Storm](https://raw.githubusercontent.com/ItsTerm1n4l/Wallpapers/main/images/airplane-flying-through-stormy-clouds.jpeg)
![Again](/images/Abstract-stripe-corner.jpg)
![Not an image](/images/notes.txt)
![Outside](/other/picture.png)
![Escape](/images/../secret.png)
";
        assert_eq!(
            files(page),
            vec![
                "Abstract-stripe-corner.jpg".to_owned(),
                "airplane-flying-through-stormy-clouds.jpeg".to_owned(),
            ]
        );
    }
}
