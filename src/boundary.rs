//! Test-only structural checks. `island/` is pure: every file except `island/service.rs`
//! must not name the runtime, the service module, or anything in Kanade outside `island/`.
//! Parsed with syn, so aliases (`use kanade_runtime as ui`), nesting, and test code are all covered.
//! And `wayland_client` stays in `doctor/`: the runtime owns Kanade's Wayland, doctor only inspects it;
//! `crates/lock`, outside `src/`, has its own connection, for the lock screen (ADR 0025).
//! And no color literal outside `theme.rs` (#106): components draw theme tokens. Test code may.

use std::collections::HashSet;
use std::fs;
use std::iter::Peekable;
use std::path::{Path, PathBuf};

use proc_macro2::{TokenStream, TokenTree};
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{
    Attribute, Expr, ExprCall, ExprPath, ImplItemFn, Item, ItemMod, ItemUse, LitStr, Macro, Meta,
    MetaList, Path as SynPath, Token, UseRename, UseTree,
};

const SERVICE: &str = "service";

// the runtime that draws, which island/ never names
const RUNTIME: &str = "kanade_runtime";

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

        if first == Some(RUNTIME) {
            self.violations
                .push(format!("uses the runtime: {}", segments.join("::")));
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

    // macro bodies are opaque to syn, so rebuild the `a::b::c` paths from tokens and apply
    // the same rules; a lone `kanade_runtime` identifier is rejected anywhere
    fn check_tokens(&mut self, tokens: TokenStream) {
        let mut tokens = tokens.into_iter().peekable();

        while let Some(token) = tokens.next() {
            match token {
                TokenTree::Ident(ident) => {
                    let mut segments = vec![ident.to_string()];

                    while let Some(next) = take_path_segment(&mut tokens) {
                        segments.push(next);
                    }

                    if segments.len() == 1 && segments[0] == RUNTIME {
                        self.violations.push("macro mentions the runtime".into());
                    }

                    self.check_segments(&segments);
                }
                TokenTree::Group(group) => self.check_tokens(group.stream()),
                _ => {}
            }
        }
    }
}

// consumes `:: ident` if it comes next
fn take_path_segment(tokens: &mut Peekable<proc_macro2::token_stream::IntoIter>) -> Option<String> {
    let mut lookahead = tokens.clone();

    let colon =
        |token: Option<TokenTree>| matches!(token, Some(TokenTree::Punct(p)) if p.as_char() == ':');

    if !colon(lookahead.next()) || !colon(lookahead.next()) {
        return None;
    }

    let Some(TokenTree::Ident(ident)) = lookahead.next() else {
        return None;
    };

    *tokens = lookahead;

    Some(ident.to_string())
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

        // include! pulls in a file the checker never sees
        if mac
            .path
            .segments
            .last()
            .is_some_and(|s| s.ident == "include")
        {
            self.violations
                .push("include! escapes the checked tree".into());
        }

        self.check_tokens(mac.tokens.clone());
    }

    // `#[derive(a::B)]`, `#[cfg_attr(..)]` and friends carry paths as tokens
    fn visit_meta_list(&mut self, list: &'ast MetaList) {
        self.visit_path(&list.path);

        self.check_tokens(list.tokens.clone());
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

// `#[path = ..]` directly or smuggled in `#[cfg_attr(.., path = ..)]`, however deeply nested
fn has_path_attr(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("path")
            || attr.path().is_ident("cfg_attr")
                && matches!(&attr.meta, Meta::List(list) if mentions(list.tokens.clone(), "path"))
    })
}

// whether any identifier, in a macro too, is `name`; comments are no tokens
fn mentions(tokens: TokenStream, name: &str) -> bool {
    tokens.into_iter().any(|token| match token {
        TokenTree::Ident(ident) => ident == name,
        TokenTree::Group(group) => mentions(group.stream(), name),
        _ => false,
    })
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

    // service.rs is the only exception, `pub mod service;` in island/mod.rs needs none
    for (path, depth) in files {
        let source = fs::read_to_string(&path).unwrap();

        let found = violations(&source, depth);

        assert!(found.is_empty(), "{}: {found:?}", path.display());
    }
}

