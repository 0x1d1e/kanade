//! Per-app notification rules (`[[notifications.rules]]`): each matches a notification by the app,
//! desktop entry or category its sender gives, or a piece of its summary, and says what it does: a
//! Banner or none, kept in the history or not, with a sound or none, shown under DND, how long its Banner stays and
//! which urgency it counts as. Matching ignores case; every match field a rule gives must match,
//! and the first rule that matches wins. With none matching, a notification does what it always
//! did. A rule applies once, as its notification arrives.
//!
//! A rule names no notification content in anything it logs: a problem names the rule by number.

use std::time::Duration;

use toml::Value;

use super::Urgency;
use crate::config::{LONGEST, SHORTEST};

// one `[[notifications.rules]]` entry, as the file gives it
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Rule {
    // what it matches; at least one is given
    pub app: Option<String>,
    pub desktop_entry: Option<String>,
    pub category: Option<String>,

    // a piece of the summary
    pub summary: Option<String>,

    // what it does; none leaves that as it would be
    pub banner: Option<bool>,
    pub history: Option<bool>,
    pub sound: Option<bool>,
    pub bypass_dnd: Option<bool>,
    pub timeout: Option<Duration>,
    pub urgency: Option<Urgency>,
}

// what a notification does, by the rule that matched it or none
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effects {
    // shows as a Banner
    pub banner: bool,

    // stays in the history once its Banner is gone; without, it closes as expired then
    pub history: bool,

    // plays the sound `notifications.sound` gives, which it never does with that off
    pub sound: bool,

    // its Banner shows under DND, as a Critical one does
    pub bypass_dnd: bool,

    // how long its Banner shows, Critical ones too; none keeps the urgency's time
    pub timeout: Option<Duration>,

    // the urgency it counts as, over the one its sender gave
    pub urgency: Option<Urgency>,
}

impl Default for Effects {
    fn default() -> Self {
        Effects {
            banner: true,
            history: true,
            sound: true,
            bypass_dnd: false,
            timeout: None,
            urgency: None,
        }
    }
}

// what of a notification a rule matches
#[derive(Debug, Clone, Copy)]
pub struct Fields<'a> {
    pub app: &'a str,
    pub desktop_entry: &'a str,
    pub category: &'a str,
    pub summary: &'a str,
}

const MATCHES: [&str; 4] = ["app", "desktop_entry", "category", "summary"];
const EFFECTS: [&str; 6] = [
    "banner",
    "history",
    "sound",
    "bypass_dnd",
    "timeout",
    "urgency",
];

// what the first rule matching does, the defaults for none
pub fn resolve(rules: &[Rule], fields: Fields) -> Effects {
    let Some(rule) = rules.iter().find(|rule| rule.matches(fields)) else {
        return Effects::default();
    };
    let default = Effects::default();

    Effects {
        banner: rule.banner.unwrap_or(default.banner),
        history: rule.history.unwrap_or(default.history),
        sound: rule.sound.unwrap_or(default.sound),
        bypass_dnd: rule.bypass_dnd.unwrap_or(default.bypass_dnd),
        timeout: rule.timeout,
        urgency: rule.urgency,
    }
}

impl Rule {
    // every match field given matches, ignoring case
    fn matches(&self, fields: Fields) -> bool {
        let same = |given: &Option<String>, field: &str| {
            given
                .as_ref()
                .is_none_or(|given| given.to_lowercase() == field.to_lowercase())
        };
        let entry = |given: &Option<String>| {
            given.as_ref().is_none_or(|given| {
                desktop_id(given).to_lowercase() == desktop_id(fields.desktop_entry).to_lowercase()
            })
        };
        let within = self.summary.as_ref().is_none_or(|piece| {
            fields
                .summary
                .to_lowercase()
                .contains(&piece.to_lowercase())
        });

        self.given()
            && same(&self.app, fields.app)
            && entry(&self.desktop_entry)
            && same(&self.category, fields.category)
            && within
    }

    // a rule with no match field would match every notification, so none is read
    fn given(&self) -> bool {
        self.app.is_some()
            || self.desktop_entry.is_some()
            || self.category.is_some()
            || self.summary.is_some()
    }

    // `notifications.rules` as the file gives it: a list of tables, each checked whole
    pub fn read_all(value: &Value) -> Result<Vec<Rule>, String> {
        let list = value
            .as_array()
            .ok_or("expected a list of rules, as [[notifications.rules]]")?;

        list.iter()
            .enumerate()
            .map(|(index, rule)| {
                Rule::read(rule).map_err(|why| format!("rule {}: {why}", index + 1))
            })
            .collect()
    }

