//! `pw-dump --monitor`, which `privacy` and `audio` each follow on their own (ADR 0003, ADR 0011):
//! it prints PipeWire's graph, then every object that changes, so a reader blocks on its output and
//! wakes only when the graph does. What an object means stays with the Module reading it.

use std::io::{self, BufRead};

use super::json::Json;

// prints the PipeWire graph, and each change to it
pub const DUMP: &str = "pw-dump";

// what `wake::run` runs it with
pub const MONITOR: &[&str] = &["--monitor", "--no-colors"];

// sets one key of a metadata, like the defaults
pub const SET_METADATA: &str = "pw-metadata";

pub const NODE: &str = "PipeWire:Interface:Node";
pub const LINK: &str = "PipeWire:Interface:Link";
pub const METADATA: &str = "PipeWire:Interface:Metadata";

/*
 * hands `print` the objects of each print until the output ends, and says why it ended. Each print
 * is a JSON array whose closing bracket alone on a line ends it, the objects inside being indented
 */
pub fn prints(lines: impl BufRead, mut print: impl FnMut(&[Json])) -> io::Error {
    let mut text = String::new();

    for line in lines.lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => return error,
        };

        text.push_str(&line);
        text.push('\n');

        if line != "]" {
            continue;
        }

        // a print this reader cannot follow is skipped, the next one still counts
        let Some(objects) = Json::parse(&text) else {
            eprintln!("kanade: skipped a pw-dump print that is not JSON");
            text.clear();
            continue;
        };

        text.clear();
        print(objects.as_array().unwrap_or_default());
    }

    io::ErrorKind::UnexpectedEof.into()
}

// an object as pw-dump prints it
pub struct Object<'a>(pub &'a Json);

impl Object<'_> {
    // ids are unique across types
    pub fn id(&self) -> Option<u64> {
        self.0.get("id").and_then(Json::as_u64)
    }

    // removed objects print only their id and a null info, naming no type
    pub fn removed(&self) -> bool {
        self.0.get("info") == Some(&Json::Null)
    }

    pub fn kind(&self) -> Option<&str> {
        self.0.get("type").and_then(Json::as_str)
    }

    pub fn info(&self) -> Option<&Json> {
        self.0.get("info").filter(|info| **info != Json::Null)
    }

    // a text property of a node
    pub fn prop(&self, key: &str) -> Option<&str> {
        self.info()?.get("props")?.get(key)?.as_str()
    }

    // a property of a node that is true, not false or missing
    pub fn flag(&self, key: &str) -> bool {
        let flag = || self.info()?.get("props")?.get(key)?.as_bool();

        flag() == Some(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(text: &str) -> (Vec<Vec<u64>>, io::ErrorKind) {
        let mut prints = Vec::new();

        let lost = super::prints(text.as_bytes(), |objects| {
            prints.push(
                objects
                    .iter()
                    .filter_map(|object| Object(object).id())
                    .collect(),
            )
        });

        (prints, lost.kind())
    }

    #[test]
    fn each_print_ends_at_its_closing_bracket() {
        let text = "[\n  {\n    \"id\": 1\n  },\n  {\n    \"id\": 2\n  }\n]\n[\n  {\n    \"id\": 3\n  }\n]\n";

        assert_eq!(
            ids(text),
            (vec![vec![1, 2], vec![3]], io::ErrorKind::UnexpectedEof)
        );
    }

    #[test]
    fn a_print_that_is_not_json_is_skipped() {
        let text = "[\n  {\n]\n[\n  {\n    \"id\": 3\n  }\n]\n";

        assert_eq!(ids(text).0, vec![vec![3]]);
    }

    #[test]
    fn only_a_null_info_is_a_removal() {
        let removed = Json::parse(r#"{"id": 4, "info": null}"#).unwrap();
        let metadata = Json::parse(r#"{"id": 4, "type": "x", "metadata": []}"#).unwrap();

        assert!(Object(&removed).removed());
        assert!(!Object(&metadata).removed());
    }
}