// every .rs under `folder`, outside `skip`
fn rust_files(folder: &Path, skip: &Path, out: &mut Vec<PathBuf>) {
    for path in fs::read_dir(folder).unwrap().map(|e| e.unwrap().path()) {
        if path == skip {
            continue;
        }

        if path.is_dir() {
            rust_files(&path, skip, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

// a crate is reached only by its name, `extern crate wayland_client as w` too
const WAYLAND: &str = "wayland_client";

#[test]
fn wayland_stays_in_doctor() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    let mut files = Vec::new();

    rust_files(&src, &src.join("doctor"), &mut files);

    assert!(files.iter().any(|path| path.ends_with("main.rs")));

    for path in files {
        let tokens: TokenStream = fs::read_to_string(&path).unwrap().parse().unwrap();

        assert!(
            !mentions(tokens, WAYLAND),
            "{} uses {WAYLAND}",
            path.display()
        );
    }

    let doctor = fs::read_to_string(src.join("doctor/wayland.rs")).unwrap();

    assert!(mentions(doctor.parse().unwrap(), WAYLAND));
}

// the runtime's color type, and what makes one from literals
const COLOR: &str = "Color";
const CONSTRUCTORS: [&str; 3] = ["rgb", "rgba", "from"];

// finds colors written as literals: `Color::WHITE`, `Color::rgb(1, 2, 3)`, `"#fff"`
struct Literals {
    // `Color` and whatever a `use` renamed it to
    names: HashSet<String>,
    found: Vec<String>,
}

impl Literals {
    // `segments` name a constant of the color type, like `Color::WHITE` or `kanade_runtime::Color::BLACK`
    fn constant(&self, segments: &[String]) -> bool {
        let [.., ty, name] = segments else {
            return false;
        };

        self.names.contains(ty) && name.starts_with(|c: char| c.is_ascii_uppercase())
    }

    // `segments` name a constructor of the color type, like `Color::rgb`
    fn constructor(&self, segments: &[String]) -> bool {
        let [.., ty, name] = segments else {
            return false;
        };

        self.names.contains(ty) && CONSTRUCTORS.contains(&name.as_str())
    }

    // macro bodies are opaque to syn, so the same rules run on their tokens
    fn check_tokens(&mut self, tokens: TokenStream) {
        let mut tokens = tokens.into_iter().peekable();

        while let Some(token) = tokens.next() {
            match token {
                TokenTree::Ident(ident) => {
                    let mut segments = vec![ident.to_string()];

                    while let Some(next) = take_path_segment(&mut tokens) {
                        segments.push(next);
                    }

                    let literal_arguments = matches!(
                        tokens.peek(),
                        Some(TokenTree::Group(group)) if group.stream().into_iter().any(|token| {
                            matches!(token, TokenTree::Literal(_))
                        })
                    );

                    if self.constant(&segments) || self.constructor(&segments) && literal_arguments
                    {
                        self.found
                            .push(format!("in a macro: {}", segments.join("::")));
                    }
                }
                TokenTree::Literal(literal) => {
                    if let Ok(text) = syn::parse_str::<LitStr>(&literal.to_string())
                        && hex(&text.value())
                    {
                        self.found.push(format!("in a macro: {literal}"));
                    }
                }
                TokenTree::Group(group) => self.check_tokens(group.stream()),
                TokenTree::Punct(_) => {}
            }
        }
    }
}

// what the runtime's `Color::from` reads: 3, 6 or 8 hex digits, any leading `#` optional
fn hex(text: &str) -> bool {
    let digits = text.trim_start_matches('#');

    [3, 6, 8].contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_hexdigit())
}

// `#[test]`, or a `#[cfg(..)]` that holds only under test, like `cfg(all(test, unix))`
fn test_only(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("test")
            || attr.path().is_ident("cfg")
                && attr
                    .parse_args::<Meta>()
                    .is_ok_and(|meta| needs_test(&meta))
    })
}

// a cfg predicate that is false outside test builds
fn needs_test(meta: &Meta) -> bool {
    let Meta::List(list) = meta else {
        return meta.path().is_ident("test");
    };

    let Ok(args) = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) else {
        return false;
    };

    if list.path.is_ident("all") {
        args.iter().any(needs_test)
    } else if list.path.is_ident("any") {
        !args.is_empty() && args.iter().all(needs_test)
    } else {
        false
    }
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

// an argument that is a literal, also negated, cast or in parentheses
fn literal(expr: &Expr) -> bool {
    match expr {
        Expr::Lit(_) => true,
        Expr::Unary(unary) => literal(&unary.expr),
        Expr::Cast(cast) => literal(&cast.expr),
        Expr::Paren(paren) => literal(&paren.expr),
        _ => false,
    }
}

fn segments(path: &SynPath) -> Vec<String> {
    path.segments.iter().map(|s| s.ident.to_string()).collect()
}

// every `use .. Color as X`, so `X::WHITE` is caught too
struct Renames(HashSet<String>);

impl<'ast> Visit<'ast> for Renames {
    fn visit_use_rename(&mut self, rename: &'ast UseRename) {
        if rename.ident == COLOR {
            self.0.insert(rename.rename.to_string());
        }
    }
}

impl<'ast> Visit<'ast> for Literals {
    fn visit_item(&mut self, item: &'ast Item) {
        if !test_only(item_attrs(item)) {
            visit::visit_item(self, item);
        }
    }