    fn read(value: &Value) -> Result<Rule, String> {
        let table = value.as_table().ok_or("expected a table")?;
        let mut rule = Rule::default();

        for (key, value) in table {
            let flag = || {
                value
                    .as_bool()
                    .ok_or_else(|| format!("{key}: expected true or false"))
            };
            let text = || {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|text| !text.is_empty() && !text.contains('\n'))
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{key}: expected a line of text"))
            };

            match key.as_str() {
                "app" => rule.app = Some(text()?),
                "desktop_entry" => rule.desktop_entry = Some(text()?),
                "category" => rule.category = Some(text()?),
                "summary" => rule.summary = Some(text()?),
                "banner" => rule.banner = Some(flag()?),
                "history" => rule.history = Some(flag()?),
                "sound" => rule.sound = Some(flag()?),
                "bypass_dnd" => rule.bypass_dnd = Some(flag()?),
                "timeout" => rule.timeout = Some(timeout(value)?),
                "urgency" => rule.urgency = Some(urgency(value)?),
                key => {
                    return Err(format!(
                        "unknown key {key}, expected {} or {}",
                        MATCHES.join(", "),
                        EFFECTS.join(", ")
                    ));
                }
            }
        }

        if !rule.given() {
            return Err(format!("matches nothing: give {}", MATCHES.join(", ")));
        }

        Ok(rule)
    }

    // as the file would give it
    pub fn value(&self) -> Value {
        let mut table = toml::Table::new();
        let mut text = |key: &str, given: &Option<String>| {
            if let Some(given) = given {
                table.insert(key.to_owned(), Value::String(given.clone()));
            }
        };

        text("app", &self.app);
        text("desktop_entry", &self.desktop_entry);
        text("category", &self.category);
        text("summary", &self.summary);

        for (key, flag) in [
            ("banner", self.banner),
            ("history", self.history),
            ("sound", self.sound),
            ("bypass_dnd", self.bypass_dnd),
        ] {
            if let Some(flag) = flag {
                table.insert(key.to_owned(), Value::Boolean(flag));
            }
        }

        if let Some(timeout) = self.timeout {
            let ms = i64::try_from(timeout.as_millis()).unwrap_or(i64::MAX);
            table.insert(String::from("timeout"), Value::Integer(ms));
        }

        if let Some(urgency) = self.urgency {
            table.insert(
                String::from("urgency"),
                Value::String(name(urgency).to_owned()),
            );
        }

        Value::Table(table)
    }
}

// a desktop entry with or without its `.desktop`
fn desktop_id(entry: &str) -> &str {
    entry.strip_suffix(".desktop").unwrap_or(entry)
}

fn timeout(value: &Value) -> Result<Duration, String> {
    let ms = value.as_integer().ok_or("timeout: expected milliseconds")?;

    u64::try_from(ms)
        .ok()
        .filter(|ms| (SHORTEST..=LONGEST).contains(ms))
        .map(Duration::from_millis)
        .ok_or_else(|| format!("timeout: {ms} is outside {SHORTEST}-{LONGEST} ms"))
}

const URGENCIES: [(&str, Urgency); 3] = [
    ("low", Urgency::Low),
    ("normal", Urgency::Normal),
    ("critical", Urgency::Critical),
];

fn urgency(value: &Value) -> Result<Urgency, String> {
    let expected = "urgency: expected \"low\" or \"normal\" or \"critical\"";
    let given = value.as_str().ok_or(expected)?;

    URGENCIES
        .iter()
        .find(|(name, _)| *name == given)
        .map(|&(_, urgency)| urgency)
        .ok_or_else(|| format!("{expected}, found \"{given}\""))
}

