//! The island core. Pure and Amane-free except `service`, so it unit-tests with plain `cargo test`.

pub mod service;

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    // pure modules must not reach Amane or the service that wraps them
    #[test]
    fn pure_modules_do_not_touch_amane_or_service() {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/island");

        let forbidden = [["amane", "::"].concat(), ["service", "::"].concat()];

        for entry in fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();

            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }

            if path.file_name().is_some_and(|name| name == "service.rs") {
                continue;
            }

            let code = fs::read_to_string(&path).unwrap();

            // this test names its own needles, so only the code before any test module counts
            let code = code.split("#[cfg(test)]").next().unwrap();

            for needle in &forbidden {
                let hits = code.lines().filter(|line| line.contains(needle.as_str()));

                assert_eq!(hits.count(), 0, "{} uses `{needle}`", path.display());
            }
        }
    }
}
