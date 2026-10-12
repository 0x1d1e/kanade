//! The short label of a keyboard layout, like "US" for "English (US)", for the layout OSD. niri
//! names a layout as XKB describes it; the rules list XKB installs (`evdev.lst`) says which layout
//! each description is, a variant's by its layout. A name it does not list keeps its first letters.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

// the most letters a label has, so it fits its lamp
const LONGEST: usize = 3;

// where XKB keeps its rules lists, the first that reads; NixOS has no /usr/share
const SYSTEM: [&str; 2] = ["/usr/share/X11/xkb", "/run/current-system/sw/share/X11/xkb"];

// every description the rules list, read once on the first switch
static CODES: OnceLock<HashMap<String, String>> = OnceLock::new();

// the label for the layout XKB describes as `name`
pub fn short(name: &str) -> String {
    label(
        name,
        CODES.get_or_init(|| codes(&read().unwrap_or_default())),
    )
}

fn label(name: &str, codes: &HashMap<String, String>) -> String {
    let letters: String = match codes.get(name) {
        Some(code) => code.chars().filter(char::is_ascii_alphanumeric).collect(),
        None => name
            .chars()
            .filter(|letter| letter.is_alphabetic())
            .take(2)
            .collect(),
    };

    letters
        .chars()
        .take(LONGEST)
        .collect::<String>()
        .to_uppercase()
}

fn read() -> Option<String> {
    let roots = env::var_os("XKB_CONFIG_ROOT")
        .map(PathBuf::from)
        .into_iter()
        .chain(SYSTEM.map(PathBuf::from));

    roots
        .flat_map(|root| ["evdev.lst", "base.lst"].map(|list| root.join("rules").join(list)))
        .find_map(|path| fs::read_to_string(path).ok())
}

/*
 * each layout's and variant's description to its layout's code, from a rules list: sections headed
 * `! layout` and `! variant`, a layout line `  us  English (US)`, a variant line
 * `  dvorak  us: English (Dvorak)`. A layout's own description wins over a variant's
 */
fn codes(list: &str) -> HashMap<String, String> {
    let mut codes = HashMap::new();
    let mut variants = Vec::new();
    let mut section = "";

    for line in list.lines() {
        if let Some(header) = line.strip_prefix('!') {
            section = header.trim();
            continue;
        }

        let Some((code, description)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        let description = description.trim();

        match section {
            "layout" => {
                codes.insert(description.to_owned(), code.to_owned());
            }
            "variant" => {
                if let Some((layout, description)) = description.split_once(": ") {
                    variants.push((description.to_owned(), layout.to_owned()));
                }
            }
            _ => {}
        }
    }

    for (description, layout) in variants {
        codes.entry(description).or_insert(layout);
    }

    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "! model
  pc105           Generic 105-key PC

! layout
  us              English (US)
  de              German
  ch              German (Switzerland)
  epo             Esperanto

! variant
  dvorak          us: English (Dvorak)
  ch              de: German (Switzerland)
  nodeadkeys      de: German (no dead keys)

! option
  grp                  Switching to another layout
";

    #[test]
    fn a_layout_reads_as_its_code_and_a_variant_as_its_layout_s() {
        let codes = codes(LIST);

        assert_eq!(label("English (US)", &codes), "US");
        assert_eq!(label("German", &codes), "DE");
        assert_eq!(label("Esperanto", &codes), "EPO");
        assert_eq!(label("English (Dvorak)", &codes), "US");
        assert_eq!(label("German (no dead keys)", &codes), "DE");

        // listed as a layout and as a variant of another, the layout wins
        assert_eq!(label("German (Switzerland)", &codes), "CH");

        // models and options are no layouts
        assert_eq!(label("Generic 105-key PC", &codes), "GE");
    }

    // with no rules list, or a layout of the user's own, the name's first letters
    #[test]
    fn an_unlisted_layout_keeps_its_first_letters() {
        assert_eq!(label("English (US)", &HashMap::new()), "EN");
        assert_eq!(label("Русская", &HashMap::new()), "РУ");
        assert_eq!(label("", &HashMap::new()), "");
    }
}
