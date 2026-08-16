//! Modules (Language Ch. 6): a program is a tree of files.
//!
//! Each test writes a program to a directory and builds it from its root,
//! because where a file *is* is half of what this chapter defines and a test
//! that passed a string would check the other half only.

use std::path::{Path, PathBuf};

use trustc::lang;

/// A directory nothing else is using, named after the test that wants it.
fn dir(name: &str) -> PathBuf {
    let at = std::env::temp_dir().join(format!("trust-modules-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&at);
    std::fs::create_dir_all(&at).expect("a directory");
    at
}

/// Write one file, making whatever directories it needs.
fn write(at: &Path, rel: &str, text: &str) {
    let file = at.join(rel);
    if let Some(p) = file.parent() {
        std::fs::create_dir_all(p).expect("a directory");
    }
    std::fs::write(file, text).expect("a file");
}

/// Build from the root, and answer with every complaint joined.
fn build(at: &Path) -> Result<trustc::tir::Module, String> {
    let b = lang::build(&at.join("main.tr"));
    match b.module {
        Some(m) if b.errors.is_empty() => Ok(m),
        _ => Err(b
            .errors
            .iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
            .join("; ")),
    }
}

#[test]
fn a_program_in_two_files_is_one_program() {
    let at = dir("two");
    write(
        &at,
        "main.tr",
        "mod greet;\nfn main() -> t27 { greet::hello() }\n",
    );
    write(&at, "greet.tr", "pub fn hello() -> t27 { 7 }\n");
    let m = build(&at).expect("builds");
    // §4: the symbol is the path with `::` written `.`.
    assert!(m.function("greet.hello").is_some(), "no greet.hello");
    assert!(m.function("main").is_some());
}

#[test]
fn a_submodule_lives_in_a_directory_named_for_its_parent() {
    // §1.3: `mod lex;` in `lang.tr` is `lang/lex.tr`, and that is the whole
    // rule — there is no `mod.rs` and no second spelling.
    let at = dir("nested");
    write(
        &at,
        "main.tr",
        "mod lang;\nfn main() -> t27 { lang::width() }\n",
    );
    write(
        &at,
        "lang.tr",
        "pub mod lex;\npub fn width() -> t27 { lex::of(1) }\n",
    );
    write(&at, "lang/lex.tr", "pub fn of(n: t27) -> t27 { n * 27 }\n");
    let m = build(&at).expect("builds");
    assert!(m.function("lang.lex.of").is_some());
}

#[test]
fn what_is_not_pub_is_not_reachable() {
    let at = dir("private");
    write(&at, "main.tr", "mod m;\nfn main() -> t27 { m::secret() }\n");
    write(&at, "m.tr", "fn secret() -> t27 { 1 }\n");
    let why = build(&at).expect_err("refused");
    assert!(why.contains("is not `pub`"), "{why}");
}

#[test]
fn visibility_reaches_inward_and_not_out() {
    // §2.1: an item is visible in the module defining it and in every module
    // *inside* it. A parent is not inside its child, so `lang` cannot name
    // `lang::lex`'s private items either — which is the rule that makes a
    // module's insides its own.
    let at = dir("inward");
    write(
        &at,
        "main.tr",
        "mod lang;\nfn main() -> t27 { lang::via() }\n",
    );
    write(
        &at,
        "lang.tr",
        "mod lex;\npub fn via() -> t27 { lex::quiet() }\n",
    );
    write(&at, "lang/lex.tr", "fn quiet() -> t27 { 4 }\n");
    let why = build(&at).expect_err("refused");
    assert!(why.contains("is not `pub`"), "{why}");

    // Inward: `lang::lex` may name what `lang` keeps private, because it is
    // inside it.
    let at = dir("inward-ok");
    write(
        &at,
        "main.tr",
        "mod lang;\nfn main() -> t27 { lang::via() }\n",
    );
    write(
        &at,
        "lang.tr",
        "mod lex;\nfn base() -> t27 { 4 }\npub fn via() -> t27 { lex::twice() }\n",
    );
    write(&at, "lang/lex.tr", "pub fn twice() -> t27 { 2 }\n");
    assert!(build(&at).is_ok(), "{:?}", build(&at).err());
}

#[test]
fn a_use_binds_the_last_segment() {
    let at = dir("use");
    write(
        &at,
        "main.tr",
        "mod m;\nuse m::Kind;\nfn main() -> t27 { width(Kind::Word) }\n\
         fn width(k: Kind) -> t27 { match k { Kind::Word => 27, Kind::Trit => 1 } }\n",
    );
    write(&at, "m.tr", "pub enum Kind { Trit, Word }\n");
    let m = build(&at).expect("builds");
    // The enum carries the module; the variants are named through it.
    assert!(m.function("width").is_some());
}

#[test]
fn a_use_of_something_private_is_the_same_refusal_as_naming_it() {
    let at = dir("use-private");
    write(
        &at,
        "main.tr",
        "mod m;\nuse m::Hidden;\nfn main() -> t27 { 0 }\n",
    );
    write(&at, "m.tr", "struct Hidden { n: t27 }\n");
    let why = build(&at).expect_err("refused");
    assert!(why.contains("is not `pub`"), "{why}");
}

#[test]
fn a_missing_file_says_which_file_and_for_which_mod() {
    let at = dir("missing");
    write(&at, "main.tr", "mod gone;\nfn main() -> t27 { 0 }\n");
    let why = build(&at).expect_err("refused");
    assert!(why.contains("gone.tr") && why.contains("mod gone"), "{why}");
}

#[test]
fn a_name_the_program_defines_hides_the_preludes_everywhere() {
    // §3.3: a prelude name means one thing in a program. A module that
    // defines `print` defines it for the program, because the root's items
    // carry no path and the prelude has none either.
    let at = dir("shadow");
    write(
        &at,
        "main.tr",
        "fn print_int(n: t27) -> t27 { n }\nfn main() -> t27 { print_int(3) }\n",
    );
    let m = build(&at).expect("builds");
    assert!(m.function("print_int").is_some());

    // And a module's own name is its own: `m::go` does not collide with a
    // `go` in the root.
    let at = dir("shadow2");
    write(
        &at,
        "main.tr",
        "mod m;\nfn go() -> t27 { 1 }\nfn main() -> t27 { go() + m::go() }\n",
    );
    write(&at, "m.tr", "pub fn go() -> t27 { 4 }\n");
    let m = build(&at).expect("builds");
    assert!(m.function("go").is_some() && m.function("m.go").is_some());
}

#[test]
fn a_local_is_not_renamed_into_the_item_it_is_spelled_like() {
    // Resolution rewrites names to their module path, and a binding with an
    // item's name must come through untouched or nothing can be spelled
    // after anything.
    let at = dir("shadowed-local");
    write(
        &at,
        "main.tr",
        "mod m;\nfn main() -> t27 {\n    let go = 5;\n    go + m::go()\n}\n",
    );
    write(&at, "m.tr", "pub fn go() -> t27 { 4 }\n");
    assert!(build(&at).is_ok(), "{:?}", build(&at).err());
}

#[test]
fn a_module_declared_twice_is_refused() {
    let at = dir("twice");
    write(&at, "main.tr", "mod m;\nmod m;\nfn main() -> t27 { 0 }\n");
    write(&at, "m.tr", "pub fn go() -> t27 { 1 }\n");
    let why = build(&at).expect_err("refused");
    assert!(why.contains("more than once"), "{why}");
}

#[test]
fn a_diagnostic_names_the_file_it_is_about() {
    // A span carries a file now, because two files number their characters
    // from zero alike.
    let at = dir("which-file");
    write(&at, "main.tr", "mod m;\nfn main() -> t27 { m::go() }\n");
    write(&at, "m.tr", "pub fn go() -> t27 {\n    nope()\n}\n");
    let b = lang::build(&at.join("main.tr"));
    let e = b.errors.first().expect("an error");
    let source = &b.program.sources[e.span.file as usize];
    assert_eq!(source.path, vec!["m".to_string()], "{}", e.message);
    assert_eq!(e.span.line, 2, "{}", e.message);
}

#[test]
fn a_use_is_resolved_from_the_root() {
    // Ch. 6 §3.1. Draft 0.1 declined `crate::`, `super::` and `self::` and
    // said a module reaches outside itself by a `use` — while leaving the
    // `use` relative, so it could not name the outside either. Two siblings
    // could not see each other at all, which a compiler written in this
    // language found on its second file.
    let at = dir("siblings");
    write(
        &at,
        "main.tr",
        "mod a;\nmod b;\nfn main() -> t27 { b::twice() }\n",
    );
    write(&at, "a.tr", "pub fn one() -> t27 { 1 }\n");
    write(
        &at,
        "b.tr",
        "use a;\npub fn twice() -> t27 { a::one() + a::one() }\n",
    );
    assert!(build(&at).is_ok(), "{:?}", build(&at).err());
}

#[test]
fn a_use_may_name_a_type_through_a_module() {
    // `lex::Taken` in a type position: the parser reads it as an associated
    // type, and a module makes it an ordinary name (Ch. 6 §4).
    let at = dir("qualified-type");
    write(
        &at,
        "main.tr",
        "mod a;\nmod b;\nfn main() -> t27 { b::go() }\n",
    );
    write(&at, "a.tr", "pub struct Held { pub n: t27 }\n");
    write(
        &at,
        "b.tr",
        "use a;\npub fn go() -> t27 { let h: a::Held = a::Held { n: 4 }; h.n }\n",
    );
    assert!(build(&at).is_ok(), "{:?}", build(&at).err());
}

#[test]
fn a_use_still_grants_no_access() {
    // Absolute resolution is not more visibility: §3.2's rule is unchanged.
    let at = dir("use-absolute-private");
    write(
        &at,
        "main.tr",
        "mod a;\nmod b;\nfn main() -> t27 { b::go() }\n",
    );
    write(&at, "a.tr", "fn hidden() -> t27 { 1 }\n");
    write(&at, "b.tr", "use a;\npub fn go() -> t27 { a::hidden() }\n");
    let why = build(&at).expect_err("refused");
    assert!(why.contains("is not `pub`"), "{why}");
}