fn name(urgency: Urgency) -> &'static str {
    URGENCIES
        .iter()
        .find(|(_, named)| *named == urgency)
        .map_or("normal", |(name, _)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(text: &str) -> Result<Vec<Rule>, String> {
        let table: toml::Table = text.parse().expect("valid TOML");

        Rule::read_all(&table["rules"])
    }

    fn fields<'a>(app: &'a str, summary: &'a str) -> Fields<'a> {
        Fields {
            app,
            desktop_entry: "",
            category: "",
            summary,
        }
    }

    #[test]
    fn no_rule_keeps_todays_behavior() {
        assert_eq!(
            resolve(&[], fields("Firefox", "Download done")),
            Effects::default()
        );
        assert_eq!(
            Effects::default(),
            Effects {
                banner: true,
                history: true,
                sound: true,
                bypass_dnd: false,
                timeout: None,
                urgency: None,
            }
        );
    }

    #[test]
    fn matching_ignores_case_and_needs_every_field_given() {
        let rules = rules(
            r#"
            [[rules]]
            app = "slack"
            summary = "DEPLOY"
            banner = false
            "#,
        )
        .unwrap();

        // both given match, whatever the case
        assert!(!resolve(&rules, fields("Slack", "Deploy finished")).banner);
        assert!(!resolve(&rules, fields("SLACK", "the deploy failed")).banner);

        // the app alone is not enough, nor the summary alone
        assert!(resolve(&rules, fields("Slack", "lunch?")).banner);
        assert!(resolve(&rules, fields("Discord", "deploy finished")).banner);

        // the app matches whole, never a piece of it
        assert!(resolve(&rules, fields("Slacker", "deploy")).banner);
    }

    #[test]
    fn desktop_entry_and_category_match_with_or_without_desktop() {
        let rules = rules(
            r#"
            [[rules]]
            desktop_entry = "org.gnome.Nautilus.desktop"
            history = false

            [[rules]]
            category = "im.received"
            bypass_dnd = true
            "#,
        )
        .unwrap();

        let nautilus = Fields {
            app: "Files",
            desktop_entry: "org.gnome.nautilus",
            category: "",
            summary: "",
        };
        let message = Fields {
            app: "Chat",
            desktop_entry: "",
            category: "IM.Received",
            summary: "",
        };

        assert!(!resolve(&rules, nautilus).history);
        assert!(resolve(&rules, message).bypass_dnd);
        assert!(resolve(&rules, message).history);
        assert_eq!(resolve(&rules, fields("Chat", "")), Effects::default());
    }

    #[test]
    fn the_first_matching_rule_wins_whole() {
        let rules = rules(
            r#"
            [[rules]]
            app = "mail"
            summary = "invoice"
            urgency = "critical"

            [[rules]]
            app = "mail"
            banner = false
            timeout = 2000
            "#,
        )
        .unwrap();

        // the first rule's effects only: nothing of the second's adds to them
        let invoice = resolve(&rules, fields("Mail", "Your invoice"));
        assert_eq!(invoice.urgency, Some(Urgency::Critical));
        assert!(invoice.banner);
        assert_eq!(invoice.timeout, None);

        let other = resolve(&rules, fields("Mail", "Hello"));
        assert!(!other.banner);
        assert_eq!(other.timeout, Some(Duration::from_millis(2000)));
        assert_eq!(other.urgency, None);
    }

    #[test]
    fn a_bad_rule_is_refused_by_number() {
        let refused = |text| rules(text).unwrap_err();

        assert_eq!(
            refused("rules = [{ app = \"a\" }, { banner = false }]"),
            "rule 2: matches nothing: give app, desktop_entry, category, summary"
        );
        assert!(
            refused("rules = [{ app = \"a\", banner = \"no\" }]").starts_with("rule 1: banner:")
        );
        assert!(refused("rules = [{ app = \"a\", timeout = 0 }]").contains("outside"));
        assert!(refused("rules = [{ app = \"a\", timeout = 60001 }]").contains("outside"));
        assert!(refused("rules = [{ app = \"a\", urgency = \"high\" }]").contains("\"high\""));
        assert!(refused("rules = [{ app = \"\" }]").contains("app: expected a line of text"));
        assert!(refused("rules = [{ ap = \"a\" }]").contains("unknown key ap"));
        assert!(refused("rules = \"a\"").contains("expected a list"));
        assert!(refused("rules = [\"a\"]").contains("expected a table"));
    }

    #[test]
    fn a_rule_reads_back_as_written() {
        let read = rules(
            r#"
            [[rules]]
            app = "Mail"
            desktop_entry = "thunderbird"
            category = "email"
            summary = "invoice"
            banner = false
            history = true
            sound = false
            bypass_dnd = true
            timeout = 1500
            urgency = "low"
            "#,
        )
        .unwrap();

        let again = Rule::read_all(&Value::Array(read.iter().map(Rule::value).collect()));

        assert_eq!(again, Ok(read));
    }
}
