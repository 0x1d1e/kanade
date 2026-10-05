//! Test-only structural check that `island/` is pure: every file except `island/service.rs`
//! must not name Amane, the service module, or anything in Kanade outside `island/`.
//! Parsed with syn, so aliases (`use amane as ui`), nesting, and test code are all covered.

use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::{TokenStream, TokenTree};
use syn::visit::{self, Visit};
use syn::{Attribute, ItemMod, ItemUse, Macro, Path as SynPath, UseTree};

const SERVICE: &str = "service";

struct Checker {
    // how deep the current module sits below `island`, `super` may not climb above 0
    depth: usize,
    violations: Vec<String>,
}

impl Checker {
    // `path` is the segments of a path or a flattened use tree, in order
    fn check_segments(&mut self, segments: &[String]) {
        let first = segments.first().map(String::as_str);

        let supers = segments.iter().take_while(|s| *s == "super").count();

        if first == Some("amane") {
            self.violations
                .push(format!("uses amane: {}", segments.join("::")));
        }

        if supers > self.depth {
            self.violations
                .push(format!("`super` escapes island: {}", segments.join("::")));
        }

        // `crate::` is only allowed to stay inside island
        if first == Some("crate") && segments.get(1).map(String::as_str) != Some("island") {
            self.violations.push(format!(
                "depends on Kanade outside island: {}",
                segments.join("::")
            ));
        }

        // multi-segment so a local named `service` is fine, `service::x` and `a::service` are not
        if segments.len() > 1 && segments.iter().any(|s| s == SERVICE) {
            self.violations
                .push(format!("uses service: {}", segments.join("::")));
        }
    }

    fn check_tokens(&mut self, tokens: TokenStream) {
        for token in tokens {
            match token {
                TokenTree::Ident(ident) if ident == "amane" => {
                    self.violations.push("macro mentions amane".into());
                }
                TokenTree::Group(group) => self.check_tokens(group.stream()),
                _ => {}
            }
        }
    }
}

fn flatten(tree: &UseTree, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            flatten(&path.tree, prefix, out);
            prefix.pop();
        }
        UseTree::Name(name) => out.push([prefix.as_slice(), &[name.ident.to_string()]].concat()),
        UseTree::Rename(rename) => {
            out.push([prefix.as_slice(), &[rename.ident.to_string()]].concat());
        }
        UseTree::Glob(_) => out.push(prefix.clone()),
        UseTree::Group(group) => {
            for tree in &group.items {
                flatten(tree, prefix, out);
            }
        }
    }
}

impl<'ast> Visit<'ast> for Checker {
    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        let mut paths = Vec::new();

        flatten(&item.tree, &mut Vec::new(), &mut paths);

        for segments in paths {
            self.check_segments(&segments);
        }
    }

    fn visit_path(&mut self, path: &'ast SynPath) {
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();

        self.check_segments(&segments);

        visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, mac: &'ast Macro) {
        self.visit_path(&mac.path);

        self.check_tokens(mac.tokens.clone());
    }

    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        if has_path_attr(&item.attrs) {
            self.violations
                .push("#[path] module escapes the checked tree".into());
        }

        if item.content.is_some() {
            self.depth += 1;

            visit::visit_item_mod(self, item);

            self.depth -= 1;
        }
    }

    fn visit_item_extern_crate(&mut self, item: &'ast syn::ItemExternCrate) {
        self.violations.push(format!("extern crate {}", item.ident));
    }
}

fn has_path_attr(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident("path"))
}

// violations in one file, `depth` is the file's module depth below `island`
pub fn violations(source: &str, depth: usize) -> Vec<String> {
    let file = syn::parse_file(source).expect("island file must parse");

    let mut checker = Checker {
        depth,
        violations: Vec::new(),
    };

    checker.visit_file(&file);

    checker.violations
}

// every .rs under island except service.rs, with its module depth below island
fn island_files(folder: &Path, depth: usize, out: &mut Vec<(PathBuf, usize)>) {
    let mut entries: Vec<_> = fs::read_dir(folder)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();

    entries.sort();

    for path in entries {
        if path.is_dir() {
            island_files(&path, depth + 1, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let name = path.file_name().unwrap();

            if depth == 0 && name == "service.rs" {
                continue;
            }

            // mod.rs is the folder's own module, any other file is one level deeper
            let file_depth = if name == "mod.rs" { depth } else { depth + 1 };

            out.push((path, file_depth));
        }
    }
}

#[test]
fn island_is_pure() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/island");

    let mut files = Vec::new();

    island_files(&folder, 0, &mut files);

    assert!(!files.is_empty());

    // service.rs is the only exception, and `pub mod service;` in island/mod.rs declares it
    for (path, depth) in files {
        let source = fs::read_to_string(&path).unwrap();

        let mut found = violations(&source, depth);

        if path.ends_with("island/mod.rs") {
            found.retain(|v| v != "#[path] module escapes the checked tree");
        }

        assert!(found.is_empty(), "{}: {found:?}", path.display());
    }
}

#[cfg(test)]
mod checker {
    use super::violations;

    fn flagged(source: &str, depth: usize) -> bool {
        !violations(source, depth).is_empty()
    }

    #[test]
    fn catches_direct_and_aliased_amane() {
        assert!(flagged("use amane::Color;", 1));
        assert!(flagged("use amane as ui;", 1));
        assert!(flagged("use ::amane::Color;", 1));
        assert!(flagged("use {amane::Color, std::fmt};", 1));
        assert!(flagged("fn f() -> amane::Color { todo!() }", 1));
        assert!(flagged("extern crate amane as ui;", 1));
    }

    #[test]
    fn catches_service_in_any_form() {
        assert!(flagged("use super::service::IslandService;", 1));
        assert!(flagged("use super::service as s;", 1));
        assert!(flagged("use crate::island::service::IslandService;", 1));
        assert!(flagged("fn f() { let _ = service::IslandService; }", 1));
    }

    #[test]
    fn catches_nested_modules_tests_and_macros() {
        assert!(flagged("mod inner { use amane::Color; }", 1));
        assert!(flagged("#[cfg(test)] mod t { use amane::Color; }", 1));
        assert!(flagged(
            "fn f() { println!(\"{}\", amane::Color::BLUE); }",
            1
        ));
        assert!(flagged("#[path = \"../view.rs\"] mod v;", 1));
    }

    #[test]
    fn catches_escaping_kanade() {
        assert!(flagged("use crate::view::island;", 1));
        assert!(flagged("use crate::theme::CANVAS_WIDTH;", 1));
        // island/mod.rs is depth 0, so its `super` is the crate root
        assert!(flagged("use super::theme::CANVAS_WIDTH;", 0));
        assert!(flagged("use super::super::theme::CANVAS_WIDTH;", 1));
    }

    #[test]
    fn allows_pure_code() {
        assert!(!flagged("use std::time::Duration;", 1));
        assert!(!flagged("use super::activity::Activity;", 1));
        assert!(!flagged("use crate::island::activity::Activity;", 1));
        assert!(!flagged("fn f(service: u8) -> u8 { service }", 1));
        assert!(!flagged("mod inner { use super::Thing; }", 1));
    }
}