    fn visit_impl_item_fn(&mut self, item: &'ast ImplItemFn) {
        if !test_only(&item.attrs) {
            visit::visit_impl_item_fn(self, item);
        }
    }

    fn visit_expr_path(&mut self, expr: &'ast ExprPath) {
        let segments = segments(&expr.path);

        if self.constant(&segments) {
            self.found.push(segments.join("::"));
        }

        visit::visit_expr_path(self, expr);
    }

    fn visit_expr_call(&mut self, call: &'ast ExprCall) {
        if let Expr::Path(func) = &*call.func {
            let segments = segments(&func.path);

            if self.constructor(&segments) && call.args.iter().any(literal) {
                self.found.push(format!("{}(..)", segments.join("::")));
            }
        }

        visit::visit_expr_call(self, call);
    }

    fn visit_lit_str(&mut self, lit: &'ast LitStr) {
        if hex(&lit.value()) {
            self.found.push(format!("{:?}", lit.value()));
        }
    }

    fn visit_macro(&mut self, mac: &'ast Macro) {
        self.check_tokens(mac.tokens.clone());
    }
}

// color literals in one file's non-test code
pub fn color_literals(source: &str) -> Vec<String> {
    let file = syn::parse_file(source).expect("source file must parse");

    let mut renames = Renames(HashSet::from([COLOR.to_owned()]));
    renames.visit_file(&file);

    let mut literals = Literals {
        names: renames.0,
        found: Vec::new(),
    };
    literals.visit_file(&file);

    literals.found
}

#[test]
fn colors_stay_in_theme() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let theme = src.join("theme.rs");

    let mut files = Vec::new();

    rust_files(&src, &theme, &mut files);

    assert!(files.iter().any(|path| path.ends_with("view.rs")));

    for path in files {
        let found = color_literals(&fs::read_to_string(&path).unwrap());

        assert!(found.is_empty(), "{}: {found:?}", path.display());
    }

    assert!(!color_literals(&fs::read_to_string(theme).unwrap()).is_empty());
}

#[cfg(test)]
mod checker {
    use super::{WAYLAND, color_literals, mentions, violations};

    fn flagged(source: &str, depth: usize) -> bool {
        !violations(source, depth).is_empty()
    }

