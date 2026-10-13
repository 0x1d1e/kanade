//! The JSON niri and pw-dump print and the tools we call take, as `serde_json` reads it.

pub use serde_json::Value as Json;

// none for anything but exactly one value, whitespace around it aside
pub fn parse(text: &str) -> Option<Json> {
    serde_json::from_str(text).ok()
}

// the items of an array, as a slice
pub fn array(value: &Json) -> Option<&[Json]> {
    value.as_array().map(Vec::as_slice)
}

// `text` as a JSON string, quoted, for a tool that takes JSON
pub fn quote(text: &str) -> String {
    Json::from(text).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quoted_text_reads_back_whole() {
        let text = "a \"b\" \\ \n\u{1}é\u{1F600}";

        assert_eq!(
            parse(&quote(text)).as_ref().and_then(Json::as_str),
            Some(text)
        );
        assert!(parse(r#"{"a": 1} x"#).is_none());
    }
}