    #[test]
    fn catches_direct_and_aliased_kanade_runtime() {
        assert!(flagged("use kanade_runtime::Color;", 1));
        assert!(flagged("use kanade_runtime as ui;", 1));
        assert!(flagged("use ::kanade_runtime::Color;", 1));
        assert!(flagged("use {kanade_runtime::Color, std::fmt};", 1));
        assert!(flagged("fn f() -> kanade_runtime::Color { todo!() }", 1));
        assert!(flagged("extern crate kanade_runtime as ui;", 1));
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
        assert!(flagged("mod inner { use kanade_runtime::Color; }", 1));
        assert!(flagged(
            "#[cfg(test)] mod t { use kanade_runtime::Color; }",
            1
        ));
        assert!(flagged(
            "fn f() { println!(\"{}\", kanade_runtime::Color::BLUE); }",
            1
        ));
        assert!(flagged("#[path = \"../view.rs\"] mod v;", 1));
        assert!(flagged("#[path = \"../view.rs\"] mod v;", 0));
        assert!(flagged(
            "#[cfg_attr(test, path = \"../view.rs\")] mod v;",
            1
        ));
        assert!(flagged(
            "#[cfg_attr(test, path = \"../view.rs\")] mod v;",
            0
        ));
        assert!(flagged(
            "#[cfg_attr(a, cfg_attr(b, path = \"../view.rs\"))] mod v;",
            1
        ));
        assert!(flagged(
            "mod inner { #[cfg_attr(test, path = \"../view.rs\")] mod v; }",
            1
        ));
        assert!(flagged(
            "fn f() { let _ = matches!(crate::view::x(), _); }",
            1
        ));
        assert!(flagged(
            "fn f() { let _ = format!(\"{}\", crate::theme::CANVAS_WIDTH); }",
            1
        ));
        assert!(flagged(
            "fn f() { let _ = format!(\"{}\", service::X); }",
            1
        ));
        assert!(flagged(
            "fn f() { let _ = vec![[super::super::theme::X]]; }",
            1
        ));
        assert!(flagged("include!(\"../view.rs\");", 1));
        assert!(flagged(
            "macro_rules! m { () => { $crate::view::x() }; }",
            1
        ));
        assert!(flagged("#[derive(kanade_runtime::Thing)] struct S;", 1));
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
    fn finds_a_crate_named_anywhere_but_in_comments() {
        let named = |source: &str| mentions(source.parse().unwrap(), WAYLAND);

        assert!(named("use wayland_client::Connection;"));
        assert!(named("extern crate wayland_client as w;"));
        assert!(named(
            "fn f() { let _ = vec![::wayland_client::Connection::connect_to_env()]; }"
        ));
        assert!(!named(
            "// wayland_client\nfn f() { let _ = \"wayland_client\"; }"
        ));
    }

    #[test]
    fn catches_color_literals_however_written() {
        let caught = |source: &str| !color_literals(source).is_empty();

        assert!(caught("const C: Color = Color::rgb(1, 2, 3);"));
        assert!(caught("fn f() -> Color { Color::rgba(0, 0, 0, 89) }"));
        assert!(caught("fn f() -> Color { kanade_runtime::Color::WHITE }"));
        assert!(caught(
            "use kanade_runtime::Color as Ink; fn f() -> Ink { Ink::BLACK }"
        ));
        assert!(caught(
            "fn f() -> Color { Color::rgb(c.red(), 0, c.blue()) }"
        ));
        assert!(caught("fn f() -> Color { Color::rgb(-1 as u8, 0, 0) }"));
        assert!(caught("fn f() -> Color { Color::from(\"#ff0000\") }"));
        assert!(caught("fn f() -> Color { \"#fff\".into() }"));
        assert!(caught(r#"fn f() -> Color { "ff0000".into() }"#));
        assert!(caught(r#"fn f() -> Color { "fff".into() }"#));
        assert!(caught(r#"fn f() -> Color { "ff000080".into() }"#));
        assert!(caught("fn f() -> Color { \"##fff\".into() }"));
        assert!(caught(r#"fn f() { let _ = vec!["ff0000"]; }"#));
        assert!(caught("fn f() { let _ = vec![Color::TRANSPARENT]; }"));
        assert!(caught(
            "fn f() { let _ = vec![Text::new(\"\").color(Color::rgb(1, 2, 3))]; }"
        ));
        assert!(caught(
            "fn f() { let _ = children![x.color(\"#123456\".into())]; }"
        ));
        assert!(caught("impl S { fn f() -> Color { Color::RED } }"));
        assert!(caught("#[cfg(not(test))] fn f() -> Color { Color::RED }"));
        assert!(caught(
            "#[cfg(any(test, unix))] fn f() -> Color { Color::RED }"
        ));
        assert!(caught("#[cfg(unix)] fn f() -> Color { Color::RED }"));
        assert!(caught("mod inner { const C: Color = Color::GREEN; }"));
    }

    #[test]
    fn allows_tokens_and_test_code() {
        let caught = |source: &str| !color_literals(source).is_empty();

        assert!(!caught("fn f() -> Color { theme::island().surface }"));
        assert!(!caught(
            "fn f(r: u8, g: u8, b: u8) -> Color { Color::rgb(r, g, b) }"
        ));
        assert!(!caught(
            "fn f(c: Color) -> Color { theme::faded(c, theme::DISABLED) }"
        ));
        assert!(!caught("fn f() { let _ = format!(\"#{}\", 1); }"));
        assert!(!caught("fn f() { let _ = \"#tag\"; }"));
        assert!(!caught(r#"fn f() { let _ = "ffff"; }"#));
        assert!(!caught(
            "#[cfg(all(test, target_os = \"linux\"))] mod t { const C: Color = Color::RED; }"
        ));
        assert!(!caught(
            "#[cfg(any(test, all(test, unix)))] fn f() -> Color { Color::RED }"
        ));
        assert!(!caught(
            "#[cfg(test)] mod tests { const C: Color = Color::RED; }"
        ));
        assert!(!caught(
            "#[test] fn t() { assert_ne!(Color::RED, Color::rgb(1, 2, 3)); }"
        ));
        assert!(!caught(
            "impl S { #[cfg(test)] fn f() -> Color { Color::RED } }"
        ));
        assert!(!caught("// Color::WHITE\nfn f() {}"));
    }

    #[test]
    fn allows_pure_code() {
        assert!(!flagged("use std::time::Duration;", 1));
        assert!(!flagged("use super::activity::Activity;", 1));
        assert!(!flagged("use crate::island::activity::Activity;", 1));
        assert!(!flagged("fn f(service: u8) -> u8 { service }", 1));
        assert!(!flagged("mod inner { use super::Thing; }", 1));
        assert!(!flagged(
            "fn f() { let _ = format!(\"{}\", crate::island::x::Y); }",
            1
        ));
        assert!(!flagged(
            "fn f(service: u8) { let _ = format!(\"{}\", service); }",
            1
        ));
        assert!(!flagged("#[derive(Debug, Clone)] struct S;", 1));
        assert!(!flagged("pub mod service;", 0));
        assert!(!flagged("#[cfg_attr(test, derive(Debug))] struct S;", 1));
        assert!(!flagged("#[cfg(test)] mod tests {}", 1));
    }
}
